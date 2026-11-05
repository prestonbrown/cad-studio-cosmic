#!/bin/bash
# Runs on mercury. Drives the build/test/capture host (RIG_HOST, default
# thelio). Each worktree gets its own source dir
# (~/wt/<name>) and target dir (~/targets/<name>) on the host, so several
# worktrees build in parallel without sharing a cargo lock. Nothing is ever
# committed on the rig: source of record stays on mercury.
#
#   rig.sh sync <name> [worktree-dir]    rsync the worktree (default: cwd) up
#   rig.sh run  <name> <command...>      run in ~/wt/<name> with its target dir
#                                        (e.g. rig.sh run foo cargo test -p app --lib)
#   rig.sh cap  <name> <cap> [WxH] [drive] [wait] [light|dark] [xdo-file]
#                                        capture with ~/targets/<name>/release/cad-studio,
#                                        png lands in <worktree>/../rig-caps/<name>/<cap>.png
#
# A drive or xdo file given as a local path is copied up first.
set -eu
cmd=$1; name=$2; shift 2
host=${RIG_HOST:-thelio}
case "$cmd" in
  sync)
    src=${1:-$PWD}
    ssh "$host" "mkdir -p ~/wt/$name ~/targets"
    # A fresh target per worktree: copying a checkout's target dir costs
    # more than a clean build on a many-core host.
    # No -t: a file whose content changed lands with the rig's current time
    # (cargo's freshness check is mtime-based); --checksum skips files whose
    # content already matches, so unchanged crates stay fresh.
    rsync -rlpDz --checksum --delete --exclude target --exclude .superpowers --exclude .git \
      -e ssh "$src"/ "$host:wt/$name/"
    ;;
  run)
    # ~/.local/bin first: on a jobpool host its cargo/make shims share one
    # jobserver with every other build there.
    ssh "$host" "cd ~/wt/$name && { [ -f ~/.cargo/env ] && . ~/.cargo/env; true; } && export PATH=~/.local/bin:\$PATH CARGO_TARGET_DIR=~/targets/$name && $*"
    ;;
  cap)
    cap=$1; geom=${2:-1320x840}; drive=${3:-open-box.drive}; wait=${4:-12}; mode=${5:-light}; xdo=${6:-}
    here=$(cd "$(dirname "$0")" && pwd)
    up() { if [ -f "$1" ]; then scp -q "$1" "$host:caps/$name-$(basename "$1")"; echo "caps/$name-$(basename "$1")";
           elif [ -f "$here/$1" ]; then scp -q "$here/$1" "$host:caps/$name-$1"; echo "caps/$name-$1"; else echo "$1"; fi; }
    ssh "$host" "mkdir -p ~/caps"
    d=$(up "$drive"); x=""; [ -n "$xdo" ] && x=$(up "$xdo")
    ssh "$host" "cd ~ && BIN=~/targets/$name/release/cad-studio ~/wt/$name/scripts/rig/cap.sh $name-$cap $geom ~/$d $wait $mode ${x:+~/$x}"
    out=${RIG_CAPS:-$PWD/../rig-caps/$name}; mkdir -p "$out"
    scp -q "$host:caps/$name-$cap.png" "$out/$cap.png" && echo "$out/$cap.png"
    ;;
  *) echo "unknown command $cmd" >&2; exit 2 ;;
esac
