//! Windowed spike: the golden fixture model in a window through the
//! viewer's own Depth24PlusStencil8 pass. Mirrors iced's `custom_shader`
//! example app shape; drag orbits and scroll zooms through the parity
//! input formulas (facts #18-#20: 0.008 rad/pt, pitch clamp 1.5607,
//! precise deltas / 12, zoom clamps [0.1r, 200r] on the distance); the
//! checkboxes toggle wireframe mode (facts #44) and the parity grid
//! overlay, and `m` flips msaa 1x <-> 4x — an internal pipeline rebuild,
//! since iced's Storage owns one pipeline per type. Pan exists as the
//! scene formula but stays unwired until M4. Run from the repo root:
//! `cargo run -p viewer --example spike_window`

use std::sync::Arc;

use engine::parsing::threemf::parse_3mf;
use iced::keyboard;
use iced::mouse::ScrollDelta;
use iced::widget::{center, checkbox, column, mouse_area, row, shader, text};
use iced::{Center, Element, Fill, Point, Size, Subscription};

use viewer::render::{ModelScene, NEUTRAL, PartScene};

fn main() -> iced::Result {
    iced::application(Spike::default, Spike::update, Spike::view)
        .title("CAD viewer spike")
        .window_size(Size::new(900.0, 700.0))
        .subscription(Spike::subscription)
        .run()
}

#[derive(Debug, Clone)]
enum Message {
    Pressed,
    Released,
    Moved(Point),
    Zoom(ScrollDelta),
    Wireframe(bool),
    ShowGrid(bool),
    ToggleMsaa,
}

struct Spike {
    scene: ModelScene,
    dragging: bool,
    last: Option<Point>,
}

impl Default for Spike {
    fn default() -> Self {
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../fixtures/golden/model.3mf"
        ))
        .expect("golden fixture model.3mf");
        let parts = parse_3mf(&bytes).expect("golden model.3mf parses");
        Self {
            scene: ModelScene::new(Arc::new(PartScene::from_parts(&parts, NEUTRAL))),
            dragging: false,
            last: None,
        }
    }
}

impl Spike {
    fn update(&mut self, message: Message) {
        match message {
            Message::Pressed => {
                self.dragging = true;
                self.last = None;
            }
            Message::Released => {
                self.dragging = false;
                self.last = None;
            }
            Message::Moved(p) => {
                if self.dragging {
                    if let Some(last) = self.last {
                        let (dx, dy) = (p.x - last.x, p.y - last.y);
                        // iced's cursor y grows downward; the parity
                        // formulas speak AppKit's bottom-up deltas (facts
                        // #18: drag up increases pitch) — flip dy at the
                        // boundary, dx passes through
                        self.scene.orbit(dx, -dy);
                    }
                    self.last = Some(p);
                }
            }
            Message::Zoom(delta) => {
                // facts #20: precise (trackpad) deltas count 12 px per zoom
                // step; notched-wheel line deltas are steps already. iced's
                // Pixels variant is the precise one. A zero vertical delta
                // falls back to horizontal, as upstream's scrollWheel does.
                let (x, y, precise) = match delta {
                    ScrollDelta::Lines { x, y } => (x, y, false),
                    ScrollDelta::Pixels { x, y } => (x, y, true),
                };
                let d = if y != 0.0 { y } else { x };
                let steps = if precise { d / 12.0 } else { d };
                self.scene.zoom(0.9f32.powf(steps));
            }
            Message::Wireframe(on) => self.scene.wireframe = on,
            Message::ShowGrid(on) => self.scene.show_grid = on,
            Message::ToggleMsaa => {
                self.scene.msaa = if self.scene.msaa == 1 { 4 } else { 1 };
            }
        }
    }

    /// `m` flips msaa — plain keys only, so a modifier combo stays free for
    /// M4's pan wiring. listen() delivers ignored events, keeping real text
    /// input keystrokes out.
    fn subscription(&self) -> Subscription<Message> {
        fn hotkey(event: keyboard::Event) -> Option<Message> {
            let keyboard::Event::KeyPressed {
                modified_key,
                modifiers,
                ..
            } = event
            else {
                return None;
            };
            if !modifiers.is_empty() {
                return None;
            }
            match modified_key.as_ref() {
                keyboard::Key::Character("m") => Some(Message::ToggleMsaa),
                _ => None,
            }
        }
        keyboard::listen().filter_map(hotkey)
    }

    fn view(&self) -> Element<'_, Message> {
        let canvas = mouse_area(shader(&self.scene).width(Fill).height(Fill))
            .on_press(Message::Pressed)
            .on_release(Message::Released)
            .on_move(Message::Moved)
            .on_scroll(Message::Zoom);

        let toggles = row![
            checkbox(self.scene.wireframe)
                .label("wireframe")
                .on_toggle(Message::Wireframe),
            checkbox(self.scene.show_grid)
                .label("grid")
                .on_toggle(Message::ShowGrid),
        ]
        .spacing(12);

        center(
            column![
                canvas,
                toggles,
                text(format!(
                    "drag: orbit · scroll: zoom · m: msaa ({}×)",
                    self.scene.msaa
                ))
                .size(12)
            ]
            .spacing(4)
            .align_x(Center),
        )
        .padding(8)
        .into()
    }
}
