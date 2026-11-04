//! Probe instrument for the cosmic menu overlay's "Downcast widget state"
//! panic (docs/research/libcosmic-menu-dynamic-items-bug.md). NOT part of
//! the app: the record's probes run it on the rig under Xvfb, with the
//! probed root opened by xdotool while its items change.
//!
//! The bar has two roots: "Altro" (eight fixed `Item::Button`s, the shape
//! of the app's File root) and "Sonda", the probed root. Once the first
//! cycle fires, the probe rebuilds the Sonda items every `DYN_MENU_TICK_MS`
//! (default 75) for `DYN_MENU_CYCLES` cycles (default 200), then prints
//! `PROBE <probe> SURVIVED <n> cycles` and exits 0. A panic aborts the run
//! with its message on stderr.
//!
//!   DYN_MENU_PROBE=labels  fixed `Item::Button` set, every label's text
//!                          changes each cycle (the parameters flip)
//!   DYN_MENU_PROBE=check   fixed `Item::CheckBox` set, the ✓ moves each
//!                          cycle (the speed menu: rates plus Ripeti)
//!   DYN_MENU_PROBE=grow    `Item::Button` set grows 2→8 and shrinks back
//!   DYN_MENU_PROBE=custom  fixed set of `button::custom` leaves with
//!                          shrink height (the version menu's shape)
//!   DYN_MENU_PROBE=static  nothing changes (the control)
//!   DYN_MENU_ROOT=first    Sonda is root 0 instead of root 1
//!   DYN_MENU_PHASE=<n>     the cycle counter starts at n (=6 opens the
//!                          grow probe at 8 items, so it shrinks while open)
//!   DYN_MENU_START_MS=<ms> delay before the first cycle (default 6000),
//!                          the window for the driver to open the menu
use cosmic::app::{self, Core, Settings};
use cosmic::iced::Length;
use cosmic::iced::Task;
use cosmic::widget::menu::{self, Item, ItemHeight, ItemWidth};
use cosmic::widget::{button, container, text};
use cosmic::{Action, Element};
use std::collections::HashMap;
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Probe {
    Static,
    Labels,
    Check,
    Grow,
    Custom,
}

struct Demo {
    core: Core,
    probe: Probe,
    probe_first: bool,
    cycle: usize,
    /// The last cycle: the starting phase plus `DYN_MENU_CYCLES`.
    last: usize,
    tick: Duration,
}

#[derive(Clone, Debug)]
enum Message {
    Cycle,
    Nothing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NothingAction;

impl menu::action::MenuAction for NothingAction {
    type Message = Message;

    fn message(&self) -> Message {
        Message::Nothing
    }
}

fn stamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() % 86_400;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        secs / 3600,
        secs / 60 % 60,
        secs % 60,
        now.subsec_millis()
    )
}

fn after(delay: Duration) -> Task<Action<Message>> {
    Task::perform(async move { tokio::time::sleep(delay).await }, |()| {
        Message::Cycle
    })
    .map(Action::App)
}

fn env_num(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// The speed menu's rates (MotionControls.swift:35-48).
const RATES: [&str; 5] = ["0.25×", "0.5×", "1×", "2×", "4×"];

impl Demo {
    fn probe_children(&self) -> Vec<menu::Tree<Message>> {
        let n = self.cycle;
        match self.probe {
            Probe::Static => menu::items(
                &HashMap::new(),
                (0..5)
                    .map(|i| Item::Button(format!("Voce {i}"), None, NothingAction))
                    .collect::<Vec<_>>(),
            ),
            Probe::Labels => menu::items(
                &HashMap::new(),
                vec![
                    Item::Button(
                        if n.is_multiple_of(2) {
                            "Mostra parametri".to_string()
                        } else {
                            "Nascondi parametri".to_string()
                        },
                        None,
                        NothingAction,
                    ),
                    Item::Button(format!("Ciclo {n}"), None, NothingAction),
                    Item::Divider,
                    Item::Button("x".repeat(1 + n % 23), None, NothingAction),
                    Item::Button("Fisso".to_string(), None, NothingAction),
                ],
            ),
            Probe::Check => {
                let mut entries: Vec<Item<NothingAction, String>> = RATES
                    .iter()
                    .enumerate()
                    .map(|(i, rate)| {
                        Item::CheckBox(rate.to_string(), None, n % RATES.len() == i, NothingAction)
                    })
                    .collect();
                entries.push(Item::Divider);
                entries.push(Item::CheckBox(
                    "Ripeti".to_string(),
                    None,
                    n.is_multiple_of(2),
                    NothingAction,
                ));
                menu::items(&HashMap::new(), entries)
            }
            Probe::Grow => {
                // 2,3,..,8,7,..,3,2,3,..: removal happens while open too
                let phase = n % 12;
                let count = 2 + if phase <= 6 { phase } else { 12 - phase };
                menu::items(
                    &HashMap::new(),
                    (0..count)
                        .map(|i| Item::Button(format!("Voce {i} ({n})"), None, NothingAction))
                        .collect::<Vec<_>>(),
                )
            }
            Probe::Custom => (0..4)
                .map(|i| {
                    let body = cosmic::widget::Column::new()
                        .spacing(2)
                        .push(text(format!("Versione {i}")))
                        .push(text(format!("· {n} s fa")).size(11))
                        .padding([6, 16]);
                    let leaf: Element<'_, Message> = button::custom(body)
                        .class(cosmic::theme::Button::MenuItem)
                        .on_press(Message::Nothing)
                        .width(Length::Fill)
                        .into();
                    menu::Tree::new(leaf)
                })
                .collect(),
        }
    }
}

impl app::Application for Demo {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;

    const APP_ID: &'static str = "net.brownhouse.CadStudioDynMenuRepro";

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: ()) -> (Self, Task<Action<Message>>) {
        let probe = match std::env::var("DYN_MENU_PROBE").as_deref() {
            Ok("labels") => Probe::Labels,
            Ok("check") => Probe::Check,
            Ok("grow") => Probe::Grow,
            Ok("custom") => Probe::Custom,
            _ => Probe::Static,
        };
        let probe_first = std::env::var("DYN_MENU_ROOT").as_deref() == Ok("first");
        let phase = env_num("DYN_MENU_PHASE", 0) as usize;
        let cycles = env_num("DYN_MENU_CYCLES", 200) as usize;
        let demo = Self {
            core,
            probe,
            probe_first,
            cycle: phase,
            last: phase + cycles,
            tick: Duration::from_millis(env_num("DYN_MENU_TICK_MS", 75)),
        };
        eprintln!(
            "{} probe={probe:?} root={} cycles={} tick={:?}",
            stamp(),
            if probe_first { "first" } else { "second" },
            cycles,
            demo.tick
        );
        let start = Duration::from_millis(env_num("DYN_MENU_START_MS", 6000));
        (demo, after(start))
    }

    fn update(&mut self, message: Message) -> Task<Action<Message>> {
        match message {
            Message::Nothing => Task::none(),
            Message::Cycle => {
                if self.cycle >= self.last {
                    eprintln!(
                        "{} PROBE {:?} SURVIVED {} cycles",
                        stamp(),
                        self.probe,
                        self.cycle
                    );
                    std::process::exit(0);
                }
                self.cycle += 1;
                if self.cycle.is_multiple_of(20) || self.cycle == 1 {
                    eprintln!("{} cycle {}", stamp(), self.cycle);
                }
                after(self.tick)
            }
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let other = menu::Tree::with_children(
            Element::from(menu::root("Altro")),
            menu::items(
                &HashMap::new(),
                (0..8)
                    .map(|i| Item::Button(format!("Altro {i}"), None, NothingAction))
                    .collect::<Vec<_>>(),
            ),
        );
        // DYN_MENU_ROOT_LABEL=1: the probed root's own label is the checked
        // rate, as a speed menu titled by the current speed would be
        let root_label = if std::env::var("DYN_MENU_ROOT_LABEL").as_deref() == Ok("1") {
            RATES[self.cycle % RATES.len()].to_string()
        } else {
            "Sonda".to_string()
        };
        let probed =
            menu::Tree::with_children(Element::from(menu::root(root_label)), self.probe_children());
        let roots = if self.probe_first {
            vec![probed, other]
        } else {
            vec![other, probed]
        };
        let bar = menu::bar(roots)
            .item_width(ItemWidth::Static(260))
            .item_height(ItemHeight::Dynamic(30));
        container(bar).padding(20).into()
    }
}

fn main() -> cosmic::iced::Result {
    app::run::<Demo>(Settings::default(), ())
}
