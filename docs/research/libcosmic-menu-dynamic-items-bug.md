# Engineering record: libcosmic menu overlay panics ("Downcast widget state")
# (INTERNAL — not filed upstream: pop-os/COSMIC rejects AI-assisted bug reports
# outright, Preston's ruling 2026-10-28. Kept as the boundary the app's menus
# obey, the probes that pin it, and the evidence trail if the pin ever moves.)

## Boundary (pinned 2026-11-04, M9 Task 5)

The "dynamic items corrupt the tree" reading below was wrong. Item churn
is not the trigger; two separate libcosmic defects at the dbc62f2 pin
are, and each has an exact boundary.

**Defect 1: every root opens against root 0's item trees.**
`init_root_menu` (src/widget/menu/menu_inner.rs:1227-1292) finds the
root under the cursor (root `i`) and builds its `MenuBounds` with
`&mut state.tree.children[0].children` (:1276), root 0's flat item trees,
whatever `i` is (`init_root_popup_menu`, :1348, carries the same line
under a TODO). `get_children_layout` (:1748-1806) then lays out each item
of root `i` whose height is `Length::Shrink` against `tree[mt.index]`
from root 0. An item whose widget shape differs from root 0's item at the
same flat index downcasts the wrong state: "Downcast widget state"
(tree.rs:507, a type MISMATCH: `downcast_mut`'s `expect` on a `Some` of
another type; `State::None` panics "Downcast on stateless state",
tree.rs:505) or an index panic when root `i` has more flat items than
root 0. Items with a fixed or fill height never reach that `layout` call
(the `Length::Fixed` arm, :1777), and `Item::Button`,
`Item::ButtonDisabled`, `Item::CheckBox` and `Item::Folder` are all
`menu_button`s, `Length::Fixed(36.0)` (menu_tree.rs:140). Every
later layout, draw and hover indexes the correct root
(`tree_children[data.active_root[0]]`, :491; :897; :1408). The version
menu's leaves were `button::custom(Column(Text, Text))` with shrink
height in a root after the first, so its first open with an entry
panicked; a one-root bare app opens root 0 against its own trees, which
is why it never reproduced.

**Defect 2: the open menu's bounds are frozen at open time.**
`MenuState::menu_bounds` (child sizes and positions) is computed once,
when the root opens. Items added while open stay invisible until the
menu reopens; an item REMOVED while open makes `MenuState::layout` slice
`menu_tree[start_index..=end_index]` past the shorter list (:331):
"range end index 7 out of range for slice of length 7".

Tree reconciliation itself is sound: `MenuBar::diff` runs
`menu_roots_diff` (menu_bar.rs:120-161), a positional `diff_children`
over each root's flattened items (`set_index` and `flattern` walk in the
same order, menu_tree.rs:89-130), and `Tree::diff` rebuilds any child
whose tag changed (iced/core/src/widget/tree.rs:158-252).

### Probes (app/examples/dynamic_menu.rs, rig, Xvfb, GL backend)

The probed root is opened with a real X click and an item hovered BEFORE
the cycles start; captures at about cycle 20 and 180 show the menu open
with the changed content. 200 rebuilds 75 ms apart, `ItemHeight::Dynamic`
(the app's setting), a fixed eight-`Item::Button` root beside it.

| Probe | Root | Result | Why |
|---|---|---|---|
| (a) labels: fixed `Item::Button` set, every label's text changes (Mostra/Nascondi parametri, a counter, a growing string) | second | SURVIVED 200 | same tags, positional diff; fixed height skips defect 1 |
| (b) checkmark: fixed `Item::CheckBox` set, the ✓ moves among five rates and Ripeti toggles | second | SURVIVED 200 | the check slot swaps icon ↔ space, a tag change `Tree::diff` rebuilds; fixed height |
| (b') same, plus the root's own label changes (the current speed) | first, second | SURVIVED 200 each | root text re-lays its paragraph; same widget shape |
| (c1) `Item::Button` set grows 2 → 8 while open | second | SURVIVED 200, new rows hidden until reopen | defect 2 |
| (c2) set shrinks while open (opened at 8) | second | PANIC at the first removal, menu_inner.rs:331 | defect 2 |
| (c3) shrink-height `button::custom` leaves, fixed set | second | PANIC on the open click, before any cycle (`init_root_menu` → `MenuBounds::new` → `get_children_layout`) | defect 1 |
| (c3') same leaves, label churn every cycle | first | SURVIVED 200 | root 0 opens against its own trees |
| control: nothing changes | second | SURVIVED 200 | |

### The rules this pins

1. Fixed-height kit items (`Item::Button`, `ButtonDisabled`,
   `CheckBox`, `Folder`, `Divider`) may change label text, enabled state
   and check state between rebuilds in any root. Root labels may change
   text.
2. A root's item SET (count and order) is fixed while the app runs, or
   at least never shrinks while that menu can be open; growing is safe
   but invisible until reopened.
3. Custom shrink-height leaves only in root 0 of their bar, or give them
   a fixed height (`Length::Fixed`), which skips the aliasing path.

The version history stays a popover: it grows and shrinks with the
project and its leaves are custom.

## The original claim (superseded by the boundary above)

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

## Re-probe (2026-10-29, M8 T12): NOT FIXED at upstream HEAD

libcosmic 95236a9 (upstream HEAD as of 2026-10-27; iced at both pins wants
wgpu 28.0, so the pin constraint binds nothing): the exact panic —
"Downcast widget state", tree.rs:507, the menu_inner init_root_menu →
get_children_layout stack — reproduced after a jar-project turn recorded a
version and the version menu root was clicked. The probe had to restore
the pre-workaround menu root (16ea45e's arrangement) in a throwaway
worktree to expose the surface; the popover UI cannot test the bug at all.
The popover workaround and this watch carry to M9. The M4 pinch watch also
re-probed: NO PinchGesture arm anywhere in the bumped rev's iced/winit
(conversion.rs has no gesture arm; winit 0.31.0-beta.2). Evidence:
thelio:/tmp/m8t12-evidence-run.log; report in this repo's SDD workspace.

## Evidence preserved

- /tmp/crash-repro-jar on the reporting machine: the exact project state
  (93-row agent session; the crash fires when its second version lands).
- The driven repro: `CAD_STUDIO_DRIVE` script (app/src/drive.rs's `chat`
  command) sends the turn; RUST_BACKTRACE=1 in the launcher env.
