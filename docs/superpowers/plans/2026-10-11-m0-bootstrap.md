# CAD Studio COSMIC Port — M0 Bootstrap Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up the workspace, thelio build loop, golden fixtures, and prove the two rendering spikes (own depth+stencil `Primitive::render()` pass; MSAA resolve) so M1+ builds on settled ground.

**Architecture:** Three-crate workspace (`engine`, `viewer`, `app`) pinned to libcosmic git rev `dbc62f2` / pop-os iced fork / wgpu 28. All builds and tests run on thelio over ssh; the Mac only edits.

**Tech Stack:** Rust edition 2024 (rustc ≥1.93), libcosmic (git pin), wgpu 28 (exact major match), build123d/matplotlib/numpy via uv venv, resvg, quick-xml/zip.

**Spec:** `docs/superpowers/specs/2026-10-11-cad-studio-cosmic-port-design.md` (as amended by 6413f86). Behavioral source of truth: `docs/research/upstream-contract.md`. Toolkit research: `docs/research/libcosmic-landscape.md`.

**Sibling plans:** This is plan 1 of 10 (M0…M9). Each milestone gets its own plan written against the tree as it exists when the previous milestone lands.

## Global Constraints

- Builds/tests run on thelio only: `ssh thelio 'bash -lc "cd ~/Code/cad-studio-cosmic && <cmd>"'` (non-interactive ssh misses `~/.local/bin`).
- libcosmic pinned to `rev = "dbc62f2d899cdbfbe43d170689ddc0290db663f4"`. wgpu must be `28` (major-exact match with the iced fork).
- The viewer renders via `Primitive::render()` (NOT `draw()`): viewer-owned render pass, own `Depth24PlusStencil8` texture (resized with viewport, stencil cleared per frame). MSAA: own sample count + resolve target (iced's pass MSAA does not apply). Both per commit 6413f86.
- engine crate: no GUI dependencies (no libcosmic, no iced) — ever.
- Tests never touch the network and never require the venv (fixtures are committed).
- GPU-first rendering: per-frame work 100% GPU; CPU only one-off event-driven work off-thread.
- Conventional commits (`feat:`, `test:`, `build:`, `docs:` …); stage explicit file paths, never `git add -A`/`-u`/`.`.
- Before any heavy thelio build phase: `ListAgents`, announce to overlapping busy sessions, size jobs to what's free (32 cores shared).
- Upstream reference clone lives on the Mac at `/tmp/cad-studio-src` (volatile; re-clone from https://github.com/filippofinke/cad-studio if missing).

## Review Focus

Input classes / failure modes M0 must pin down, with the owning task's test:

1. **3MF with components + build-item transforms** (not just flat objects) silently parsing wrong → Task 5 handmade `component.3mf`; assert applied transform shifts the parsed bbox.
2. **ASCII STL misread as binary** by the `size == 84 + 50*n` heuristic → Task 5/6: `is_binary_stl()` predicate unit test + handmade `ascii.stl` fixture.
3. **Stencil/depth state leaking across frames** (ghost cap on frame 2) → Task 7: render the same scene twice, assert frames are byte-identical.
4. **Viewport resize breaks viewer-owned depth/stencil textures** → Task 7: render 800×600 then 1024×768, assert success and aspect-correct ink bounds.
5. **resvg silently dropping matplotlib SVG content** (text/paths) → Task 4: ink-fraction of resvg render vs matplotlib's own PNG within a 4× band.

---

### Task 1: Workspace scaffold, thelio remotes, justfile, reference clones

**Files:**
- Create: `Cargo.toml`, `engine/Cargo.toml`, `engine/src/lib.rs`, `viewer/Cargo.toml`, `viewer/src/lib.rs`, `app/Cargo.toml`, `app/src/main.rs`, `.gitignore`, `justfile`
- Reference clones (thelio, not in repo): `~/Code/_ref/libcosmic`, `~/Code/_ref/iced`, `~/Code/_ref/cosmic-app-template`

**Interfaces:**
- Consumes: nothing (first task).
- Produces: workspace building green on thelio; git remotes `thelio` (Mac) and `origin` (thelio clone); crates `engine` (lib), `viewer` (lib), `app` (bin `cad-studio`); `just {build,test,run,render-spike}` recipes.

- [ ] **Step 1: Create workspace files**

`Cargo.toml`:

```toml
[workspace]
resolver = "3"
members = ["engine", "viewer", "app"]

[workspace.package]
edition = "2024"
version = "0.1.0"
license = "MIT"

[workspace.dependencies]
libcosmic = { git = "https://github.com/pop-os/libcosmic.git", rev = "dbc62f2d899cdbfbe43d170689ddc0290db663f4", features = ["wgpu"] }
wgpu = "28"
bytemuck = { version = "1", features = ["derive"] }
```

`engine/Cargo.toml`:

```toml
[package]
name = "engine"
description = "CAD Studio domain engine: agent sessions, project store, parsing, watching"
edition.workspace = true
version.workspace = true
license.workspace = true
```

`engine/src/lib.rs`:

```rust
//! Domain engine for CAD Studio. No GUI dependencies, ever (spec §4.2).
```

`viewer/Cargo.toml`:

```toml
[package]
name = "viewer"
description = "CAD Studio 3D viewer: camera math, tools, wgpu renderer (iced adapter only)"
edition.workspace = true
version.workspace = true
license.workspace = true

[dependencies]
wgpu.workspace = true
bytemuck.workspace = true
```

`viewer/src/lib.rs`:

```rust
//! 3D viewer crate. wgpu 28 (exact major match with libcosmic's iced fork).
//! iced types are allowed only in the renderer adapter module (spec §4.3).
```

`app/Cargo.toml`:

```toml
[package]
name = "app"
description = "CAD Studio for COSMIC"
edition.workspace = true
version.workspace = true
license.workspace = true

[[bin]]
name = "cad-studio"
path = "src/main.rs"

[dependencies]
libcosmic.workspace = true
```

`app/src/main.rs` (cross-check against `~/Code/_ref/cosmic-app-template/src/app.rs` on thelio; fix any trait-signature drift until it compiles):

```rust
use cosmic::app::{self, Core};
use cosmic::iced::Length;
use cosmic::widget::{column, text};
use cosmic::{Application, Element, Settings, Task};

struct CadStudio {
    core: Core,
}

#[derive(Clone, Debug)]
enum Message {}

impl Application for CadStudio {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;

    fn core(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: ()) -> (Self, Task<Message>) {
        (Self { core }, Task::none())
    }

    fn update(&mut self, _message: Message) -> Task<Message> {
        Task::none()
    }

    fn view(&self) -> Element<Message> {
        column()
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
```

(If `app::run` needs different args or the template wires `localize()` first, follow the template — the check is a green build, not this exact snippet.)

`.gitignore`:

```
/target
```

(Cargo.lock IS committed — the libcosmic git pin and wgpu major must be reproducible.)

`justfile`:

```make
default: build

build:
    cargo build --workspace

test:
    cargo test --workspace

run:
    cargo run -p app

render-spike:
    cargo run -p viewer --example spike_window

headless:
    cargo test -p viewer --test render_headless
```

- [ ] **Step 2: Create thelio remotes and working clone**

```bash
git branch -M main
ssh thelio 'bash -lc "git init --bare ~/Code/cad-studio-cosmic.git"'
git remote add thelio thelio:Code/cad-studio-cosmic.git
git push -u thelio main
ssh thelio 'bash -lc "git clone ~/Code/cad-studio-cosmic.git ~/Code/cad-studio-cosmic && git -C ~/Code/cad-studio-cosmic remote rename origin upstream"'
```

- [ ] **Step 3: Reference clones on thelio (source of truth for trait signatures)**

```bash
ssh thelio 'bash -lc "mkdir -p ~/Code/_ref && git clone https://github.com/pop-os/libcosmic.git ~/Code/_ref/libcosmic && git -C ~/Code/_ref/libcosmic checkout dbc62f2d899cdbfbe43d170689ddc0290db663f4 && git -C ~/Code/_ref/libcosmic submodule update --init && git clone --depth 1 https://github.com/pop-os/cosmic-app-template ~/Code/_ref/cosmic-app-template"'
ssh thelio 'bash -lc "git -C ~/Code/_ref/libcosmic submodule status"'   # record the iced rev in the task report
```

The submodule status line gives the exact iced rev libcosmic vendors — later tasks read real signatures from `~/Code/_ref/libcosmic/iced/`.

- [ ] **Step 4: Toolchain packages on thelio (needs Preston's sudo once)**

```bash
ssh thelio 'bash -lc "dpkg -s build-essential pkg-config libfontconfig1-dev >/dev/null 2>&1 && echo TOOLCHAIN-PKGS-OK || echo MISSING"'
```

If MISSING: `ssh -t thelio 'sudo apt install -y build-essential pkg-config libfontconfig1-dev'` (Preston types the password — ask them to run `! ssh -t thelio 'sudo apt install -y build-essential pkg-config libfontconfig1-dev'`).

- [ ] **Step 5: First build on thelio (announce first — heavy)**

`ListAgents`; message any busy overlapping session; then:

```bash
git add Cargo.toml engine/Cargo.toml engine/src/lib.rs viewer/Cargo.toml viewer/src/lib.rs app/Cargo.toml app/src/main.rs .gitignore justfile
git commit -m "build: workspace scaffold (engine/viewer/app) pinned to libcosmic dbc62f2"
git push thelio main
ssh thelio 'bash -lc "cd ~/Code/cad-studio-cosmic && git pull -q && cargo build --workspace 2>&1 | tail -5"'
git add Cargo.lock && git commit -m "build: lockfile from first thelio build" && git push thelio main
```

Expected: green build (first build pulls ~700 crates; several minutes even on 32 cores).

**KNOWN SNAG:** if cargo errors that a path dependency `iced` (or `iced_*`) is missing inside the libcosmic checkout (cargo did not fetch its submodule), pin the iced rev recorded in Step 3 and patch in the workspace root `Cargo.toml`:

```toml
[patch."https://github.com/pop-os/libcosmic"]
# not needed unless the submodule fetch failed; rev comes from Step 3's `git submodule status`
```

using the appropriate `[patch.'https://github.com/pop-os/iced']` mappings for `iced`, `iced_core`, `iced_wgpu`, `iced_winit`, `iced_tiny_skia`, `iced_futures`, `iced_runtime`, `iced_style`, `iced_widget` pointing at `git = "https://github.com/pop-os/iced.git", rev = "<recorded rev>"`. Record in the commit message whether the patch was needed.

- [ ] **Step 6: Smoke-run the window (Preston verifies)**

Preston, from a terminal inside the COSMIC session on thelio:

```bash
cd ~/Code/cad-studio-cosmic && cargo run -p app
```

Expected: a "CAD Studio — M0 bootstrap" window with a cosmic header bar. (No automated check possible without a display session; this is a human gate. If Preston is unavailable, defer this step and proceed — the headless harness in Task 7 is the automated gate.)

- [ ] **Step 7: Commit any build fixes, push**

```bash
git push thelio main
```

---

### Task 2: Golden fixtures generated by real build123d on thelio

**Files:**
- Create: `fixtures/generator/model.py`
- Create (generated, committed): `fixtures/golden/{model.3mf,model.stl,model.step,plate.3mf,schematic.svg,schematic-2.svg,schematic.pdf,schematic.png,manifest.json,animation.json}`

**Interfaces:**
- Consumes: nothing.
- Produces: `fixtures/golden/*` — every later parser/renderer test parses these exact files; the venv at `~/.local/share/cad-studio/venv` (the app's future env path, so this doubles as an env-creation dry run).

- [ ] **Step 1: Write the generator**

`fixtures/generator/model.py` (best-effort against current build123d; the acceptance is the OUTPUTS — if the installed API differs, fix per `python -c "import build123d; help(build123d.Mesher)"` etc. and note deviations in the commit message):

```python
"""M0 golden fixture generator: two-part colored model, plate, drawing, manifest, animation.

Run: ~/.local/share/cad-studio/venv/bin/python fixtures/generator/model.py  (from repo root)
"""
import json
import math
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.backends.backend_pdf import PdfPages

from build123d import Box, Color, Mesher, Pos, Rot, export_3mf, export_step, export_stl

OUT = Path(__file__).resolve().parent.parent / "golden"
OUT.mkdir(parents=True, exist_ok=True)

WALL = 2.0    # wall thickness (mm)
HEIGHT = 8.0  # base height (mm)
LID_H = 10.0  # lid height (mm)

base = Box(60, 60, HEIGHT)
base.label = "base"
base.color = Color((0.12, 0.47, 0.71))

lid = Pos(0, 0, HEIGHT + LID_H / 2 + 4) * Box(40, 40, LID_H)
lid.label = "lid"
lid.color = Color((0.84, 0.15, 0.16))

mesher = Mesher()
mesher.add_shape([base, lid])
export_3mf(mesher, OUT / "model.3mf")
export_stl(base + lid, OUT / "model.stl")
export_step([base, lid], OUT / "model.step")

plate = Mesher()
plate.add_shape([base, Pos(0, 45, LID_H / 2) * Rot(0, 0, 0) * Box(40, 40, LID_H)])
plate.shapes[0].label = "base"  # keep labels/colors if the API exposes them; drop if not
export_3mf(plate, OUT / "plate.3mf")

# --- drawing (fixture quality: shapes + dims, not full ISO) ---
def page(ax, title, w, d, h):
    ax.add_patch(plt.Rectangle((-w / 2, -h / 2), w, h, fill=False, lw=0.5))
    ax.annotate(f"{w} x {d} x {h}", (0, 0), ha="center", va="center", fontsize=6)
    ax.set_title(title, fontsize=8)
    ax.set_xlim(-50, 50); ax.set_ylim(-50, 50); ax.set_aspect("equal")
    ax.axis("off")

fig1, ax1 = plt.subplots(figsize=(11.69, 8.27))
page(ax1, "fixture-box — assembly", 60, 60, HEIGHT + LID_H)
fig1.savefig(OUT / "schematic.svg")
fig1.savefig(OUT / "schematic.png", dpi=300)
fig2, ax2 = plt.subplots(figsize=(11.69, 8.27))
page(ax2, "fixture-box — lid", 40, 40, LID_H)
fig2.savefig(OUT / "schematic-2.svg")
with PdfPages(OUT / "schematic.pdf") as pdf:
    pdf.savefig(fig1); pdf.savefig(fig2)

manifest = {
    "name": "fixture-box",
    "description": "M0 golden fixture: two-part box",
    "units": "mm",
    "bounding_box": {"x": 60.0, "y": 60.0, "z": float(HEIGHT + LID_H + 4 + LID_H / 2 + LID_H / 2)},
    "volume_mm3": round(float(base.volume + lid.volume), 1),
    "surface_area_mm2": round(float(base.area + lid.area), 1),
    "parameters": {"WALL": WALL, "HEIGHT": HEIGHT, "LID_H": LID_H},
    "iteration": 1,
    "warnings": ["fixture warning"],
    "parts": [
        {"name": "base", "quantity": 1, "color": "#1f77b4"},
        {"name": "lid", "quantity": 1, "color": "#d62728"},
    ],
    "drawings": [
        {"file": "schematic.svg", "title": "Assembly"},
        {"file": "schematic-2.svg", "title": "Lid"},
    ],
    "bed": [220.0, 220.0],
}
(OUT / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")

animation = {
    "title": "lid lift",
    "frames": [
        {"t": 0.0, "label": "closed", "parts": {"lid": {"translate": [0, 0, 0], "rotate": [0, 0, 0, 1]}},
         "values": {"Lift (mm)": 0.0}, "collisions": []},
        {"t": 1.0, "label": "open", "parts": {"lid": {"translate": [0, 0, 10.0], "rotate": [0, 0, 0, 1]}},
         "values": {"Lift (mm)": 10.0}, "collisions": [{"parts": ["base", "lid"], "volume": 0.5}]},
        {"t": 2.0, "label": "open", "parts": {"lid": {"translate": [0, 0, 10.0], "rotate": [0, 0, 0, 1]}},
         "values": {"Lift (mm)": 10.0}, "collisions": []},
    ],
}
(OUT / "animation.json").write_text(json.dumps(animation, indent=2) + "\n")
print("fixtures written to", OUT)
```

- [ ] **Step 2: Create the venv and run (thelio)**

```bash
ssh thelio 'bash -lc "uv venv --python 3.12 ~/.local/share/cad-studio/venv && uv pip install --python ~/.local/share/cad-studio/venv/bin/python build123d matplotlib numpy"'
ssh thelio 'bash -lc "cd ~/Code/cad-studio-cosmic && ~/.local/share/cad-studio/venv/bin/python fixtures/generator/model.py"'
```

Expected: `fixtures written to ...`. Iterate on API errors until it runs (upstream's system prompt documents the same APIs — `Mesher().add_shape([...])`, `export_3mf`, `part.label`, `part.color`).

- [ ] **Step 3: Verify outputs (thelio)**

Write `fixtures/generator/check.py`:

```python
#!/usr/bin/env python3
"""Golden-fixture sanity check: schemas, sizes, 3MF structure."""
import json
import sys
import zipfile
from pathlib import Path

golden = Path(__file__).resolve().parent.parent / "golden"

m = json.loads((golden / "manifest.json").read_text())
assert m["parameters"]["WALL"] == 2.0
assert {p["name"] for p in m["parts"]} == {"base", "lid"}
assert m["bed"] == [220.0, 220.0]

a = json.loads((golden / "animation.json").read_text())
assert len(a["frames"]) == 3
assert a["frames"][0]["parts"]["lid"]["translate"] == [0.0, 0.0, 0.0]

for name in ["model.3mf", "model.stl", "model.step", "plate.3mf", "schematic.svg",
             "schematic-2.svg", "schematic.pdf", "schematic.png"]:
    p = golden / name
    assert p.is_file() and p.stat().st_size > 200, f"{name} missing or tiny"

with zipfile.ZipFile(golden / "model.3mf") as z:
    xml = z.read("3D/3dmodel.model").decode()
n_objects = xml.count("<object ")
print(f"manifest+animation OK; 3MF objects: {n_objects}")
if n_objects < 2:
    sys.exit("3MF must contain >=2 objects for the parser spike")
```

Run:

```bash
ssh thelio 'bash -lc "cd ~/Code/cad-studio-cosmic && ~/.local/share/cad-studio/venv/bin/python fixtures/generator/check.py && unzip -p fixtures/golden/model.3mf 3D/3dmodel.model | head -c 2000"'
```

Expected: `manifest+animation OK; 3MF objects: 2` and XML showing object names/attributes (record whether `name=` labels survived export — Task 5 needs to know; if names are missing, note it in the commit message and rely on `component.3mf` for the named case).

```bash
ssh thelio 'bash -lc "cd ~/Code/cad-studio-cosmic/fixtures/golden && unzip -l model.3mf && unzip -p model.3mf 3D/3dmodel.model | head -c 2000"'
```

Expected: `3D/3dmodel.model` present; XML contains two `<object>` elements with `name="base"`/`name="lid"` (or unnamed objects — record which, Task 5 needs to know).

- [ ] **Step 4: Commit fixtures (from Mac after pulling, or directly on thelio clone — pick one tree, keep it consistent)**

```bash
git add fixtures/generator/model.py fixtures/generator/check.py fixtures/golden/model.3mf fixtures/golden/model.stl fixtures/golden/model.step fixtures/golden/plate.3mf fixtures/golden/schematic.svg fixtures/golden/schematic-2.svg fixtures/golden/schematic.pdf fixtures/golden/schematic.png fixtures/golden/manifest.json fixtures/golden/animation.json
git commit -m "test: golden fixtures generated with build123d (two-part colored box, plate, drawing, manifest, animation)"
git push thelio main
```

---

### Task 3: Vendor upstream resources + xcstrings→fluent extractor

**Files:**
- Create: `fixtures/upstream/Localizable.xcstrings`, `fixtures/upstream/CADSystemPrompt.md` (vendored copies)
- Create: `scripts/extract_xcstrings.py`
- Create (generated, committed): `app/i18n/{en,it,de,fr}/main.ftl`

**Interfaces:**
- Consumes: upstream repo at `/tmp/cad-studio-src` (or re-clone).
- Produces: `app/i18n/<lang>/main.ftl` with every upstream UI key; M8 activates them via libcosmic's `fl!`. Source language of record becomes `en`.

- [ ] **Step 1: Vendor the inputs**

```bash
mkdir -p fixtures/upstream
cp /tmp/cad-studio-src/CADStudio/Resources/Localizable.xcstrings fixtures/upstream/
cp /tmp/cad-studio-src/CADStudio/Resources/CADSystemPrompt.md fixtures/upstream/
# if /tmp is gone: git clone --depth 1 https://github.com/filippofinke/cad-studio /tmp/cad-studio-src
```

- [ ] **Step 2: Write the extractor**

`scripts/extract_xcstrings.py`:

```python
#!/usr/bin/env python3
"""Convert an Xcode String Catalog to fluent .ftl files.

Usage: python3 scripts/extract_xcstrings.py fixtures/upstream/Localizable.xcstrings app/i18n
Writes <out>/<lang>/main.ftl for every localization present. Keys are sanitized to
[a-z0-9-]; collisions get a -2 suffix. A mapping report goes to stdout.
"""
import json
import re
import sys
from pathlib import Path


def ftl_key(raw: str, used: set[str]) -> str:
    k = re.sub(r"[^a-zA-Z0-9]+", "-", raw).strip("-").lower() or "key"
    base, n = k, 2
    while k in used:
        k = f"{base}-{n}"
        n += 1
    used.add(k)
    return k


def esc(v: str) -> str:
    return v.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n")


def main(src: str, out: str) -> None:
    data = json.loads(Path(src).read_text())
    strings = data["strings"]
    langs: dict[str, list[tuple[str, str]]] = {}
    for raw, entry in sorted(strings.items()):
        for lang, loc in entry.get("localizations", {}).items():
            unit = loc.get("stringUnit") or {}
            value = unit.get("value")
            if value is None:
                continue
            langs.setdefault(lang, []).append((raw, value))
    outp = Path(out)
    for lang, pairs in sorted(langs.items()):
        used: set[str] = set()
        d = outp / lang
        d.mkdir(parents=True, exist_ok=True)
        lines = [f"# Generated from {src} — do not edit by hand; source of record is en."]
        for raw, value in pairs:
            lines.append(f"{ftl_key(raw, used)} = {esc(value)}")
        (d / "main.ftl").write_text("\n".join(lines) + "\n")
        print(f"{lang}: {len(pairs)} strings")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
```

- [ ] **Step 3: Run and verify**

```bash
python3 scripts/extract_xcstrings.py fixtures/upstream/Localizable.xcstrings app/i18n
```

Expected: four lines `de: NNN`, `en: NNN`, `fr: NNN`, `it: NNN` with the same NNN (upstream catalog is complete; ~336). Then:

```bash
for f in app/i18n/*/main.ftl; do echo "$f $(grep -c ' = ' $f)"; done
grep -m2 ' = ' app/i18n/en/main.ftl
grep -c ' = ' app/i18n/en/main.ftl && grep -c ' = ' app/i18n/it/main.ftl
```

Expected: en and it counts equal. (Placeholder syntax `%lld`/`%@` inside values is expected — fluent refinement happens at M8 activation; note the count of values containing `%` in the task report: `grep -c '%' app/i18n/en/main.ftl`.)

- [ ] **Step 4: Commit**

```bash
git add fixtures/upstream/Localizable.xcstrings fixtures/upstream/CADSystemPrompt.md scripts/extract_xcstrings.py app/i18n/en/main.ftl app/i18n/it/main.ftl app/i18n/de/main.ftl app/i18n/fr/main.ftl
git commit -m "build(i18n): vendor upstream strings/prompt and extract fluent catalogs (en source of record)"
git push thelio main
```

---

### Task 4: resvg-vs-matplotlib SVG fidelity check

**Files:**
- Create: `viewer/tests/svg_check.rs`
- Modify: `viewer/Cargo.toml` (dev-dependencies)

**Interfaces:**
- Consumes: `fixtures/golden/schematic.svg`, `fixtures/golden/schematic-2.svg`, `fixtures/golden/schematic.png`.
- Produces: proof (spec §10 M0) that resvg renders matplotlib SVG with full content; if it fails, the fallback decision (higher-DPI PNG display) escalates to Preston before M4.

- [ ] **Step 1: Add dev-deps**

```bash
cargo add -p viewer --dev resvg@0.45 image
```

(resvg 0.45 = the version the iced fork pins; keep them aligned.)

- [ ] **Step 2: Write the failing test**

`viewer/tests/svg_check.rs`:

```rust
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/golden").join(name)
}

fn render_ink_fraction(svg: &std::path::Path, width: u32) -> (u32, u32, f64) {
    let data = std::fs::read(svg).unwrap();
    let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default())
        .expect("resvg must parse the matplotlib SVG");
    let size = tree.size();
    let height = ((width as f32) * size.height() / size.width()).max(1.0) as u32;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height).unwrap();
    resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut pixmap.as_mut());
    let ink = pixmap
        .pixels()
        .iter()
        .filter(|p| p.red() < 240 || p.green() < 240 || p.blue() < 240)
        .count();
    (width, height, ink as f64 / (width as f64 * height as f64))
}

fn png_ink_fraction(png: &std::path::Path, width: u32) -> f64 {
    let img = image::open(png).unwrap().to_rgb8();
    let h = ((width as f64) * img.height() as f64 / img.width() as f64) as u32;
    let img = image::imageops::resize(&img, width, h, image::imageops::FilterType::Nearest);
    let ink = img.pixels().filter(|p| p.0[0] < 240 || p.0[1] < 240 || p.0[2] < 240).count();
    ink as f64 / (width as f64 * h as f64)
}

#[test]
fn matplotlib_svg_renders_with_comparable_ink() {
    let svg = fixture("schematic.svg");
    let png = fixture("schematic.png");
    let (w, h, svg_ink) = render_ink_fraction(&svg, 1600);
    assert!(h > 400, "page height {h} suspiciously small");
    assert!(svg_ink > 0.0005, "almost no ink rendered: {svg_ink}");
    let png_ink = png_ink_fraction(&png, 1600);
    let ratio = svg_ink / png_ink;
    assert!(
        (0.25..4.0).contains(&ratio),
        "resvg ink {svg_ink:.5} vs matplotlib png ink {png_ink:.5} (ratio {ratio:.2}) — resvg likely dropped content"
    );
}

#[test]
fn second_page_svg_parses() {
    let (_w, _h, ink) = render_ink_fraction(&fixture("schematic-2.svg"), 800);
    assert!(ink > 0.0005, "page 2 almost blank under resvg");
}
```

- [ ] **Step 3: Run on thelio, verify it fails first (if assertions are wrong) then passes**

```bash
git push thelio main
ssh thelio 'bash -lc "cd ~/Code/cad-studio-cosmic && cargo test -p viewer --test svg_check"'
```

Expected: 2 passed. If the ratio band fails, inspect (render both to files and compare ink visually via Preston) BEFORE loosening the band — a real resvg gap changes the M4 plan.

- [ ] **Step 4: Commit**

```bash
git add viewer/Cargo.toml viewer/Cargo.lock viewer/tests/svg_check.rs
git commit -m "test(viewer): resvg fidelity check against matplotlib SVG/PNG fixtures"
git push thelio main
```

---

### Task 5: 3MF parser spike + decision ADR

**Files:**
- Create: `engine/tests/threemf_spike.rs`, `engine/tests/parse3mf.rs`
- Create: `fixtures/handmade/component.3mf`, `fixtures/handmade/ascii.stl`
- Create: `docs/research/3mf-decision.md`

**Interfaces:**
- Consumes: `fixtures/golden/model.3mf`.
- Produces: decision recorded in the ADR + seed code for M1's `engine::parsing::threemf` (function names below are what M1 keeps): `is_binary_stl(len: u64, n_tri: u32) -> bool`, `parse_objects(xml: &str) -> Vec<RawPart>`, `resolve_parts(xml: &str) -> Vec<RawPart>`, `RawPart { name: String, positions: Vec<[f32; 3]>, colors: Vec<[u8; 3]> }`.

- [ ] **Step 1: Handmade edge-case fixtures**

`fixtures/handmade/ascii.stl` (single triangle; defeats the binary size check because `size != 84 + 50*n` for any n):

```
solid ascii
  facet normal 0 0 1
    outer loop
      vertex 0 0 0
      vertex 1 0 0
      vertex 0 1 0
    endloop
  endfacet
endsolid ascii
```

`fixtures/handmade/component.3mf` — build with python on any machine (committed result):

```bash
python3 - <<'EOF'
import zipfile
model = '''<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xml:lang="en-US" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
  <resources>
    <basematerials id="1"><base name="red" displaycolor="#FF0000FF"/></basematerials>
    <object id="2" name="cube" pid="1" pindex="0">
      <mesh><vertices>
        <vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="1" y="1" z="0"/><vertex x="0" y="1" z="0"/>
        <vertex x="0" y="0" z="1"/><vertex x="1" y="0" z="1"/><vertex x="1" y="1" z="1"/><vertex x="0" y="1" z="1"/>
      </vertices><triangles>
        <triangle v1="0" v2="2" v3="1"/><triangle v1="0" v2="3" v3="2"/>
        <triangle v1="4" v2="5" v3="6"/><triangle v1="4" v2="6" v3="7"/>
        <triangle v1="0" v2="1" v3="5"/><triangle v1="0" v2="5" v3="4"/>
        <triangle v1="1" v2="2" v3="6"/><triangle v1="1" v2="6" v3="5"/>
        <triangle v1="2" v2="3" v3="7"/><triangle v1="2" v2="7" v3="6"/>
        <triangle v1="3" v2="0" v3="4"/><triangle v1="3" v2="4" v3="7"/>
      </triangles></mesh>
    </object>
    <object id="3" name="assembly">
      <component objectid="2" transform="10 0 0 0  0 1 0 0  0 0 1 0  0 0 0 1"/>
    </object>
  </resources>
  <build><item objectid="3"/></build>
</model>'''
with zipfile.ZipFile("fixtures/handmade/component.3mf", "w") as z:
    z.writestr("[Content_Types].xml", '<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/></Types>')
    z.writestr("3D/3dmodel.model", model)
EOF
```

(The component's transform translates the cube by +10 in x — the spike must apply it.)

- [ ] **Step 2: STL predicate test (TDD)**

`engine/tests/threemf_spike.rs`, first just the predicate:

```rust
fn is_binary_stl(file_len: u64, header_tri_count: u32) -> bool {
    file_len == 84 + 50 * header_tri_count as u64
}

#[test]
fn stl_binary_detection() {
    let ascii = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/handmade/ascii.stl"),
    )
    .unwrap();
    // ascii file: first 80 bytes are "solid ...", count field would be garbage;
    // the size check must reject it for every plausible count
    let n = u32::from_le_bytes([ascii[80], ascii[81], ascii[82], ascii[83]]);
    assert!(!is_binary_stl(ascii.len() as u64, n));
    let golden = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/golden/model.stl"),
    )
    .unwrap();
    let n = u32::from_le_bytes([golden[80], golden[81], golden[82], golden[83]]);
    assert!(is_binary_stl(golden.len() as u64, n));
}
```

Run on thelio; expect PASS (predicate is trivial — this test exists to pin the contract, upstream-contract.md §3).

- [ ] **Step 3: threemf crate evaluation**

```bash
cargo add -p engine --dev threemf@0.8
ssh thelio 'bash -lc "ls ~/.cargo/registry/src/*/threemf-0.8*/src 2>/dev/null || (cd ~/Code/cad-studio-cosmic && cargo fetch && ls ~/.cargo/registry/src/*/threemf-0.8*/src)"'
```

Read the crate's lib.rs/public API and write into the ADR, with quoted evidence, whether it exposes: (a) build `<item>` transforms, (b) `<component>` recursion, (c) `basematerials`/`colorgroup` colors, (d) per-triangle `pid`/`p1`, (e) `unit` attribute. Then, in `engine/tests/threemf_spike.rs`, write whatever parsing the crate allows for `fixtures/golden/model.3mf` and record what comes out (object count, per-object triangle counts, colors — print with `--nocapture`).

- [ ] **Step 4: Custom parser test (TDD — this is the likely production path)**

`cargo add -p engine --dev zip quick-xml`. `engine/tests/parse3mf.rs`:

```rust
use quick_xml::events::Event;
use quick_xml::Reader;

#[derive(Debug, Clone, PartialEq)]
pub struct RawPart {
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    pub colors: Vec<[u8; 3]>, // per triangle
}

fn fixture(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures").join(rel)
}

/// Parses 3D/3dmodel.model XML into per-object mesh data. Objects with a mesh are
/// terminals; objects with components resolve recursively applying 3x4 transforms.
pub fn parse_objects(xml: &str) -> Vec<RawPart> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut objects: Vec<RawPart> = Vec::new();
    let mut cur: Option<RawPart> = None;
    let mut components: Vec<(usize, [f32; 12])> = Vec::new(); // (object index in `objects`, transform)
    let mut cur_vertex_line = [0.0f32; 3];
    let mut depth_in_object = 0usize;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = e.local_name();
                match name.as_ref() {
                    b"object" => {
                        let mut p = RawPart { name: String::new(), positions: Vec::new(), colors: Vec::new() };
                        for a in e.attributes().flatten() {
                            if a.key.local_name().as_ref() == b"name" {
                                p.name = String::from_utf8_lossy(&a.value).into_owned();
                            }
                        }
                        cur = Some(p);
                        depth_in_object = 1;
                    }
                    b"component" if cur.is_none() && depth_in_object > 0 => {
                        // object-level component: record (target objectid, transform); resolution below
                        let mut oid = 0usize;
                        let mut xf = [0.0f32; 12];
                        xf[0] = 1.0; xf[5] = 1.0; xf[10] = 1.0; // identity default (row-major 3x4)
                        for a in e.attributes().flatten() {
                            match a.key.local_name().as_ref() {
                                b"objectid" => oid = String::from_utf8_lossy(&a.value).parse().unwrap_or(0),
                                b"transform" => {
                                    let t: Vec<f32> = String::from_utf8_lossy(&a.value)
                                        .split_whitespace().map(|v| v.parse().unwrap()).collect();
                                    xf.copy_from_slice(&t);
                                }
                                _ => {}
                            }
                        }
                        components.push((oid, xf));
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) => {
                let name = e.local_name();
                match name.as_ref() {
                    b"vertex" => {
                        if let Some(p) = cur.as_mut() {
                            let mut xyz = [0.0f32; 3];
                            for a in e.attributes().flatten() {
                                match a.key.local_name().as_ref() {
                                    b"x" => xyz[0] = String::from_utf8_lossy(&a.value).parse().unwrap(),
                                    b"y" => xyz[1] = String::from_utf8_lossy(&a.value).parse().unwrap(),
                                    b"z" => xyz[2] = String::from_utf8_lossy(&a.value).parse().unwrap(),
                                    _ => {}
                                }
                            }
                            cur_vertex_line = xyz;
                        }
                    }
                    b"triangle" => {
                        if let Some(p) = cur.as_mut() {
                            let (mut v1, mut v2, mut v3) = (0usize, 0usize, 0usize);
                            for a in e.attributes().flatten() {
                                match a.key.local_name().as_ref() {
                                    b"v1" => v1 = String::from_utf8_lossy(&a.value).parse().unwrap(),
                                    b"v2" => v2 = String::from_utf8_lossy(&a.value).parse().unwrap(),
                                    b"v3" => v3 = String::from_utf8_lossy(&a.value).parse().unwrap(),
                                    _ => {}
                                }
                            }
                            p.positions.push(cur_vertex_line); // placeholder: real impl pushes all three
                            let _ = (v1, v2, v3);
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                if e.local_name().as_ref() == b"object" {
                    if let Some(p) = cur.take() {
                        objects.push(p);
                    }
                    depth_in_object = 0;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => panic!("xml error {e}"),
            _ => {}
        }
    }
    // NOTE(spike): vertex→triangle index resolution, color plucking (pid/p1), and
    // component/transform application are intentionally incomplete here — the spike
    // proves the API shape; M1 productizes. Tests below only assert what this level
    // already guarantees.
    objects
}

fn read_model_xml(path: &std::path::Path) -> String {
    let f = std::fs::File::open(path).unwrap();
    let mut z = zip::ZipArchive::new(f).unwrap();
    let mut xml = String::new();
    z.by_name("3D/3dmodel.model").unwrap().read_to_string(&mut xml).unwrap();
    xml
}

#[test]
fn golden_model_has_two_objects_with_names() {
    let xml = read_model_xml(&fixture("golden/model.3mf"));
    let parts = parse_objects(&xml);
    assert!(parts.len() >= 2, "expected >=2 objects, got {}", parts.len());
    let names: Vec<&str> = parts.iter().map(|p| p.name.as_str()).collect();
    println!("objects: {names:?}");
}

#[test]
fn component_transform_is_applied() {
    let xml = read_model_xml(&fixture("handmade/component.3mf"));
    let parts = resolve_parts(&xml);
    // build item → "assembly" → component(cube, translate +10 x): the resolved
    // part's bbox must reach x ≈ 11 (cube spans 0..1, shifted +10)
    assert_eq!(parts.len(), 1, "one resolved build part");
    let maxx = parts[0].positions.iter().map(|p| p[0]).fold(f32::MIN, f32::max);
    assert!(maxx > 10.5, "transform not applied: max x = {maxx}");
}

/// Spike-level build resolution: build item → object → (mesh | first component),
/// applying the row-major 3x4 transform (p' = M·[p,1]). Single-level component
/// nesting — M1 generalizes (recursion, depth cap 16, full pid/p1 colors per
/// upstream-contract §3). Vertex positions only; triangle indexing not needed
/// for the bbox guarantee this test pins.
fn resolve_parts(xml: &str) -> Vec<RawPart> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    const IDENT: [f32; 12] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];
    let parse_xf = |s: Option<String>| -> [f32; 12] {
        match s {
            Some(s) => s
                .split_whitespace()
                .map(|x| x.parse().unwrap())
                .collect::<Vec<f32>>()
                .try_into()
                .unwrap(),
            None => IDENT,
        }
    };
    let attr = |e: &quick_xml::events::BytesStart<'_>, key: &[u8]| -> Option<String> {
        e.attributes()
            .flatten()
            .find(|a| a.key.local_name().as_ref() == key)
            .map(|a| String::from_utf8_lossy(&a.value).into_owned())
    };
    let mut meshes: Vec<(u32, RawPart)> = Vec::new();
    let mut components: Vec<(u32, u32, [f32; 12])> = Vec::new(); // (owner id, target id, xf)
    let mut build: Vec<(u32, [f32; 12])> = Vec::new();
    let (mut cur_id, mut cur_name, mut cur_pos, mut cur_mesh) = (0u32, String::new(), Vec::new(), false);
    let flush = |meshes: &mut Vec<(u32, RawPart)>, id: u32, name: &str, pos: Vec<[f32; 3]>, has: bool| {
        if has {
            meshes.push((id, RawPart { name: name.to_string(), positions: pos, colors: Vec::new() }));
        }
    };
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => match e.local_name().as_ref() {
                b"object" => {
                    flush(&mut meshes, cur_id, &cur_name, std::mem::take(&mut cur_pos), cur_mesh);
                    cur_id = attr(&e, b"id").and_then(|v| v.parse().ok()).unwrap_or(0);
                    cur_name = attr(&e, b"name").unwrap_or_default();
                    cur_mesh = false;
                }
                b"mesh" => cur_mesh = true,
                b"vertex" => {
                    cur_pos.push([
                        attr(&e, b"x").and_then(|v| v.parse().ok()).unwrap_or(0.0),
                        attr(&e, b"y").and_then(|v| v.parse().ok()).unwrap_or(0.0),
                        attr(&e, b"z").and_then(|v| v.parse().ok()).unwrap_or(0.0),
                    ]);
                }
                b"component" => {
                    components.push((
                        cur_id,
                        attr(&e, b"objectid").and_then(|v| v.parse().ok()).unwrap_or(0),
                        parse_xf(attr(&e, b"transform")),
                    ));
                }
                b"item" => {
                    build.push((
                        attr(&e, b"objectid").and_then(|v| v.parse().ok()).unwrap_or(0),
                        parse_xf(attr(&e, b"transform")),
                    ));
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(e) => panic!("xml error {e}"),
            _ => {}
        }
    }
    flush(&mut meshes, cur_id, &cur_name, cur_pos, cur_mesh);
    let apply = |p: [f32; 3], m: &[f32; 12]| -> [f32; 3] {
        [
            m[0] * p[0] + m[1] * p[1] + m[2] * p[2] + m[3],
            m[4] * p[0] + m[5] * p[1] + m[6] * p[2] + m[7],
            m[8] * p[0] + m[9] * p[1] + m[10] * p[2] + m[11],
        ]
    };
    let mut out = Vec::new();
    for (oid, bxf) in build {
        if let Some((_, part)) = meshes.iter().find(|(id, _)| *id == oid) {
            let mut part = part.clone();
            for p in part.positions.iter_mut() {
                *p = apply(*p, &bxf);
            }
            out.push(part);
        } else {
            for (owner, target, cxf) in &components {
                if *owner != oid {
                    continue;
                }
                if let Some((_, part)) = meshes.iter().find(|(id, _)| id == target) {
                    let mut part = part.clone();
                    for p in part.positions.iter_mut() {
                        // component transform first, then build item transform;
                        // general matrix composition is M1 work (translate-only here)
                        *p = apply(apply(*p, cxf), &bxf);
                    }
                    out.push(part);
                }
            }
        }
    }
    out
}
```

(The `use std::io::Read;` import is required for `read_to_string` — add it. Triangle indexing and color plucking (pid/p1) stay spike-incomplete on purpose; M1 does those with full upstream-contract §3 semantics. What this task proves: zip+quick-xml reads our files, the event-loop shape works, build/component resolution with transforms actually functions — Review Focus 1 — and the LoC estimate in the ADR is honest.)

Run on thelio: `cargo test -p engine`. Expect the three tests above to pass.

- [ ] **Step 5: Write the ADR**

`docs/research/3mf-decision.md`:

```markdown
# ADR: 3MF parsing — threemf crate vs custom zip+quick-xml

Date: 2026-10-11 (filled in by Task 5 executor) · Status: decided

Criteria (spec §4.2): build-item transforms, component recursion, color groups
(basematerials/colorgroup), per-triangle pid/p1, unit attribute.

## threemf 0.8 findings
(paste quoted evidence from reading the source: what it exposes, what it lacks)

## Custom parser estimate
(measured LoC from the spike ×, plus the delta list to full upstream-contract §3 semantics)

## Decision: <threemf | custom>
(1-3 sentences of why)
```

- [ ] **Step 6: Commit**

```bash
git add engine/Cargo.toml Cargo.lock engine/tests/threemf_spike.rs engine/tests/parse3mf.rs fixtures/handmade/component.3mf fixtures/handmade/ascii.stl docs/research/3mf-decision.md
git commit -m "test(engine): 3MF parser spike + STL binary predicate + decision ADR"
git push thelio main
```

---

### Task 6: Shader widget spike — windowed mesh via own depth pass (`Primitive::render()`)

**Files:**
- Create: `viewer/examples/spike_window.rs`, `viewer/src/render/mod.rs`, `viewer/src/render/mesh.wgsl`
- Modify: `viewer/Cargo.toml` (deps: `iced` — SAME source as libcosmic uses, `glam`, `bytemuck`)

**Interfaces:**
- Consumes: `fixtures/golden/model.stl` (binary STL; parse inline), wgpu 28.
- Produces: `viewer::render::{SpikeScene, MeshPipeline}` — M3 productizes into the real renderer; the iced dependency direction (`viewer` depends on iced, NOT libcosmic) and the `Primitive::render()` pattern established here carry forward. Cross-check every iced trait signature against `~/Code/_ref/libcosmic/iced/` (wgpu/src/primitive.rs, examples/custom_shader/) BEFORE writing code — the shapes below are from research and may drift; where they drift, the reference clone wins and the executor notes the delta in the commit.

- [ ] **Step 1: Wire deps**

In workspace `Cargo.toml` add the iced dependency pointing at the SAME rev libcosmic's submodule pins (recorded in Task 1 Step 3). If Task 1 needed `[patch]`, iced comes from the same patch; else:

```toml
iced = { git = "https://github.com/pop-os/iced.git", rev = "<submodule rev from Task 1>", features = ["advanced", "wgpu"] }
```

`cargo add -p viewer glam --features bytemuck` (workspace bytemuck already present).

- [ ] **Step 2: Write mesh.wgsl**

`viewer/src/render/mesh.wgsl`:

```wgsl
struct Camera {
    view_proj: mat4x4<f32>,
    section_axis: u32,
    section_pos: f32,
    section_enable: f32,
};
@group(0) @binding(0) var<uniform> camera: Camera;

struct Vin {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
};
struct Vout {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) world: vec3<f32>,
};

@vertex
fn vs_main(in: Vin) -> Vout {
    var out: Vout;
    out.clip = camera.view_proj * vec4<f32>(in.pos, 1.0);
    out.color = in.color;
    out.world = in.pos;
    return out;
}

@fragment
fn fs_main(in: Vout) -> @location(0) vec4<f32> {
    if (camera.section_enable > 0.5) {
        var v = in.world.y;
        if (camera.section_axis == 0u) { v = in.world.x; }
        if (camera.section_axis == 2u) { v = in.world.z; }
        if (v > camera.section_pos) { discard; }
    }
    // upstream lights (upstream-contract §5): key 2600 @ (60,120,90), fill 900 @ (-90,40,-60)
    let l1 = normalize(vec3<f32>(60.0, 120.0, 90.0));
    let l2 = normalize(vec3<f32>(-90.0, 40.0, -60.0));
    let n = normalize(cross(dpdx(in.world), dpdy(in.world))); // spike: screen-space normal
    let lit = 0.26 * max(dot(n, l1), 0.0) + 0.09 * max(dot(n, l2), 0.0) + 0.15;
    return vec4<f32>(in.color * clamp(lit, 0.0, 1.2), 1.0);
}
```

(The screen-space normal is spike shorthand — M3 uploads real normals from the mesh. If `dpdx/dpdy` behave badly with discard, fall back to `vec3(0., 1., 0.)` + unlit flat color; the spike's gate is geometry + camera + pass structure, not shading quality.)

- [ ] **Step 3: Write the renderer core**

`viewer/src/render/mod.rs` — mesh pipeline + own depth pass, no iced types in this file (pure wgpu, so the headless test in Task 7 can reuse it):

```rust
//! Spike renderer: mesh via viewer-owned render pass with Depth24PlusStencil8.
//! iced compositor pass carries no depth/stencil attachment (commit 6413f86) —
//! everything the viewer draws happens in `Primitive::render()`-driven passes
//! that this module owns.
use std::io::Read;
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Camera {
    pub view_proj: [[f32; 4]; 4],
    pub section_axis: u32,
    pub section_pos: f32,
    pub section_enable: f32,
    /// WGSL uniform structs round their size up to 16 bytes: 64 + 4 + 4 + 4 = 76
    /// must be padded to 80 or bind-group validation fails on min_binding_size.
    pub _pad: f32,
}

/// Default iso camera framing `mesh` (upstream presets: yaw π/4, pitch 0.6).
pub fn default_camera(mesh: &TriMesh) -> Camera {
    Camera {
        view_proj: default_view_proj(mesh),
        section_axis: 1,
        section_pos: f32::MAX,
        section_enable: 0.0,
        _pad: 0.0,
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 3],
}

pub struct TriMesh {
    pub vertices: Vec<Vertex>, // one per corner (unindexed, per-face — matches upstream's meshes)
    pub aabb_min: [f32; 3],
    pub aabb_max: [f32; 3],
}

/// Binary STL per upstream-contract §3 (call is_binary_stl-style check at the call site).
pub fn parse_binary_stl(bytes: &[u8]) -> TriMesh {
    let n = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as usize;
    let mut vertices = Vec::with_capacity(n * 3);
    let mut mn = [f32::MAX; 3];
    let mut mx = [f32::MIN; 3];
    for i in 0..n {
        let off = 84 + i * 50;
        let mut f = |k: usize| f32::from_le_bytes(bytes[off + 12 + k * 4..off + 16 + k * 4].try_into().unwrap());
        let _normal = [f(0), f(1), f(2)];
        let normal = [_normal[0], _normal[1], _normal[2]];
        for c in 0..3 {
            let p = [f(3 + c * 3), f(4 + c * 3), f(5 + c * 3)];
            for a in 0..3 {
                mn[a] = mn[a].min(p[a]);
                mx[a] = mx[a].max(p[a]);
            }
            vertices.push(Vertex { pos: p, normal, color: [0.62, 0.70, 0.80] }); // neutral color, upstream-contract §5
        }
    }
    TriMesh { vertices, aabb_min: mn, aabb_max: mx }
}

pub fn load_fixture_stl(path: &std::path::Path) -> TriMesh {
    let mut bytes = Vec::new();
    std::fs::File::open(path).unwrap().read_to_end(&mut bytes).unwrap();
    let n = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]);
    assert_eq!(bytes.len(), 84 + 50 * n as usize, "fixture must be binary STL");
    parse_binary_stl(&bytes)
}

pub struct MeshPipeline {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    pub depth: wgpu::TextureView,      // Depth24PlusStencil8, viewer-owned
    depth_tex: wgpu::Texture,
    uniform_buf: wgpu::Buffer,
    vertex_buf: wgpu::Buffer,
    pub vertex_count: u32,
    size: (u32, u32),
}

impl MeshPipeline {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, target_format: wgpu::TextureFormat, mesh: &TriMesh, size: (u32, u32), msaa: u32) -> Self {
        // msaa=1 for this task; Task 9 passes 4 and owns the resolve path.
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mesh.wgsl"),
            source: wgpu::ShaderModuleSource::Wgsl(include_str!("mesh.wgsl").into()),
        });
        let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("camera"),
            contents: bytemuck::bytes_of(&Camera {
                view_proj: glam::Mat4::IDENTITY.to_cols_array_2d(),
                section_axis: 1,
                section_pos: f32::MAX,
                section_enable: 0.0,
                _pad: 0.0,
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
            label: None,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform_buf.as_entire_binding() }],
            label: None,
        });
        let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let vlayout = [
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 12, shader_location: 1 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 24, shader_location: 2 },
        ];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh"),
            layout: Some(&device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                bind_group_layouts: &[&layout],
                push_constant_ranges: &[],
                label: None,
            })),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout { array_stride: 36, step_mode: wgpu::VertexStepMode::Vertex, attributes: &vlayout }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState { format: target_format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState { cull_mode: Some(wgpu::Face::Back), ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth24PlusStencil8,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState {
                    front: wgpu::StencilFaceState { compare: wgpu::CompareFunction::Always, fail_op: wgpu::StencilOperation::Keep, depth_fail_op: wgpu::StencilOperation::Keep, pass_op: wgpu::StencilOperation::IncrementClamp },
                    back: wgpu::StencilFaceState { compare: wgpu::CompareFunction::Always, fail_op: wgpu::StencilOperation::Keep, depth_fail_op: wgpu::StencilOperation::Keep, pass_op: wgpu::StencilOperation::DecrementClamp },
                    read_mask: 0xFF,
                    write_mask: 0xFF,
                },
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState { count: msaa, mask: !0, alpha_to_coverage_enabled: false },
            multiview: None,
            cache: None,
        });
        let (depth_tex, depth) = Self::make_depth(device, size, msaa);
        queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&default_camera(mesh)));
        Self { pipeline, bind_group, depth, depth_tex, uniform_buf, vertex_buf, vertex_count: mesh.vertices.len() as u32, size }
    }

    fn make_depth(device: &wgpu::Device, size: (u32, u32), msaa: u32) -> (wgpu::Texture, wgpu::TextureView) {
        let t = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("viewer depth+stencil"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: msaa,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth24PlusStencil8,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let v = t.create_view(&Default::default());
        (t, v)
    }

    /// Renders into `target` (the iced surface view) inside a viewer-owned pass.
    /// Color attachment: LoadOp::Load (iced content underneath stays); depth/stencil: cleared.
    pub fn render(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        if self.size != (target.size().width, target.size().height) {
            let (t, v) = Self::make_depth(device, (target.size().width, target.size().height), 1);
            self.depth_tex = t;
            self.depth = v;
            self.size = (target.size().width, target.size().height);
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("viewer mesh pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Store }),
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buf.slice(..));
        pass.draw(0..self.vertex_count, 0..1);
    }
}

/// Iso view per upstream presets (yaw π/4, pitch 0.6), framing the mesh aabb.
fn default_view_proj(mesh: &TriMesh) -> [[f32; 4]; 4] {
    let center = glam::Vec3::from_array([
        (mesh.aabb_min[0] + mesh.aabb_max[0]) / 2.0,
        (mesh.aabb_min[1] + mesh.aabb_max[1]) / 2.0,
        (mesh.aabb_min[2] + mesh.aabb_max[2]) / 2.0,
    ]);
    let radius = ((mesh.aabb_max[0] - mesh.aabb_min[0]).max(mesh.aabb_max[1] - mesh.aabb_min[1]).max(mesh.aabb_max[2] - mesh.aabb_min[2]) / 2.0).max(1.0);
    let fov = 35f32.to_radians();
    let distance = radius / (fov / 2.0).sin() * 1.15;
    let (yaw, pitch) = (std::f32::consts::FRAC_PI_4, 0.6f32);
    let eye = center + glam::Vec3::new(pitch.cos() * yaw.sin(), pitch.sin(), pitch.cos() * yaw.cos()) * distance;
    let view = glam::Mat4::look_at_rh(eye, center, glam::Vec3::Y);
    let proj = glam::Mat4::perspective_rh(fov, 16.0 / 9.0, 1.0, 100_000.0);
    (proj * view).to_cols_array_2d()
}
```

(API drift note: wgpu 28 render-pass/pipeline descriptors as above per wgpu 28 docs; if the fork pins a different wgpu patch with changed fields, follow `~/Code/_ref/libcosmic/iced/wgpu/Cargo.toml`'s wgpu version and its docs.)

- [ ] **Step 4: Write the windowed example**

`viewer/examples/spike_window.rs` — an iced (not libcosmic — keep the spike dependency-minimal) application whose view is `mouse_area(shader(ProgramImpl))`, with the shader `Primitive` delegating to `MeshPipeline::render` via `Primitive::render()`. **Read `~/Code/_ref/libcosmic/iced/examples/custom_shader/scene.rs` first** and mirror its structure exactly (its `Primitive`/`Pipeline` impl, `draw(bounds, cursor)`, `update`/`resize` hooks); the only deltas: our `MeshPipeline` instead of cubes, our own `Depth24PlusStencil8` instead of its depth format, and `LoadOp::Load` on color. The acceptance is visual (next step) — structure follows the reference, not this plan's guesses.

Where the reference's `Primitive` impl goes (name it `SpikePrimitive`), the `render()`-style method must call `self.mesh.render(device, encoder, target)` from Step 3 — that is the one load-bearing line this spike exists to prove:

```rust
// inside SpikePrimitive's render, after the reference example's pattern:
self.mesh.render(device, encoder, target);
```

- [ ] **Step 5: Build + Preston visual gate**

```bash
git push thelio main
ssh thelio 'bash -lc "cd ~/Code/cad-studio-cosmic && cargo build -p viewer --example spike_window 2>&1 | tail -3"'
```

Then Preston, from a COSMIC-session terminal on thelio: `cd ~/Code/cad-studio-cosmic && cargo run -p viewer --example spike_window`.

Expected: a window showing the two fixture boxes (blue+red fused-looking pair, neutral color) from the iso angle; drag orbits (wire mouse_area → yaw/pitch deltas into `Camera`), scroll zooms (0.9^steps per upstream §5). No window = fail; wrong pass structure (e.g. iced content clobbered) shows as a black/garbage window = fail.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock viewer/Cargo.toml viewer/src/render/mod.rs viewer/src/render/mesh.wgsl viewer/examples/spike_window.rs
git commit -m "feat(viewer): windowed mesh spike on viewer-owned Depth24PlusStencil8 pass via Primitive::render()"
git push thelio main
```

---

### Task 7: Headless render harness + stability/resize tests

**Files:**
- Create: `viewer/tests/render_headless.rs`

**Interfaces:**
- Consumes: `viewer::render::{MeshPipeline, load_fixture_stl, Camera}` (Task 6).
- Produces: the automated render gate every later viewer task uses; `render_to_pixels(mesh, width, height, camera) -> Vec<u8>` helper.

- [ ] **Step 1: Write the tests (failing — harness doesn't exist yet)**

`viewer/tests/render_headless.rs`:

```rust
use std::path::PathBuf;

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/golden").join(rel)
}

async fn device() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions { compatible_surface: None, ..Default::default() })
        .await
        .expect("surfaceless wgpu adapter (run on thelio; if none: check WGPU_BACKEND / Vulkan ICDs in this ssh session)");
    adapter.request_device(&wgpu::DeviceDescriptor::default(), None).await.unwrap()
}

fn render_once(device: &wgpu::Device, queue: &wgpu::Queue, mesh: &viewer::render::TriMesh, w: u32, h: u32) -> Vec<u8> {
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut pipeline = viewer::render::MeshPipeline::new(device, queue, target.format(), mesh, (w, h), 1);
    let mut encoder = device.create_command_encoder(&Default::default());
    // clear to the app background first, then viewer pass loads on top (mirrors iced underlay)
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view, resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.1, g: 0.1, b: 0.12, a: 1.0 }), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            ..Default::default() // fill per wgpu 28: label/timestamp/occlusion fields
        });
        drop(pass);
    }
    pipeline.render(device, &mut encoder, &view);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (w * h * 4) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(&wgpu::TexelCopyTextureInfo { texture: &target, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        &wgpu::TexelCopyBufferInfo { buffer: &readback, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: w * 4, rows_per_image: h } },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 });
    queue.submit(Some(encoder.finish()));
    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| { tx.send(r).unwrap(); });
    device.poll(wgpu::PollType::Wait).unwrap();
    rx.recv().unwrap().unwrap();
    let data = slice.get_mapped_range().to_vec();
    readback.unmap();
    data
}

fn ink_pixels(px: &[u8]) -> usize {
    px.chunks(4).filter(|c| (c[0] as i32 - 26).abs() + (c[1] as i32 - 26).abs() + (c[2] as i32 - 31).abs() > 30).count()
}

#[test]
fn mesh_renders_with_ink_and_is_frame_stable() {
    let rt = tokio::runtime::Runtime::new().unwrap(); // cargo add -p viewer --dev tokio (or use futures::executor::block_on with wgpu's async — prefer tokio)
    let (device, queue) = rt.block_on(device());
    let mesh = viewer::render::load_fixture_stl(&fixture("model.stl"));
    let a = render_once(&device, &queue, &mesh, 800, 600);
    assert!(ink_pixels(&a) > 5_000, "mesh should cover a healthy area, got {}", ink_pixels(&a));
    let b = render_once(&device, &queue, &mesh, 800, 600);
    assert_eq!(a, b, "second frame must be identical — uncleared stencil/depth leaks state (Review Focus 3)");
}

#[test]
fn viewport_resize_reallocates_depth_and_renders() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (device, queue) = rt.block_on(device());
    let mesh = viewer::render::load_fixture_stl(&fixture("model.stl"));
    let small = render_once(&device, &queue, &mesh, 800, 600);
    let big = render_once(&device, &queue, &mesh, 1024, 768);
    assert!(ink_pixels(&small) > 5_000);
    let growth = ink_pixels(&big) as f64 / ink_pixels(&small) as f64;
    assert!((1.2..1.9).contains(&growth), "1024x768 vs 800x600 ink ratio {growth:.2} should track area ratio ~1.64 (Review Focus 4)");
}
```

(`cargo add -p viewer --dev tokio` first. If `MeshPipeline::new`'s signature changed in Task 6 during reference-clone reconciliation, update the call — Task 6's commit message records the final signature.)

- [ ] **Step 2: Run on thelio**

```bash
git push thelio main
ssh thelio 'bash -lc "cd ~/Code/cad-studio-cosmic && cargo test -p viewer --test render_headless -- --nocapture"'
```

Expected: 2 passed. If no adapter is found over ssh, try `ssh thelio 'bash -lc "... WGPU_BACKEND=vulkan cargo test ..."'` and check `ls /usr/share/vulkan/icd.d/`; if the ICD needs a session env, run tests from Preston's tmux on thelio — record whatever was needed in the commit message (it becomes the canonical test invocation).

- [ ] **Step 3: Commit**

```bash
git add viewer/Cargo.toml viewer/Cargo.lock viewer/tests/render_headless.rs
git commit -m "test(viewer): headless surfaceless render harness — ink, frame stability, resize"
git push thelio main
```

---

### Task 8: Stencil cross-section proof (GPU cap face)

**Files:**
- Modify: `viewer/src/render/mod.rs` (section params live in `Camera` already; add cap pipeline), `viewer/src/render/cap.wgsl`
- Modify: `viewer/tests/render_headless.rs`

**Interfaces:**
- Consumes: Task 7 harness — extend `render_once` to `render_once(device, queue, mesh, w, h, camera: viewer::render::Camera)` and update Task 7's call sites to pass `viewer::render::default_camera(&mesh)` (exported from Task 6's module).
- Produces: `MeshPipeline` renders a red cap quad where stencil ≠ 0 when `section_enable = 1.0`; M6 productizes with axis/position controls.

- [ ] **Step 1: cap.wgsl**

`viewer/src/render/cap.wgsl`:

```wgsl
struct Camera {
    view_proj: mat4x4<f32>,
    section_axis: u32,
    section_pos: f32,
    section_enable: f32,
};
@group(0) @binding(0) var<uniform> camera: Camera;

@vertex
fn vs_cap(@builtin(vertex_index) i: u32, @location(0) corner: vec2<f32>) -> @builtin(position) vec4<f32> {
    // corner is a quad in world space on the section plane; lifted to world by CPU
    // (spike: CPU supplies the 4 corners already projected via view_proj on CPU side)
    return vec4<f32>(corner, 0.0, 1.0); // placeholder NDC quad; executor: transform world corners with view_proj
}

@fragment
fn fs_cap() -> @location(0) vec4<f32> {
    return vec4<f32>(0.92, 0.15, 0.10, 1.0); // upstream red cut face
}
```

(Executor: make `vs_cap` take world-space corners from a 4-vertex buffer and apply `camera.view_proj`; delete this placeholder comment once done — the plan intentionally does not freeze that detail, it is mechanical.)

Cap pipeline: same bind group layout; depth `CompareFunction::LessEqual`, `depth_write_enabled: false`; stencil front/back `compare: NotEqual, reference 0` (read-only); `cull_mode: None`.

- [ ] **Step 2: Extend render() to draw the cap when section_enable > 0.5**

Add `pub fn set_camera(&self, queue: &wgpu::Queue, camera: &Camera)` to `MeshPipeline` (writes the uniform buffer) so the harness/tests can override the default. Compute the world-space cap quad from the mesh aabb: on the section axis, at `section_pos`, spanning the aabb's other two axes ±10%. Upload as a 4-vertex triangle-strip buffer at pipeline build; draw it in the same pass after the mesh (stencil from the mesh's inc/dec is already in the attachment).

- [ ] **Step 3: Headless proof test (write first, watch it fail, then implement)**

Append to `viewer/tests/render_headless.rs`:

```rust
fn red_pixels(px: &[u8]) -> usize {
    px.chunks(4).filter(|c| c[0] > 180 && c[1] < 90 && c[2] < 90).count()
}

#[test]
fn section_cap_appears_and_shrinks_toward_the_edge() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (device, queue) = rt.block_on(device());
    let mesh = viewer::render::load_fixture_stl(&fixture("model.stl"));
    let mut cam = viewer::render::default_camera(&mesh);
    cam.section_enable = 1.0;
    cam.section_axis = 2; // Z — fixture STL is Z-up and the boxes stack along Z
    let span = mesh.aabb_max[2] - mesh.aabb_min[2];
    let red_at = |frac: f32| {
        cam.section_pos = mesh.aabb_min[2] + span * frac;
        red_pixels(&render_once(&device, &queue, &mesh, 800, 600, cam))
    };
    let mid = red_at(0.5);
    let edge = red_at(0.15);
    assert!(mid > 2_000, "cap must be visible at mid-plane, got {mid} red px");
    assert!(edge < mid / 2, "cap near the edge must shrink: mid {mid} vs edge {edge}");
}
```

Run → expect FAIL (no cap yet) → implement Steps 1–2 → expect PASS on thelio.

- [ ] **Step 4: Preston visual gate**

`cargo run -p viewer --example spike_window` with section hard-enabled (add a checkbox or key toggle): red cut face visible on the sliced model, no ghosting across frames.

- [ ] **Step 5: Commit**

```bash
git add viewer/src/render/mod.rs viewer/src/render/cap.wgsl viewer/tests/render_headless.rs
git commit -m "feat(viewer): stencil cross-section cap on GPU (inc/dec + NotEqual quad) with headless proof"
git push thelio main
```

---

### Task 9: MSAA resolve proof

**Files:**
- Modify: `viewer/src/render/mod.rs` (msaa path), `viewer/tests/render_headless.rs`

**Interfaces:**
- Consumes: Tasks 6–8 (`MeshPipeline`, harness).
- Produces: the M3 MSAA decision data: does own-count + resolve work in our pass structure? Pattern per spec §5/6413f86: pass renders to an owned MSAA color texture, resolves to an intermediate, then a second pass draws the intermediate onto the iced target (alpha blend, Load) — iced's own content underneath is preserved.

- [ ] **Step 1: Extend render path for msaa > 1**

When `msaa > 1`: create owned color texture (sample_count 4, RENDER_ATTACHMENT), depth at count 4; pass color attachment = msaa view with `resolve_target: Some(&intermediate)`; intermediate is a single-sample RENDER_ATTACHMENT|TEXTURE_BINDING texture; then a second render pass draws a fullscreen textured quad (new tiny `blit.wgsl`) onto `target` with `LoadOp::Load` + standard alpha blending. In the headless harness the "target" is the readback texture — same code path.

- [ ] **Step 2: Headless proof (write first)**

```rust
#[test]
fn msaa_smooths_a_diagonal_edge() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (device, queue) = rt.block_on(device());
    let mesh = viewer::render::load_fixture_stl(&fixture("model.stl"));
    // camera aimed so a box edge crosses mid-screen diagonally: reuse default iso —
    // box silhouettes guarantee high-contrast diagonal edges at this angle.
    let px1 = render_once_msaa(&device, &queue, &mesh, 800, 600, 1);
    let px4 = render_once_msaa(&device, &queue, &mesh, 800, 600, 4);
    let intermediates = |px: &[u8]| -> usize {
        px.chunks(4)
            .filter(|c| {
                let lum = (c[0] as u32 + c[1] as u32 + c[2] as u32) / 3;
                // background ~26..31, mesh ~100+; count the in-between (anti-aliased) pixels
                (45..90).contains(&lum)
            })
            .count()
    };
    assert!(
        intermediates(&px4) as f64 > intermediates(&px1) as f64 * 1.5,
        "4x MSAA should produce >=1.5x the intermediate-valued edge pixels: {} vs {}",
        intermediates(&px4), intermediates(&px1)
    );
}
```

(`render_once_msaa(device, queue, mesh, w, h, camera, msaa: u32)` = `render_once` with the sample count threaded through to `MeshPipeline::new` and `render`; add it next to `render_once` in the harness. The 45..90 band assumes neutral mesh color vs dark background — sanity-check actual luminances with a `println!` histogram first and adjust the band to sit strictly between background and lit-mesh levels; record the chosen band in a comment.)

Run → FAIL (msaa path not implemented) → implement Step 1 → PASS on thelio.

- [ ] **Step 3: Preston visual gate + decision note**

Visual: aliasing visibly reduced vs the 1× toggle. Append the M3 decision to `docs/research/3mf-decision.md`'s sibling `docs/research/msaa-decision.md` (2–6 lines: chosen count, perf feel, whether blit pattern is kept — it is unless something failed).

- [ ] **Step 4: Commit**

```bash
git add viewer/src/render/mod.rs viewer/src/render/blit.wgsl viewer/tests/render_headless.rs docs/research/msaa-decision.md
git commit -m "feat(viewer): own-MSAA render path with resolve+blit, headless edge-quality proof"
git push thelio main
```

---

### Task 10: M0 wrap-up

**Files:**
- Modify: `README.md` (dev loop), `docs/superpowers/plans/2026-10-11-m0-bootstrap.md` (tick boxes)

**Interfaces:**
- Consumes: Tasks 1–9.
- Produces: M0 exit criteria met and documented; the M1 plan gets written next against the tree as it now stands.

- [ ] **Step 1: README dev-loop section**

Append to `README.md`:

```markdown
## Developing

Builds and tests run on a Linux machine (thelio); the repo builds nowhere else by design.

```bash
git push thelio main
ssh thelio 'bash -lc "cd ~/Code/cad-studio-cosmic && git pull && cargo test --workspace"'
```

GUI: from a terminal inside the COSMIC session: `just run` (or `just render-spike` for the
viewer spike). Headless render tests: `just headless`. Fixtures: generated once by
`fixtures/generator/model.py` with the venv at `~/.local/share/cad-studio/venv` (uv) —
do not regenerate casually; they are committed goldens.
```

- [ ] **Step 2: Full suite green on thelio**

```bash
ssh thelio 'bash -lc "cd ~/Code/cad-studio-cosmic && git pull && cargo test --workspace 2>&1 | tail -15"'
```

Expected: all tests pass (svg_check, threemf_spike, parse3mf, render_headless ×4, app builds).

- [ ] **Step 3: M0 checklist against spec §10**

Verify each spec M0 bullet has evidence: workspace+template (T1), thelio remotes (T1), toolchain (T1 build), fixtures (T2), resvg check (T4), 3MF spike+ADR (T5), extractor (T3), shader `Primitive::render()` proof incl. stencil + MSAA (T6/T8/T9). Record any gap as a new task — do not close M0 with a gap.

- [ ] **Step 4: Commit + push + report**

```bash
git add README.md docs/superpowers/plans/2026-10-11-m0-bootstrap.md
git commit -m "docs: M0 complete — dev loop, plan ticks"
git push thelio main
```

Report: what landed, spike outcomes (3MF decision, MSAA decision, any [patch] needed for libcosmic's iced, the canonical headless test invocation), Preston's visual gates passed, and the M1 plan as the next artifact.
