//! Windowed spike: the golden fixture model in a window through the
//! viewer's own Depth24PlusStencil8 pass. Mirrors iced's `custom_shader`
//! example app shape; drag orbits and scroll zooms through the same
//! [`CameraInput`] the app pane drives (facts #18-#20); the checkboxes
//! toggle wireframe mode (facts #44) and the parity grid overlay, and `m`
//! flips msaa 1x <-> 4x — an internal pipeline rebuild, since iced's
//! Storage owns one pipeline per type. Pan is not wired here: it scales by
//! the viewport height, which the app pane reports from layout. Run from
//! the repo root:
//! `cargo run -p viewer --example spike_window`

use std::sync::Arc;

use engine::parsing::threemf::parse_3mf;
use iced::keyboard;
use iced::mouse::ScrollDelta;
use iced::widget::{center, checkbox, column, mouse_area, row, shader, text};
use iced::{Center, Element, Fill, Point, Size, Subscription};

use viewer::camera_input::CameraInput;
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
    input: CameraInput,
}

impl Default for Spike {
    fn default() -> Self {
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../fixtures/golden/model.3mf"
        ))
        .expect("golden fixture model.3mf");
        let parts = parse_3mf(&bytes).expect("golden model.3mf parses");
        let scene = Arc::new(PartScene::from_parts(&parts, NEUTRAL));
        let input = CameraInput::new(scene.aabb());
        let mut scene = ModelScene::new(scene);
        scene.orbit = input.orbit;
        Self { scene, input }
    }
}

impl Spike {
    fn update(&mut self, message: Message) {
        match message {
            Message::Pressed => self.input.press(false),
            Message::Released => {
                self.input.release();
            }
            Message::Moved(p) => self.input.move_to(p.x, p.y),
            Message::Zoom(delta) => {
                let (x, y, precise) = match delta {
                    ScrollDelta::Lines { x, y } => (x, y, false),
                    ScrollDelta::Pixels { x, y } => (x, y, true),
                };
                self.input.scroll(x, y, precise, false);
            }
            Message::Wireframe(on) => self.scene.wireframe = on,
            Message::ShowGrid(on) => self.scene.show_grid = on,
            Message::ToggleMsaa => {
                self.scene.msaa = if self.scene.msaa == 1 { 4 } else { 1 };
            }
        }
        self.scene.orbit = self.input.orbit;
    }

    /// `m` flips msaa — plain keys only. listen() delivers ignored events,
    /// keeping real text input keystrokes out.
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
