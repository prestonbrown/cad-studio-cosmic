//! Windowed spike: the golden fixture model in a window through the
//! viewer's own Depth24PlusStencil8 pass. Mirrors iced's `custom_shader`
//! example app shape; drag orbits, scroll zooms, the checkboxes toggle
//! wireframe mode (facts #44) and the parity grid overlay. Run from the
//! repo root: `cargo run -p viewer --example spike_window`

use std::sync::Arc;

use engine::parsing::threemf::parse_3mf;
use iced::mouse::ScrollDelta;
use iced::widget::{center, checkbox, column, mouse_area, row, shader, text};
use iced::{Center, Element, Fill, Point, Size};

use viewer::render::{NEUTRAL, PartScene, SpikeScene};

fn main() -> iced::Result {
    iced::application(Spike::default, Spike::update, Spike::view)
        .title("CAD viewer spike")
        .window_size(Size::new(900.0, 700.0))
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
}

struct Spike {
    scene: SpikeScene,
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
            scene: SpikeScene::new(Arc::new(PartScene::from_parts(&parts, NEUTRAL))),
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
                        self.scene.yaw -= dx * 0.01;
                        self.scene.pitch = (self.scene.pitch + dy * 0.01).clamp(-1.4, 1.4);
                    }
                    self.last = Some(p);
                }
            }
            Message::Zoom(delta) => {
                let steps = match delta {
                    ScrollDelta::Lines { y, .. } => y,
                    ScrollDelta::Pixels { y, .. } => y / 40.0,
                };
                self.scene.zoom = (self.scene.zoom * 0.9f32.powf(steps)).clamp(0.05, 50.0);
            }
            Message::Wireframe(on) => self.scene.wireframe = on,
            Message::ShowGrid(on) => self.scene.show_grid = on,
        }
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
            column![canvas, toggles, text("drag: orbit · scroll: zoom").size(12)]
                .spacing(4)
                .align_x(Center),
        )
        .padding(8)
        .into()
    }
}
