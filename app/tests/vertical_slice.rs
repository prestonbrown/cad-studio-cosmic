//! The M4 vertical slice: one prompt driven end to end through the whole
//! shell — `create_project` scaffolds a real project, the bridge runs a
//! turn against the fake agent (scenario `slice.jsonl`: thinking, a Write,
//! a Bash whose `#copy-outputs` directive lands the golden outputs
//! mid-turn, then the report), every bridge message folds into a real
//! [`ProjectWindow`] exactly as the app layer maps them, the engine's
//! output watcher observes the writes, and the reloaded window state is
//! asserted: 2-part ModelScene, status-bar warnings 0, schematic page
//! tabs, complete transcript rows, and the persisted chat.json. The
//! window's element tree is smoke-checked (one frame built) in the empty
//! and full shapes — the viewer pane's established pattern that element
//! construction is pure.
//!
//! The harness is bridge_e2e's ENV_LOCK + bounded-poll discipline: the
//! fake reads its env at spawn time, so the lock is held across every
//! [set env → launch → turn] window.

use std::path::PathBuf;
use std::time::Duration;

use app::activity::Activity;
use app::bridge::{Bridge, Message as BridgeMessage};
use app::windows::project::{Message, ProjectWindow};
use cosmic::iced::futures::StreamExt;
use cosmic::iced::futures::channel::mpsc::UnboundedReceiver;
use engine::agent::session::{EngineEvent, TurnOutcome};
use engine::project::store::{self, Role, ToolState, load_chat, load_metadata};
use engine::settings::Settings;
use engine::watch::{ReloadKind, watch_directory};

/// The fake reads its env at spawn time and the session snapshots the login
/// environment per attempt, so every [set env → launch → message] window
/// must be exclusive across the parallel test threads.
static ENV_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

const SLICE_SID: &str = "bdd2e831-7c7c-422f-b50a-fc22b182bae3";
const WRITE_ID: &str = "toolu_013uZKwucpApefxH7dUCMw1L";
const BASH_ID: &str = "toolu_01PiSki6C6xfd2exZn7nabus";
const PROMPT: &str = "Make a box with a lid";
const REPORT: &str = "I wrote `model.py` and ran it: the box (base + lid, 2 parts), the print plate, and both drawing pages are in `output/`.";

fn fixture_fake(p: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures/fake-agent")
        .join(p)
}

fn fixture_golden() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/golden")
}

/// Point the CAD_* env at the slice scenario. The caller holds ENV_LOCK
/// for as long as spawns can still happen.
fn set_env(root: &std::path::Path, dump: &std::path::Path) {
    // SAFETY: all setters and readers of the CAD_* variables hold ENV_LOCK.
    unsafe {
        std::env::set_var("CAD_TEST_CLAUDE", fixture_fake("fake_claude.py"));
        std::env::set_var("CAD_FAKE_SCENARIO", fixture_fake("scenarios/slice.jsonl"));
        std::env::set_var("CAD_STUDIO_RUNNING_PROCESSES", root.join("registry"));
        std::env::set_var("CAD_FAKE_ARGV_OUT", dump);
        std::env::set_var("CAD_FAKE_COPY_FROM", fixture_golden());
        std::env::remove_var("CAD_FAKE_STDERR");
    }
}

/// One message, bounded — never a fixed sleep.
async fn recv(messages: &mut UnboundedReceiver<BridgeMessage>, what: &str) -> BridgeMessage {
    match tokio::time::timeout(Duration::from_secs(5), messages.next()).await {
        Ok(Some(message)) => message,
        Ok(None) => panic!("message stream ended while waiting for {what}"),
        Err(_) => panic!("timed out waiting for {what}"),
    }
}

/// The slice's end state, as far as the assertions and the offscreen render
/// need it: the project root and the driven window.
struct SliceRun {
    root: PathBuf,
    events: Vec<EngineEvent>,
    window: ProjectWindow,
}

/// Drive the whole slice: project → window + bridge → one turn with every
/// message folded into the window (the app layer's mapping, minus the iced
/// runtime) → the watcher's kinds collected and forwarded as the window's
/// two relays would. Holds ENV_LOCK across the whole [env → turn] window.
async fn run_slice() -> SliceRun {
    let guard = ENV_LOCK.lock().await;

    // a real project through the engine's creator (upstream ProjectStore
    // .create): leaf directory + prepare's scaffolding
    let parent = std::env::temp_dir().join(format!("cad-slice-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&parent).unwrap();
    let root = store::create_project(&parent, "Slice").expect("create_project scaffolds");
    assert!(
        root.join("output").is_dir(),
        "output/ exists before any watch"
    );
    assert!(
        load_metadata(&root).is_ok(),
        "prepare wrote a project.json the store can load"
    );

    // the platform watch over output/ — the same engine call the window's
    // two watchers make, proving the fake's writes are observed as reloads
    let (_watch, reloads) = watch_directory(&root.join("output"), Duration::from_millis(300))
        .expect("output/ exists, the watch registers");

    let dump = root.join("argv-dump.jsonl");
    set_env(&root, &dump);
    let settings = Settings::default();
    let (mut window, _bridge_task) = ProjectWindow::new(root.clone(), &settings);
    // the app layer maps the bridge's stream into window messages; the
    // headless driver performs the same mapping by hand (a cosmic Task's
    // stream is private to the iced runtime)
    let (bridge, mut messages) = Bridge::launch(settings, root.clone());
    // the send gate refuses until python is verified (the m5 build
    // pipeline's guard); the app's startup check is the real transition and
    // is not this milestone's wiring — the seam m5 left "for T8's startup
    // check" stands in for it
    bridge.mark_python_ready_for_test();

    // the empty-project frame builds (the viewer pane's pure-construction
    // pattern, over the whole window)
    let _ = window.view();

    bridge.enqueue(PROMPT.to_string(), Vec::new());

    // one turn, every message folded into the window as it arrives
    let mut events = Vec::new();
    let outcome = loop {
        match recv(&mut messages, "the slice turn").await {
            BridgeMessage::Event(event) => {
                let finished = matches!(event, EngineEvent::TurnFinished(_));
                events.push(event.clone());
                let _ = window.update(Message::Bridge(BridgeMessage::Event(event)));
                if finished {
                    break match events.last() {
                        Some(EngineEvent::TurnFinished(outcome)) => outcome.clone(),
                        other => panic!("TurnFinished vanished: {other:?}"),
                    };
                }
            }
            BridgeMessage::Closed => {
                panic!("the slice turn closed without a terminal")
            }
            // a refused send announces itself as the sheet request — fail as
            // the refusal it is, not as a turn that never comes
            BridgeMessage::PythonSheetRequested => {
                panic!("the send was refused: python not verified (enqueue gate)")
            }
            other => {
                let _ = window.update(Message::Bridge(other));
            }
        }
    };
    assert_eq!(
        outcome,
        TurnOutcome::Success {
            session_id: Some(SLICE_SID.into()),
            text: REPORT.into(),
        },
        "{events:?}"
    );
    drop(bridge);

    // the watcher observed the fake's writes (bounded poll on the engine's
    // own channel): collect the whole burst — a quiet half-second after the
    // last emission, or the 5 s deadline, ends the collection. The copies
    // landed mid-turn, so the emissions are already due or land within the
    // debounce.
    let mut kinds = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let wanted = |kinds: &[ReloadKind]| {
        [
            ReloadKind::Mesh,
            ReloadKind::Plate,
            ReloadKind::Schematic,
            ReloadKind::Manifest,
        ]
        .iter()
        .all(|kind| kinds.contains(kind))
    };
    loop {
        match reloads.recv_timeout(Duration::from_millis(500)) {
            Ok(kind) => kinds.push(kind),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if wanted(&kinds) || tokio::time::Instant::now() >= deadline {
                    break;
                }
            }
        }
    }
    for kind in [
        ReloadKind::Mesh,
        ReloadKind::Plate,
        ReloadKind::Schematic,
        ReloadKind::Manifest,
    ] {
        assert!(
            kinds.contains(&kind),
            "watcher never saw {kind:?}: {kinds:?}"
        );
    }

    // forward the observed reloads as the window's two relays would: the
    // viewer pane's watch feeds Message::Viewer, the shell's feeds
    // Message::OutputReload — both over the same kinds
    let mut seen = Vec::new();
    for kind in &kinds {
        if seen.contains(kind) {
            continue;
        }
        seen.push(*kind);
        let _ = window.update(Message::Viewer(app::viewer_pane::Message::Reload(*kind)));
        let _ = window.update(Message::OutputReload(*kind));
    }

    drop(guard);
    SliceRun {
        root,
        events,
        window,
    }
}

/// The slice: prompt → agent → outputs → watcher → rebuilt panes.
#[tokio::test(flavor = "multi_thread")]
async fn prompt_to_rendered_model() {
    let SliceRun {
        root,
        events,
        window,
    } = run_slice().await;

    // ── the turn streamed every row kind ────────────────────────────────
    assert!(events.contains(&EngineEvent::ThinkingStarted), "{events:?}");
    assert!(
        events.contains(&EngineEvent::Thinking {
            estimated_tokens: 50
        }),
        "{events:?}"
    );
    assert!(
        events.contains(&EngineEvent::Thinking {
            estimated_tokens: 192
        }),
        "{events:?}"
    );
    assert!(
        events.contains(&EngineEvent::ThinkingFinished),
        "{events:?}"
    );
    for (id, name) in [(WRITE_ID, "Write"), (BASH_ID, "Bash")] {
        assert!(
            events.iter().any(|event| matches!(
                event,
                EngineEvent::ToolStarted { id: got, name: got_name, .. }
                    if got == id && got_name == name
            )),
            "no ToolStarted for {name}: {events:?}"
        );
        assert!(
            events.iter().any(|event| matches!(
                event,
                EngineEvent::ToolFinished { id: got, is_error: false, .. } if got == id
            )),
            "no successful ToolFinished for {name}: {events:?}"
        );
    }
    assert!(
        events.iter().any(|event| matches!(
            event,
            EngineEvent::ToolInput { id, .. } if id == BASH_ID
        )),
        "{events:?}"
    );
    assert!(
        events.contains(&EngineEvent::TextConfirmed {
            text: REPORT.into()
        }),
        "{events:?}"
    );

    // ── the outputs landed (the fake's copy) ────────────────────────────
    for name in [
        "model.3mf",
        "model.stl",
        "model.step",
        "plate.3mf",
        "schematic.svg",
        "schematic-2.svg",
        "schematic.png",
        "schematic.pdf",
        "manifest.json",
    ] {
        assert!(
            root.join("output").join(name).is_file(),
            "the fake never wrote output/{name}"
        );
    }
    assert!(
        root.join("model.py").is_file(),
        "the fake never wrote model.py (the warnings gate reads it)"
    );

    // ── the 3D pane: the scene rebuilt over the 2-part model ────────────
    assert_eq!(
        window.viewer.scene.scene.parts.len(),
        2,
        "the ModelScene rebuilt from output/model.3mf"
    );
    // the minimal status bar's warning count (missing_requirements)
    assert_eq!(window.viewer.warnings, 0, "a complete output set");
    // the header's missing-output warnings (missing_output_files)
    assert!(
        window.missing_outputs.is_empty(),
        "{:?}",
        window.missing_outputs
    );
    // the activity model's success classification ("Model generated" —
    // only when no output is missing at turn end)
    assert_eq!(
        window.activity,
        Activity::Succeeded("Model generated".into())
    );

    // ── the schematic pane: page tabs from the manifest ─────────────────
    let titles = window.schematic.tabs.titles();
    assert_eq!(
        titles,
        ["Assembly", "Lid"],
        "titled from the manifest drawings"
    );

    // ── the transcript rows in the window's chat state ──────────────────
    let rows = &window.chat.messages;
    assert_eq!(
        rows.iter()
            .find(|row| row.role == Role::User)
            .expect("the user row")
            .text,
        PROMPT
    );
    assert!(
        rows.iter()
            .any(|row| row.role == Role::Thinking && row.text.starts_with("Thought for")),
        "the thinking row finalized: {rows:?}"
    );
    for id in [WRITE_ID, BASH_ID] {
        let tool = rows
            .iter()
            .find(|row| row.tool.as_ref().is_some_and(|tool| tool.tool_use_id == id))
            .unwrap_or_else(|| panic!("the tool row for {id}: {rows:?}"))
            .tool
            .as_ref()
            .unwrap();
        assert_eq!(tool.state, ToolState::Succeeded);
        assert!(tool.output.as_deref().is_some_and(|o| !o.is_empty()));
        assert_eq!(tool.preview, None, "completed rows persist preview-less");
    }
    assert!(
        rows.iter()
            .any(|row| row.role == Role::Assistant && row.text == REPORT),
        "{rows:?}"
    );

    // ── the persisted chat.json (the driver's transcript) ───────────────
    let saved = load_chat(&root).expect("chat.json persisted at turn end");
    assert_eq!(
        saved.iter().filter(|row| row.role == Role::User).count(),
        1,
        "{saved:?}"
    );
    assert!(
        saved
            .iter()
            .any(|row| row.role == Role::Thinking && row.text.starts_with("Thought for")),
        "{saved:?}"
    );
    let saved_tools: Vec<_> = saved.iter().filter(|row| row.tool.is_some()).collect();
    assert_eq!(saved_tools.len(), 2, "{saved:?}");
    for row in saved_tools {
        let tool = row.tool.as_ref().unwrap();
        assert_eq!(tool.state, ToolState::Succeeded);
        assert!(tool.output.is_some());
        // the action field round-trips through the persisted schema (an
        // action-carrying row is failure-only; every success row is None)
        assert_eq!(row.action, None);
    }
    assert!(
        saved
            .iter()
            .any(|row| row.role == Role::Assistant && row.text == REPORT),
        "{saved:?}"
    );

    // ── the queue: drained, unpaused ────────────────────────────────────
    assert!(window.queue.is_empty());
    assert!(!window.queue_paused);

    // ── the window assembly: one frame builds in the full shape ────────
    let _ = window.view();
}

/// The offscreen render of the slice's final scene — Preston's screenshot.
/// Thelio-only (GPU surfaceless context; the laptop test policy): run with
/// `cargo test -p app --test vertical_slice -- --ignored` and read the PNG
/// path it prints. Renders the scene the pane actually staged (the same
/// `ModelScene` the reload path rebuilt) through the viewer's headless
/// harness — the render_headless.rs pattern.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "thelio-only: surfaceless GPU render, writes the screenshot PNG"]
async fn slice_offscreen_render() {
    use viewer::render::{Frame, MeshPipeline, Options, PartAppearance, PartPose, default_camera};

    let SliceRun { window, .. } = run_slice().await;
    let scene = window.viewer.scene.scene.clone();
    let overlay = window.viewer.scene.grid.clone();

    let (device, queue) = viewer::render::gpu::device_queue();
    let (w, h) = (1024_u32, 768_u32);
    let camera = default_camera(&scene, w as f32 / h as f32);
    let mut pipeline = MeshPipeline::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("slice screenshot target"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    // identity poses: the slice's final scene draws every part where it sits
    // (None would draw-skip — visibility, not identity); the 4x4 identity in
    // column-major [[f32; 4]; 4] form without a glam dependency here
    let poses = vec![
        Some([
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ]);
        scene.parts.len()
    ];
    // opaque appearances: the slice's assembled draw carries no motion flags
    let appearances = vec![PartAppearance::default(); scene.parts.len()];
    let frame = Frame {
        camera: &camera,
        scene: &scene,
        poses: &poses,
        appearances: &appearances,
        effective_aabb: (scene.aabb_min, scene.aabb_max),
        grid: Some(overlay.as_ref()),
        markers: None,
        ghost: None,
        options: Options::default(),
    };
    pipeline.prepare_parts(device, queue, (w, h), &frame);
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("slice screenshot clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.1,
                        g: 0.1,
                        b: 0.12,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            ..Default::default()
        });
        drop(pass);
    }
    pipeline.render(&mut encoder, &view, [0.0, 0.0, w as f32, h as f32]);
    let stride = (w * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("slice screenshot readback"),
        size: (stride * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(Some(encoder.finish()));
    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        tx.send(r).unwrap();
    });
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    rx.recv().unwrap().unwrap();
    let padded = slice.get_mapped_range().to_vec();
    readback.unmap();
    let px: Vec<u8> = padded
        .chunks(stride as usize)
        .take(h as usize)
        .flat_map(|row| row[..w as usize * 4].iter().copied())
        .collect();

    // the app background (0.1, 0.1, 0.12 clears read back sRGB-encoded as
    // ~(89, 89, 97)) — the screenshot must not be blank
    let ink = px
        .chunks(4)
        .filter(|c| {
            (c[0] as i32 - 89).abs() + (c[1] as i32 - 89).abs() + (c[2] as i32 - 97).abs() > 30
        })
        .count();
    assert!(ink > 5_000, "the render drew the model: {ink} ink px");

    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/m4-slice-render.png");
    image::RgbaImage::from_raw(w, h, px)
        .expect("the readback is exactly w*h*4")
        .save(&out)
        .expect("write the screenshot");
    println!("slice render: {} ({ink} ink px)", out.display());
}
