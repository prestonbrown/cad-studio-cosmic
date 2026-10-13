//! The engine bridge: turn driving, the send queue, and the drain loop that
//! carries [`EngineEvent`]s to the app layer — the port of upstream's
//! Project.swift turn plumbing (sendDraft/send at 177-200, the queue at
//! 72-73 and 279-304) wrapped around the engine's
//! [`Session::send`] lifecycle dance.
//!
//! The bridge is split in two halves the message stream joins:
//! - a [`Bridge`] is a thin command handle (`enqueue`/`cancel`/
//!   `resume_queue`) the app layer calls from `update`;
//! - a driver task owns the [`Session`], the FIFO queue, and the pause flag,
//!   and turns engine events into [`Message`]s the app applies to its
//!   [`crate::chat::ChatState`] — the same events also fold into a driver-side
//!   transcript that `save_chat` persists at every turn end
//!   (upstream `agentDidFinish`'s `chat.save()`, Project.swift:262-278), so
//!   chat.json survives with no window open.
//!
//! The drain law (plan Review Focus 1): commands are absorbed between EVERY
//! pair of event translations and selected against while waiting for the next
//! event, and the forward channel is unbounded — a slow consumer (or the
//! engine's 256-deep backpressure) can never wedge a cancel.
//!
//! Queue semantics (Project.swift:290-304): a send while a turn runs is
//! queued FIFO; after a successful turn the next queued send auto-runs after
//! 1.0 s; ANY other end — failure, cancellation (upstream reports cancel as
//! `continueQueue(succeeded: false)`), or the event channel closing without
//! a [`EngineEvent::TurnFinished`] — pauses the queue; `resume_queue` is
//! upstream `sendQueuedNow`. A send arriving while NO turn runs starts
//! immediately even with items queued (upstream `send` guards `isBusy`
//! only); the 1.0 s delay such a send preempts is re-armed at that turn's
//! end instead of firing between the two — at most one second of drift in
//! that race, deliberate.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Duration;

use cosmic::iced::futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use engine::agent::claude::TurnRequest;
use engine::agent::session::{EngineEvent, Session, TurnHandle, TurnOutcome};
use engine::project::store::{self, ChatMessage, Role};
use engine::settings::Settings;
use time::OffsetDateTime;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::mpsc::{self, UnboundedReceiver as CmdReceiver, UnboundedSender as CmdSender};
use uuid::Uuid;

use crate::chat::ChatState;

/// The CAD contract every agent turn runs under — upstream loads
/// `CADSystemPrompt.md` from its app bundle and passes an empty string when
/// the resource is missing; an empty template here would mean turns without
/// the contract, so [`Bridge`] is built with this verbatim copy of the
/// upstream fixture.
pub const SYSTEM_TEMPLATE: &str = include_str!("../resources/CADSystemPrompt.md");

/// The gap between a successful turn and the next queued send
/// (Project.swift:296-300's `Task.sleep(for: .seconds(1))`).
const QUEUE_DELAY: Duration = Duration::from_secs(1);

/// What the driver reports to the app layer. One variant per engine event,
/// plus the lifecycle edges the events cannot carry: a turn starting (the
/// user row to append — upstream appends it in `send`, before the agent
/// runs), the event channel closing without a terminal (fail open and leave
/// the running state), and queue/pause changes (upstream exposes
/// `queue`/`isQueuePaused` as observed state the queue list renders).
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// A send actually started (never a queued one): append the user row.
    TurnStarted {
        prompt: String,
        attachments: Vec<String>,
    },
    /// One engine event, in arrival order — apply to the chat state.
    Event(EngineEvent),
    /// The event channel closed without a [`EngineEvent::TurnFinished`]:
    /// the running tools have been failed open and saved; leave the
    /// running state.
    Closed,
    /// The queue's contents and pause state after a change.
    QueueChanged {
        queue: Vec<QueuedItem>,
        paused: bool,
    },
}

/// One command from the app layer to the driver.
#[derive(Debug, Clone, PartialEq)]
enum Command {
    Enqueue {
        prompt: String,
        attachments: Vec<String>,
    },
    Cancel,
    ResumeQueue,
    /// Drop one queued send by id (upstream `removeQueued`,
    /// Project.swift:278-283): the queue un-pauses when this empties it.
    RemoveQueued(Uuid),
}

/// A queued send (upstream `QueuedMessage`): the FIFO entry the queue holds
/// while a turn runs. The id is the identity the queue list renders and
/// [`Bridge::remove_queued`] addresses (upstream's items are full
/// `ChatMessage`s with UUIDs).
#[derive(Debug, Clone, PartialEq)]
struct QueuedSend {
    id: Uuid,
    prompt: String,
    attachments: Vec<String>,
}

/// A queued send as the app layer sees the queue (upstream exposes the full
/// `ChatMessage` items; the shell needs identity, preview text and the image
/// count).
#[derive(Debug, Clone, PartialEq)]
pub struct QueuedItem {
    pub id: Uuid,
    pub text: String,
    pub images: usize,
}

impl From<&QueuedSend> for QueuedItem {
    fn from(send: &QueuedSend) -> Self {
        Self {
            id: send.id,
            text: send.prompt.clone(),
            images: crate::chat::image_attachments(&send.attachments),
        }
    }
}

/// The app layer's handle onto the driver: clone-free by design, so dropping
/// it is the shutdown signal (the driver cancels any running turn and reaps
/// the engine's process registry — upstream's quit path reaps orphans).
pub struct Bridge {
    commands: CmdSender<Command>,
}

impl Bridge {
    /// The bridge plus the message stream to return from the app's `init` or
    /// `update`. Must be called inside the tokio runtime cosmic runs on.
    pub fn new(settings: Settings, root: impl Into<PathBuf>) -> (Self, cosmic::Task<Message>) {
        let (bridge, messages) = Self::launch(settings, root);
        (bridge, cosmic::task::stream(messages))
    }

    /// [`Bridge::new`] without the `cosmic::Task` wrapper: spawns the driver
    /// and returns its message stream raw. A `cosmic::Task`'s stream is
    /// private to the iced runtime, so headless drivers (tests, the Task 8
    /// slice) tap here — the same seam pattern as the engine's
    /// `CAD_TEST_CLAUDE` override.
    pub fn launch(
        settings: Settings,
        root: impl Into<PathBuf>,
    ) -> (Self, UnboundedReceiver<Message>) {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (msg_tx, msg_rx) = unbounded();
        let session = Session::new(settings).with_system_template(SYSTEM_TEMPLATE);
        tokio::spawn(Drive::new(root.into(), cmd_rx, msg_tx).run(session));
        (Self { commands: cmd_tx }, msg_rx)
    }

    /// Send a message: runs immediately when no turn is running, joins the
    /// queue FIFO otherwise (upstream `sendDraft`). The prompt is the user's
    /// text verbatim — the engine builds the full turn prompt around it.
    pub fn enqueue(&self, prompt: String, attachments: Vec<String>) {
        let _ = self.commands.send(Command::Enqueue {
            prompt,
            attachments,
        });
    }

    /// Drop one queued send (upstream `removeQueued`, Project.swift:278-283):
    /// the running turn is untouched; the queue un-pauses when this empties
    /// it.
    pub fn remove_queued(&self, id: Uuid) {
        let _ = self.commands.send(Command::RemoveQueued(id));
    }

    /// Interrupt the running turn (SIGINT now, SIGTERM after 3 s). The
    /// running turn only: the queue is untouched and pauses when the
    /// cancelled turn ends, matching upstream's stop.
    pub fn cancel(&self) {
        let _ = self.commands.send(Command::Cancel);
    }

    /// Un-pause the queue and start its next message now (upstream
    /// `sendQueuedNow`).
    pub fn resume_queue(&self) {
        let _ = self.commands.send(Command::ResumeQueue);
    }
}

/// How a drained turn ended, for the driver's queue decision.
#[derive(Debug, Clone, Copy, PartialEq)]
enum TurnEnd {
    /// A [`EngineEvent::TurnFinished`] arrived; `succeeded` is upstream's
    /// `failure == nil` (false for every failure AND cancellation).
    Finished { succeeded: bool },
    /// The event channel closed with no terminal event.
    Closed,
    /// The bridge handle was dropped mid-turn; the driver exits.
    Shutdown,
}

/// The driver's phase: idle, draining a turn, or in the 1.0 s gap between a
/// successful turn and the next queued send. The gap is a deadline, not a
/// spawned sleep, so re-entering the phase (a cancel arriving mid-gap)
/// restarts the wait where it left off.
enum Phase {
    Idle,
    Running {
        handle: TurnHandle,
        transcript: ChatState,
    },
    Delay {
        auto_at: tokio::time::Instant,
    },
}

/// The driver: everything the message stream's far side owns — the session,
/// the queue, the pause flag, and the transcript that persists. One `run`
/// call is the bridge's lifetime.
struct Drive {
    root: PathBuf,
    queue: VecDeque<QueuedSend>,
    paused: bool,
    cmds: CmdReceiver<Command>,
    out: UnboundedSender<Message>,
}

impl Drive {
    fn new(root: PathBuf, cmds: CmdReceiver<Command>, out: UnboundedSender<Message>) -> Self {
        Self {
            root,
            queue: VecDeque::new(),
            paused: false,
            cmds,
            out,
        }
    }

    async fn run(mut self, mut session: Session) {
        let mut phase = Phase::Idle;
        loop {
            phase = match phase {
                Phase::Idle => match self.cmds.recv().await {
                    Some(Command::Enqueue {
                        prompt,
                        attachments,
                    }) => self.start_turn(
                        &mut session,
                        QueuedSend {
                            id: Uuid::new_v4(),
                            prompt,
                            attachments,
                        },
                    ),
                    // nothing is running: upstream's stop is a no-op
                    Some(Command::Cancel) => Phase::Idle,
                    // removing from an empty-idle queue can only un-pause
                    Some(Command::RemoveQueued(_)) => {
                        self.paused = false;
                        self.notify_queue();
                        Phase::Idle
                    }
                    Some(Command::ResumeQueue) => {
                        self.paused = false;
                        self.notify_queue();
                        self.send_next_queued(&mut session)
                    }
                    None => {
                        engine::processes::reap_orphans();
                        return;
                    }
                },
                Phase::Running {
                    mut handle,
                    transcript,
                } => {
                    // split the receiver from the interrupt switch once: the
                    // drain loop selects on the receiver while calling
                    // `cancel`, two borrows of one handle the borrow checker
                    // cannot express through `&TurnHandle`. The dummy
                    // receiver left in the handle is never polled.
                    let mut events = std::mem::replace(&mut handle.events, mpsc::channel(1).1);
                    let end = self
                        .drain_turn(&mut events, || handle.cancel(), transcript)
                        .await;
                    match end {
                        TurnEnd::Finished { succeeded } => self.continue_queue(succeeded),
                        TurnEnd::Closed => self.continue_queue(false),
                        TurnEnd::Shutdown => {
                            engine::processes::reap_orphans();
                            return;
                        }
                    }
                }
                Phase::Delay { auto_at } => tokio::select! {
                    _ = tokio::time::sleep_until(auto_at) => self.send_next_queued(&mut session),
                    cmd = self.cmds.recv() => match cmd {
                        // no turn is running, so a direct send starts now —
                        // and eats the pending delay: its own turn end
                        // re-arms the queue's next step instead
                        Some(Command::Enqueue { prompt, attachments }) => {
                            self.start_turn(
                                &mut session,
                                QueuedSend {
                                    id: Uuid::new_v4(),
                                    prompt,
                                    attachments,
                                },
                            )
                        }
                        // nothing to interrupt; the gap keeps running
                        Some(Command::Cancel) => Phase::Delay { auto_at },
                        // an emptied queue un-pauses and ends the gap
                        Some(Command::RemoveQueued(id)) => {
                            self.queue.retain(|send| send.id != id);
                            if self.queue.is_empty() {
                                self.paused = false;
                                self.notify_queue();
                                Phase::Idle
                            } else {
                                self.notify_queue();
                                Phase::Delay { auto_at }
                            }
                        }
                        Some(Command::ResumeQueue) => {
                            self.paused = false;
                            self.notify_queue();
                            self.send_next_queued(&mut session)
                        }
                        None => {
                            engine::processes::reap_orphans();
                            return;
                        }
                    },
                },
            };
        }
    }

    /// One command applied outside a drain loop's reach. `cancel` is the
    /// running turn's interrupt switch, split from the event receiver the
    /// drain loop owns.
    fn absorb(&mut self, cmd: Command, cancel: &impl Fn()) {
        match cmd {
            Command::Cancel => cancel(),
            Command::Enqueue {
                prompt,
                attachments,
            } => {
                self.queue.push_back(QueuedSend {
                    id: Uuid::new_v4(),
                    prompt,
                    attachments,
                });
                self.notify_queue();
            }
            // upstream `sendNextQueued` guards `isBusy`: the flag clears now,
            // the queue moves at this turn's end
            Command::ResumeQueue => {
                self.paused = false;
                self.notify_queue();
            }
            // upstream `removeQueued`: drop the entry, and an emptied queue
            // loses its pause (Project.swift:278-283)
            Command::RemoveQueued(id) => {
                self.queue.retain(|send| send.id != id);
                if self.queue.is_empty() {
                    self.paused = false;
                }
                self.notify_queue();
            }
        }
    }

    /// Start a turn (upstream `send`, Project.swift:192-208): append the user
    /// row, save, announce, and hand the request to the engine's lifecycle
    /// dance. The transcript reloads from disk — the last turn's save is this
    /// turn's base, exactly the crash-recovery shape `load_chat`'s
    /// `unwrap_or_default` gives a fresh project.
    fn start_turn(&mut self, session: &mut Session, send: QueuedSend) -> Phase {
        let mut transcript = ChatState::new(store::load_chat(&self.root).unwrap_or_default());
        transcript.append(ChatMessage {
            id: Uuid::new_v4(),
            role: Role::User,
            text: send.prompt.clone(),
            date: OffsetDateTime::now_utc(),
            tool: None,
            detail: None,
            action: None,
            // upstream lists the references/ copies here; the engine owns
            // the copy inside the turn and does not surface the names, so
            // the persisted row carries the request's paths as given
            attachments: send.attachments.clone(),
        });
        let _ = store::save_chat(&self.root, &transcript.messages);
        let _ = self.out.unbounded_send(Message::TurnStarted {
            prompt: send.prompt.clone(),
            attachments: send.attachments.clone(),
        });
        let request = TurnRequest {
            prompt: send.prompt,
            attachments: send.attachments.iter().map(PathBuf::from).collect(),
        };
        let handle = session.send(&self.root, &request);
        Phase::Running { handle, transcript }
    }

    /// Pop the queue's front and start it — upstream `sendNextQueued`'s
    /// guards minus `isBusy` (only ever called from idle states).
    fn send_next_queued(&mut self, session: &mut Session) -> Phase {
        if self.paused {
            return Phase::Idle;
        }
        match self.queue.pop_front() {
            Some(send) => {
                self.notify_queue();
                self.start_turn(session, send)
            }
            None => Phase::Idle,
        }
    }

    /// Upstream `continueQueue(succeeded:)` (Project.swift:290-300): an empty
    /// queue needs nothing, a failure pauses, a success schedules the next
    /// send after the delay.
    fn continue_queue(&mut self, succeeded: bool) -> Phase {
        if self.queue.is_empty() {
            return Phase::Idle;
        }
        if !succeeded {
            self.paused = true;
            self.notify_queue();
            return Phase::Idle;
        }
        Phase::Delay {
            auto_at: tokio::time::Instant::now() + QUEUE_DELAY,
        }
    }

    /// Announce the queue's observable state (upstream exposes
    /// `queue`/`isQueuePaused` directly; the driver's copy is unreachable
    /// from the app, so changes travel as messages).
    fn notify_queue(&self) {
        let _ = self.out.unbounded_send(Message::QueueChanged {
            queue: self.queue.iter().map(QueuedItem::from).collect(),
            paused: self.paused,
        });
    }

    /// Drain one turn's events: forward each as a [`Message::Event`] while
    /// folding it into the transcript, saving at the terminal. Commands are
    /// absorbed between every translation and selected against while idle —
    /// the cancel law. The event receiver is passed split from the handle
    /// (whose `cancel` the loop must reach while the receiver is borrowed);
    /// the dummy receiver left in the handle is never polled.
    async fn drain_turn(
        &mut self,
        events: &mut mpsc::Receiver<EngineEvent>,
        cancel: impl Fn(),
        mut transcript: ChatState,
    ) -> TurnEnd {
        loop {
            // between every pair of translations: absorb whatever arrived
            while let Ok(cmd) = self.cmds.try_recv() {
                self.absorb(cmd, &cancel);
            }
            let event = match events.try_recv() {
                Ok(event) => event,
                Err(TryRecvError::Empty) => tokio::select! {
                    event = events.recv() => match event {
                        Some(event) => event,
                        None => {
                            self.close_open_turn(&mut transcript);
                            return TurnEnd::Closed;
                        }
                    },
                    cmd = self.cmds.recv() => match cmd {
                        Some(cmd) => {
                            self.absorb(cmd, &cancel);
                            continue;
                        }
                        None => {
                            cancel();
                            self.close_open_turn(&mut transcript);
                            return TurnEnd::Shutdown;
                        }
                    },
                },
                Err(TryRecvError::Disconnected) => {
                    self.close_open_turn(&mut transcript);
                    return TurnEnd::Closed;
                }
            };
            // a dropped receiver is the app going away: keep translating so
            // the engine's channel keeps draining, but never block on it
            transcript.apply(&event);
            if let EngineEvent::TurnFinished(outcome) = &event {
                let succeeded = matches!(outcome, TurnOutcome::Success { .. });
                // the terminal saves BEFORE it forwards: a consumer that saw
                // the TurnFinished message can read chat.json immediately
                // (upstream's agentDidFinish saves before the UI hears)
                let _ = store::save_chat(&self.root, &transcript.messages);
                let _ = self.out.unbounded_send(Message::Event(event));
                return TurnEnd::Finished { succeeded };
            }
            let _ = self.out.unbounded_send(Message::Event(event));
        }
    }

    /// The channel-close turn end (plan Review Focus 4): running tools fail
    /// open, the partial transcript saves, the app layer hears
    /// [`Message::Closed`].
    fn close_open_turn(&mut self, transcript: &mut ChatState) {
        transcript.fail_running_tools();
        let _ = store::save_chat(&self.root, &transcript.messages);
        let _ = self.out.unbounded_send(Message::Closed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmic::iced::futures::StreamExt;
    use engine::agent::tool_summary::live_title;
    use engine::project::store::{ToolActivity, ToolState, load_chat};
    use std::path::Path;

    /// The CAD contract resource is present and is the upstream fixture
    /// byte-for-byte: an empty or drifted template means agent turns run
    /// without the contract the engine substitutes into
    /// `--append-system-prompt`.
    #[test]
    fn session_carries_system_template() {
        assert!(
            !SYSTEM_TEMPLATE.trim().is_empty(),
            "the template must not be empty"
        );
        let upstream = std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/upstream/CADSystemPrompt.md"),
        )
        .unwrap();
        assert_eq!(
            SYSTEM_TEMPLATE.as_bytes(),
            upstream.as_slice(),
            "the embedded resource is the upstream fixture verbatim"
        );
    }

    fn tool_row(id: &str) -> ChatMessage {
        ChatMessage {
            id: Uuid::new_v4(),
            role: Role::Tool,
            text: live_title("Write", ""),
            date: OffsetDateTime::now_utc(),
            tool: Some(ToolActivity {
                tool_use_id: id.into(),
                name: "Write".into(),
                input: String::new(),
                output: None,
                preview: None,
                state: ToolState::Running,
            }),
            detail: None,
            action: None,
            attachments: Vec::new(),
        }
    }

    /// A driver over synthetic channels, its command sender, and its
    /// message stream.
    fn harness(root: &Path) -> (Drive, CmdSender<Command>, UnboundedReceiver<Message>) {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (msg_tx, msg_rx) = unbounded();
        (
            Drive::new(root.to_path_buf(), cmd_rx, msg_tx),
            cmd_tx,
            msg_rx,
        )
    }

    /// The event channel closing without a TurnFinished (the engine task
    /// dying) ends the turn cleanly: the open tool row fails open and
    /// persists, the app layer hears `Closed`, and the queue treats it as a
    /// failure (plan Review Focus 4).
    #[tokio::test(flavor = "multi_thread")]
    async fn channel_close_without_turn_finished() {
        let root = std::env::temp_dir().join(format!("cad-bridge-close-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let (mut drive, _cmd_tx, mut messages) = harness(&root);
        let (ev_tx, mut ev_rx) = mpsc::channel::<EngineEvent>(8);
        ev_tx
            .send(EngineEvent::ToolStarted {
                id: "t1".into(),
                name: "Write".into(),
                title: live_title("Write", ""),
            })
            .await
            .unwrap();
        drop(ev_tx);

        let transcript = ChatState::new(vec![tool_row("t1")]);
        let end = drive.drain_turn(&mut ev_rx, || {}, transcript).await;
        assert_eq!(end, TurnEnd::Closed);
        drive.queue.push_back(QueuedSend {
            id: Uuid::new_v4(),
            prompt: "next".into(),
            attachments: Vec::new(),
        });
        drive.continue_queue(false);
        assert!(
            drive.paused,
            "a channel close pauses the queue like any failure"
        );

        let saved = load_chat(&root).unwrap();
        let tool = saved[0].tool.as_ref().unwrap();
        assert_eq!(
            tool.state,
            ToolState::Failed,
            "the open tool row failed open"
        );
        assert_eq!(tool.preview, None);
        assert_eq!(
            messages.next().await,
            Some(Message::Event(EngineEvent::ToolStarted {
                id: "t1".into(),
                name: "Write".into(),
                title: live_title("Write", ""),
            })),
            "events already in flight still forward before the close"
        );
        assert_eq!(messages.next().await, Some(Message::Closed));
    }

    /// A cancel command is absorbed between event translations — no engine,
    /// no spawn: the interrupt switch fires while events keep flowing.
    #[tokio::test(flavor = "multi_thread")]
    async fn cancel_command_lands_between_translations() {
        let root = std::env::temp_dir().join(format!("cad-bridge-cancel-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let (mut drive, cmd_tx, mut messages) = harness(&root);
        let (ev_tx, mut ev_rx) = mpsc::channel::<EngineEvent>(1);
        let fired = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = std::sync::Arc::clone(&fired);
        let cancel = move || flag.store(true, std::sync::atomic::Ordering::SeqCst);

        // one event, then the channel goes quiet: the cancel must land in
        // the idle wait, not only between ready events
        ev_tx.send(EngineEvent::ThinkingStarted).await.unwrap();
        let transcript = ChatState::new(Vec::new());
        let (end, ()) = tokio::join!(drive.drain_turn(&mut ev_rx, cancel, transcript), async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cmd_tx.send(Command::Cancel).unwrap();
            cmd_tx
                .send(Command::Enqueue {
                    prompt: "queued".into(),
                    attachments: Vec::new(),
                })
                .unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
            let _ = ev_tx
                .send(EngineEvent::TurnFinished(TurnOutcome::Cancelled))
                .await;
        });
        assert!(
            fired.load(std::sync::atomic::Ordering::SeqCst),
            "cancel fired mid-drain"
        );
        assert_eq!(end, TurnEnd::Finished { succeeded: false });
        assert_eq!(drive.queue.len(), 1, "the enqueue landed while draining");
        assert_eq!(
            messages.next().await,
            Some(Message::Event(EngineEvent::ThinkingStarted))
        );
    }

    /// `remove_queued` drops the addressed entry mid-drain and un-pauses an
    /// emptied queue (upstream `removeQueued`, Project.swift:278-283); the
    /// queue's observable contents travel with the change.
    #[tokio::test(flavor = "multi_thread")]
    async fn remove_queued_drops_entry_and_unpauses_when_empty() {
        let root = std::env::temp_dir().join(format!("cad-bridge-remove-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let (mut drive, cmd_tx, mut messages) = harness(&root);
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        drive.queue.push_back(QueuedSend {
            id: first,
            prompt: "first".into(),
            attachments: vec!["shot.png".into()],
        });
        drive.queue.push_back(QueuedSend {
            id: second,
            prompt: "second".into(),
            attachments: Vec::new(),
        });
        drive.paused = true;
        drive.notify_queue();

        drive.absorb(Command::RemoveQueued(first), &|| {});
        assert_eq!(drive.queue.len(), 1, "the addressed entry is gone");
        assert_eq!(drive.queue.front().unwrap().id, second);
        assert!(
            drive.paused,
            "a queue that still holds items keeps its pause"
        );

        drive.absorb(Command::RemoveQueued(second), &|| {});
        assert!(drive.queue.is_empty());
        assert!(!drive.paused, "emptying the queue clears the pause");

        let mut seen = Vec::new();
        while let Ok(m) = messages.try_recv() {
            if let Message::QueueChanged { queue, paused } = m {
                seen.push((queue, paused));
            }
        }
        assert_eq!(
            seen.last().map(|(queue, paused)| (queue.len(), *paused)),
            Some((0, false)),
            "the final announcement carries the emptied, un-paused queue"
        );
        // the item view carries the image count the queue list renders
        let before_removal = seen.first().and_then(|(queue, _)| queue.first().cloned());
        assert_eq!(
            before_removal.map(|item| (item.text, item.images)),
            Some(("first".to_string(), 1)),
            "queued items expose preview text and the image count"
        );
        drop(cmd_tx);
    }
}
