# Build/test/capture host

Builds, tests and captures run on a Linux host, never on the Mac: `rig.sh`
(run on mercury) syncs a worktree to `RIG_HOST` (default `thelio`) and runs
commands there with a per-worktree target dir. Captures run under Xvfb with
no window manager (`cap.sh`, `capall.sh`), so the host needs Xvfb, xdotool
and ImageMagick.

- `rig.sh` (run on mercury): per-worktree sync / run / capture. See its header.
- `cap.sh` (runs on the host): one Xvfb per capture, no window manager.
- `open-box.drive`: completes first run and opens the matched-state fixture.
- Matched-state fixture: `fixtures/matched-state/make-box-fixture.sh ~/fixtures`
  rebuilds `~/fixtures/box-fixture`, the content of the upstream gallery shots
  in `shots/upstream/`.
- `seed-theme/`: writes libcosmic's default light and dark themes into
  cosmic-config. A host without a COSMIC session needs these, or light mode
  silently renders with the dark palette.
- `claude-stub`: a stand-in `claude` for hosts where the agent probe must pass
  without a login.

## Batch captures (capall.sh)
`scripts/rig/shots/` holds the drive scripts, `spec` (20 shots: 10 states ×
light/dark) and `recents.ron` (the two fixture projects upstream's gallery
shows). On the rig, with the fixture built at ~/fixtures-cap:

    ~/wt/<name>/fixtures/matched-state/make-box-fixture.sh ~/fixtures-cap
    RECENTS=~/wt/<name>/scripts/rig/shots/recents.ron \
      ~/wt/<name>/scripts/rig/capall.sh ~/targets/<name>/release/cad-studio ~/capout ~/wt/<name>/scripts/rig/shots/spec

Every shot gets its own Xvfb and ~/.config copy; 8 run at once; each waits
until its frame has painted. The full set takes under 20 s.
