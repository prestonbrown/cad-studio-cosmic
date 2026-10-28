//! Minimal repro for the dynamic-menu tree corruption (fact pack:
//! docs/research/libcosmic-menu-dynamic-items-bug.md). NOT part of the app —
//! a scratch instrument; delete when the bug is filed and fixed.
//!
//! A menu bar whose sole root's items GROW every 300 ms.
//!   DYN_MENU_LEAVES=flat   → cosmic's own Item::Button leaves
//!   DYN_MENU_LEAVES=custom → hand-built button::custom(Column(Text, Text))
//!                            leaves (the version menu's shape)
//!   DYN_MENU_LEAVES=static → never grows (the control)
//! Run under a display (Xvfb :99 works); panic or survival prints to stderr.
use cosmic::app::{self, Core, Settings};
use cosmic::iced::Task;
use cosmic::iced::Length;
use cosmic::widget::{button, container, menu, text};
use cosmic::{Action, Element};

struct Demo {
    core: Core,
    items: usize,
    leaves: Leaves,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Leaves {
    Static,
    Flat,
    Custom,
    Mutate,
}

#[derive(Clone, Debug)]
enum Message {
    Grow,
    Nothing,
}

/// The flat leaves' action (menu items demand a MenuAction, not a raw
/// message — the app's Command pattern).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NothingAction;

impl cosmic::widget::menu::action::MenuAction for NothingAction {
    type Message = Message;

    fn message(&self) -> Message {
        Message::Nothing
    }
}

fn grow_later() -> Task<Message> {
    // the grow tick rides a subscription-free timer task
    Task::perform(
        async { tokio::time::sleep(std::time::Duration::from_millis(300)).await },
        move |()| Message::Grow,
    )
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
        let leaves = match std::env::var("DYN_MENU_LEAVES").as_deref() {
            Ok("flat") => Leaves::Flat,
            Ok("custom") => Leaves::Custom,
            Ok("mutate") => Leaves::Mutate,
            _ => Leaves::Static,
        };
        eprintln!("dynamic-menu repro: leaves={leaves:?}");
        let first = if leaves == Leaves::Static {
            Task::none()
        } else {
            Task::perform(
                async { tokio::time::sleep(std::time::Duration::from_millis(2000)).await },
                move |()| Message::Grow,
            )
        };
        (Self { core, items: 2, leaves }, first.map(Action::App))
    }

    fn update(&mut self, message: Message) -> Task<Action<Message>> {
        match message {
            Message::Nothing => Task::none(),
            Message::Grow => {
                if self.leaves == Leaves::Static {
                    return Task::none();
                }
                self.items += 1;
                eprintln!("items={}", self.items);
                grow_later().map(Action::App)
            }
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let children: Vec<menu::Tree<Message>> = match self.leaves {
            Leaves::Static | Leaves::Flat => menu::items(
                &std::collections::HashMap::new(),
                (0..self.items)
                    .map(|i| menu::Item::Button(format!("Item {i}"), None, NothingAction))
                    .collect::<Vec<_>>(),
            ),
            Leaves::Custom | Leaves::Mutate => (0..self.items)
                .map(|i| {
                    // Mutate: every EXISTING item's subtitle changes each
                    // rebuild (the version menu's live per-frame time-ago)
                    let churn = match self.leaves {
                        Leaves::Mutate => format!("· {} ms ago", std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH).unwrap().subsec_millis()),
                        _ => "· just now · Jan 1, 2027 at 1:00 PM".to_string(),
                    };
                    let body = cosmic::widget::Column::new()
                        .spacing(2)
                        .push(text(format!("Item {i} head")))
                        .push(text(format!("Item {i} {churn}")).size(11))
                        .padding([6, 16]);
                    let leaf: Element<'_, Message> = button::custom(body)
                        .class(cosmic::theme::Button::MenuItem)
                        .on_press(Message::Nothing)
                        .width(Length::Fill)
                        .into();
                    menu::Tree::new(leaf)
                })
                .collect(),
        };
        let root: Element<'_, Message> = container(text("Menu")).padding([4, 10]).into();
        let tree = menu::Tree::with_children(root, children).width(340);
        let bar = menu::bar(vec![tree])
            .item_width(menu::ItemWidth::Static(340))
            .item_height(menu::ItemHeight::Dynamic(30));
        container(bar).padding(20).into()
    }
}

fn main() -> cosmic::iced::Result {
    let settings = Settings::default();
    app::run::<Demo>(settings, ())
}
