# cad-dev rig

Build, test and capture host for when thelio is out: an incus system container
(`cad-dev`, Ubuntu 24.04, on zeus's TrueNAS virtualization, pool `system`).
ssh alias `cad-dev` (ProxyJump zeus). No GPU: wgpu runs on llvmpipe through the
GL backend under Xvfb, and the viewer GPU tests run on lavapipe.

- `rig.sh` (run on mercury): per-worktree sync / run / capture. See its header.
- `cap.sh` (runs on the rig): one Xvfb per capture, no window manager.
- `open-box.drive`: completes first run and opens the matched-state fixture.
- Matched-state fixture: `fixtures/matched-state/make-box-fixture.sh ~/fixtures`
  rebuilds `~/fixtures/box-fixture`, the content of the upstream gallery shots
  in `shots/upstream/`.
- `seed-theme/`: writes libcosmic's default light and dark themes into
  cosmic-config. A real COSMIC desktop gets these from cosmic-settings; without
  them light mode silently renders with the dark palette.
- `claude-stub`: installed as `~/.local/bin/claude` so the agent probe passes.

Rig provisioning (one time, done 2026-11-02): build-essential, X11/wayland dev
libs, mesa (llvmpipe/lavapipe), Xvfb, xdotool, imagemagick, fonts, xdg-user-dirs
(the engine's settings tests need a Documents dir), Cosmic icon theme copied to
`~/.local/share/icons/Cosmic`, rustup, uv + the build123d venv at
`~/.local/share/cad-studio/venv`. Snapshots: `ssh zeus sudo incus snapshot
list cad-dev` (`base-toolchain` is the clean provisioned state).

## Batch captures (capall.sh)
`scripts/rig/shots/` holds the drive scripts, `spec` (20 shots: 10 states ×
light/dark) and `recents.ron` (the two fixture projects upstream's gallery
shows). On the rig, with the fixture built at ~/fixtures-cap:

    ~/wt/<name>/fixtures/matched-state/make-box-fixture.sh ~/fixtures-cap
    RECENTS=~/wt/<name>/scripts/rig/shots/recents.ron \
      ~/wt/<name>/scripts/rig/capall.sh ~/targets/<name>/release/cad-studio ~/capout ~/wt/<name>/scripts/rig/shots/spec

Every shot gets its own Xvfb and ~/.config copy; 8 run at once; each waits
until its frame has painted. The full set takes under 20 s.
