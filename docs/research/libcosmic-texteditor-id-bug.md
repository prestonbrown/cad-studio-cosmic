# Engineering record: libcosmic text_editor id-forwarding tree corruption
# (INTERNAL — pop-os/COSMIC rejects AI-assisted bug reports outright, so this
# is not filed upstream; Preston's ruling 2026-10-28. Kept as the workaround's
# rationale and repro evidence for whenever the pin moves or the bug resurfaces.)

## The claim
`cosmic::widget::text_editor` in context-menu mode (the default when
`.context_menu(true)` or unset on Linux) is a wrapper widget around the
inner iced `TextEditor`. The wrapper's `Widget::id()` forwards the inner
widget's id:

    src/widget/text_editor.rs:
        fn id(&self) -> Option<iced_core::widget::Id> {
            ew::<Message>(&self.inner).id()
        }

(verified present on master 2026-10-15; same in the dbc62f2 pin our app
uses). Consequence: when the editor carries a custom id (e.g.
`.id(Id::new("chat-input"))`), TWO adjacent tree levels report the SAME
custom id.

## Why that panics
iced's cached rebuild path (`UserInterface::build`) calls
`Tree::take_all_named`, which rips custom-id'd widget states into a map
keyed by id NAME ONLY. Two live nodes with the same name: the second
`HashMap::insert` silently drops the first (the wrapper's record — the
one carrying the inner editor's subtree). On re-diff, one level is
restored from the surviving record; the other takes the self-id path,
which swaps its just-ripped `None` state back WITHOUT rebuilding (its
tag still matches). A stateless shell survives to layout, and the inner
`TextEditor::layout` does `tree.state.downcast_mut()` → panic
"Downcast on stateless state" (iced tree.rs:505).

## Reproduction sketch (verify/adapt — must be your own example)
A minimal cosmic app: one window whose view is a
`cosmic::widget::text_editor(&content).id(Id::new("repro"))` (leave the
context menu at its default ON). Force a cached rebuild — e.g. any state
change that rebuilds the view (a timer tick updating unrelated text
works). Expect a panic on the second layout with the message above.
Adding `.context_menu(false)` makes the panic disappear (wrapper
collapsed, id lives on one level only).

Evidence from a real app hit by this (cad-studio port, pin dbc62f2):
 - backtrace: TextEditor::layout ← Row ← Column ← ... ← Popover ←
   iced_winit build_user_interface (panic at tree.rs:505)
 - instrumented iced (temporary [patch] to a local copy, since removed)
   logged take name="chat-input" TWICE per rebuild, then one restore
   with named_has=true and one with named_has=false immediately before
   the panic
 - first frame never panics (fresh Tree::new builds real state); only
   cached rebuilds do

## Suggested fix directions (for discussion, not a PR from us)
- stop forwarding `id()` from the wrapper when `has_context_menu`
  (report the wrapper's own id or None), or
- make the wrapper's state-tag/self-id path rebuild instead of
  restoring a ripped-None state

## Our workaround
`.context_menu(false)` on the affected editor (cost: no right-click menu
on that one editor).
