# CAD Studio for COSMIC

Describe a part, get a printable model — on Linux. A Rust port of
[cad-studio](https://github.com/filippofinke/cad-studio) for the
[COSMIC desktop](https://system76.com/cosmic), powered by your locally installed
Claude Code (or Codex) and [build123d](https://build123d.readthedocs.io).

Status: **design phase** — see
[the port design spec](docs/superpowers/specs/2026-10-11-cad-studio-cosmic-port-design.md)
and the research under `docs/research/`.

MIT licensed; a port of Filippo Finke's MIT-licensed original. Not affiliated
with Anthropic.

## Developing

Builds and tests are gated on the Linux machine (thelio), the canonical build/test environment — the suite also builds and passes on macOS, which is fine for compile iteration but is not the gate.

```bash
git push thelio main
ssh thelio 'bash -lc "cd ~/Code/cad-studio-cosmic && git pull && cargo test --workspace"'
```

GUI: from a terminal inside the COSMIC session: `just run` (or `just render-spike` for the
viewer spike). Headless render tests: `just headless`. The recipes run through
[`just`](https://github.com/casey/just) — any machine expected to run them needs it
installed (thelio has it). Fixtures: generated once by
`fixtures/generator/model.py` with the venv at `~/.local/share/cad-studio/venv` (uv) —
do not regenerate casually; they are committed goldens.
