# Engineering record: libcosmic menu overlay corrupts widget state when item sets change
# (INTERNAL — not filed upstream: pop-os/COSMIC rejects AI-assisted bug reports
# outright, Preston's ruling 2026-10-28. Kept as the workaround's rationale,
# the repro boundary (a bare app does NOT reproduce — the corruption needs our
# full window context), and the evidence trail if the pin ever moves.)

## The claim

A `cosmic::widget::menu` whose ITEMS change between view rebuilds (a
dynamic dropdown — entries appended, labels changing) corrupts the cached
widget tree: item `Text` widgets end up with stateless shells where their
paragraph state belongs, and the next menu overlay layout panics:

```
thread 'main' panicked at iced/core/src/widget/tree.rs:507:38:
Downcast widget state
```

Observed on the dbc62f2 pin. The corruption does NOT need the menu to be
open: a turn of the host app that appends a menu item mid-session is
enough; the panic then fires on the next event that runs the overlay's
layout (a later user OPEN of the menu hits it immediately).

## The panic path (RUST_BACKTRACE=1, abridged; full trace captured
## 2026-10-28 from a driven repro, key frames)

```
iced_core::widget::tree::State::downcast_mut            tree.rs:507
iced_core::widget::text::Text<..>::layout               text.rs:357
iced_widget::container::Container<..>::layout           container.rs:277/491
cosmic::widget::menu::menu_inner::get_children_layout    menu_inner.rs:1781
cosmic::widget::wrapper::RcWrapper<T>::with_data_mut     wrapper.rs:59
cosmic::widget::menu::menu_inner::get_children_layout    menu_inner.rs:1776/1803
cosmic::widget::menu::menu_inner::MenuBounds::new        menu_inner.rs:263
cosmic::widget::menu::menu_inner::init_root_menu         menu_inner.rs:1227/1266
cosmic::widget::menu::menu_inner::Menu<..>::update       menu_inner.rs:633/946
iced_core::overlay::group::Group<..>::update (×6)
iced_core::overlay::nested::Nested<..>::update
iced_runtime::user_interface::UserInterface<..>::update  user_interface.rs:234
iced_winit::run_instance
```

The `Text` inside a menu item container downcasts `tree.state` during
`layout` and finds `None` where its paragraph cache belongs.

## Reproduction shape (verify/adapt in a minimal app)

- A `menu::bar` with one root whose children are built from a Vec that
  GROWS while the app runs (e.g. a timer that pushes an item every few
  seconds, with a per-item label that embeds the current time).
- Static-item roots do NOT reproduce — the corruption tracks item-set
  changes between cached rebuilds. (In the reporting app, three static
  menus ran all day; the one dynamic menu — a version-history dropdown
  whose entries append per completed unit of work — panicked the process
  the first time an entry landed, menu never opened.)
- Trigger: any subsequent event dispatch through the overlay group (a
  user click that would open the menu, or any event that reaches the
  menu's update path). Panic as above.

## Suspect mechanism (for discussion)

`menu_inner` builds its item overlays through `RcWrapper`
(`with_data_mut`, wrapper.rs:59) each rebuild; when the item vector
changes length or identity between cached rebuilds, the overlay's tree
re-diff (iced's positional diff through the wrapper) leaves a Text
child's state detached — the same state-preservation gap family as the
`text_editor` id-forwarding bug filed separately (a widget subtree whose
state iced's name-keyed/positional rescue cannot reattach). Fix
directions: give menu items stable identity in the tree diff, or rebuild
(rather than restore) item subtrees whose element identity changed.

## Our workaround

The dynamic dropdown is no longer a cosmic menu: the button opens a
popover card of plain widgets (the host app already used popovers for
issue lists — those never corrupt). The menu bar keeps only static
roots. Cost: none functionally; the popover is anchored and styled like
the menu it replaces.

## Evidence preserved

- /tmp/crash-repro-jar on the reporting machine: the exact project state
  (93-row agent session; the crash fires when its second version lands).
- The driven repro: `CAD_STUDIO_DRIVE` script (app/src/drive.rs's `chat`
  command) sends the turn; RUST_BACKTRACE=1 in the launcher env.
