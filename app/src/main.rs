use cosmic::app::{self, Core, Settings};
use cosmic::iced::Length;
use cosmic::widget::text;
use cosmic::{Action, Application, Element, Task};

struct CadStudio {
    core: Core,
}

#[derive(Clone, Debug)]
enum Message {}

impl Application for CadStudio {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;

    const APP_ID: &'static str = "net.brownhouse.CadStudio";

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: ()) -> (Self, Task<Action<Message>>) {
        (Self { core }, Task::none())
    }

    fn update(&mut self, _message: Message) -> Task<Action<Message>> {
        Task::none()
    }

    fn view(&self) -> Element<Message> {
        cosmic::widget::column::with_capacity(2)
            .push(cosmic::widget::header_bar().title("CAD Studio"))
            .push(text("CAD Studio for COSMIC — M0 bootstrap"))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

fn main() -> cosmic::iced::Result {
    cad_studio_main()
}

#[allow(non_snake_case)]
fn cad_studio_main() -> cosmic::iced::Result {
    app::run::<CadStudio>(Settings::default(), ())
}
