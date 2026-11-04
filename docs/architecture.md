# Architecture

CAD Studio is one process and a Cargo workspace of three crates. The engine
has no GUI. The viewer draws on the GPU. The app is the libcosmic front end and
the only crate that knows about windows.

## Crates

| Crate | Role | Boundary |
|---|---|---|
| [engine/](../engine/src/lib.rs) | Domain logic: agent sessions, Python environment, project folders, parsing, settings, file watching. | No GUI dependencies. Headless-testable. |
| [viewer/](../viewer/src/lib.rs) | The 3D renderer and the pure math around it: camera, lights, section, measure picking, parts arrangement, animation playback. | wgpu only. iced types appear only in the renderer adapter module. |
| [app/](../app/src/lib.rs) | The COSMIC application: windows, panes, sheets, menus, localization, the engine bridge. | Depends on both. Nothing depends on it. |

The engine modules: [agent/](../engine/src/agent/mod.rs) (locating the CLI,
argv and prompt assembly, stream-event decoding, the turn state machine),
[python/](../engine/src/python/env.rs) (venv install and check, the
`model.py` runner), [project/](../engine/src/project/folder.rs) (folder layout,
manifest, versions, layouts, recents),
[parsing/](../engine/src/parsing/mod.rs) (STL, 3MF, animation),
[watch.rs](../engine/src/watch.rs), [processes.rs](../engine/src/processes.rs)
and [settings.rs](../engine/src/settings.rs).

## Turn lifecycle

1. The user sends a prompt in the chat pane. The app hands it to the
   [bridge](../app/src/bridge.rs).
2. The bridge checks the guards (not busy, Python ready) and queues the send.
   A send while a turn runs is queued first-in first-out. A failed or
   cancelled turn pauses the queue.
3. A driver task owns the engine
   [`Session`](../engine/src/agent/session.rs). It spawns the agent CLI,
   resuming or forking the stored session id, and reads its stream-json output.
4. The engine decodes each line into an engine event
   ([stream_event.rs](../engine/src/agent/stream_event.rs)) and sends it over a
   bounded channel.
5. The bridge translates events into app messages. The app folds them into the
   [chat state](../app/src/chat.rs) that the [chat view](../app/src/chat_view.rs)
   draws. The driver also folds them into a transcript and saves `chat.json`
   at each turn end, so the chat survives with no window open.
6. The agent writes files under `output/`. The [watcher](../engine/src/watch.rs)
   reloads the viewer and drawing panes, debounced. After a successful turn
   the engine records a version.

Builds from the toolbar and from parameter sliders skip the agent: the bridge
runs `model.py` directly with the venv's Python and records a version when the
script changed.

## Project folder

```
<project>/
  model.py                  the agent-written build script
  references/               turn attachments, named in the prompt
  output/                   model.3mf, model.stl, plate.3mf, manifest.json,
                            animation.json, schematic SVGs
  .cadstudio/
    project.json            project metadata
    chat.json               the transcript
    logs/                   one log per agent turn
    versions/<n>/           model.py, output/, chat.json, version.json
```

[engine/src/project/folder.rs](../engine/src/project/folder.rs) is the one
place that spells these paths. The formats match upstream, so a project folder
moves between the macOS app and this one.

## Settings

One file, `~/.config/cad-studio/settings.json`, owned by the engine's
[Settings](../engine/src/settings.rs) type. One writer per key: every
reader loads the file fresh and every writer goes through
[SettingsStore::update](../app/src/settings_store.rs), so no window holds a
stale copy. The settings window maps each field message to one mutation in a
single fold ([windows/settings.rs](../app/src/windows/settings.rs)). A file
that exists but cannot be read is never written over.

COSMIC config holds window and UI state only. It never holds a setting that
the engine reads.

## UI kit

[app/src/ui/](../app/src/ui/mod.rs) is the shared view kit: tone (text and
glyph colors by role), surface (cards, capsules, radii by COSMIC roundness
class), icons, buttons, form sections, pickers, segmented controls, sliders,
code blocks and the empty-pane placeholder. The kit decides colors, radii and
enablement from the theme. Each view lays itself out. Only what two views
share lives here. Text sizes come from
[typography.rs](../app/src/typography.rs).

User-visible strings go through the `fl!` macro with entries in the four
catalogs under [app/i18n/](../app/i18n). The language is chosen once at
startup from the persisted override or the desktop request.

## Drive harness

With `CAD_STUDIO_DRIVE` naming a script, the app streams the script's commands
into its update loop as the same messages a real click would produce. This
replaces synthetic input, which COSMIC gates per connection. The command list
is in [app/src/drive.rs](../app/src/drive.rs). The rig's capture scripts
([scripts/rig/](../scripts/rig/README.md)) use it to photograph every state in
light and dark.
