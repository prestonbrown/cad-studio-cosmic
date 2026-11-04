#!/bin/bash
# Captures the upstream gallery states the port compares against.
#   run.sh [state:mode ...]      default: the LIGHT and DARK lists below
# States are the functions below; each takes the mode (light|dark) and writes
# shots/upstream/<name>[-dark].png. Per-launch settings travel as
# argument-domain defaults. The app still writes window frames and recents to
# the user's stored defaults, so the run exports them first and restores them
# on exit (the backup stays in BACKUP).
set -u
here=$(cd "$(dirname "$0")" && pwd)
source "$here/lib.sh"
BACKUP=${BACKUP:-${TMPDIR:-/tmp}/cad-studio-defaults-$(date +%Y%m%d-%H%M%S).plist}
defaults export "$BUNDLE" "$BACKUP" || exit 1
restore() { quit_app; defaults delete "$BUNDLE"; defaults import "$BUNDLE" "$BACKUP"; echo "$(ts) defaults restored from $BACKUP"; }
trap restore EXIT
P=$FX/box-fixture
E=$FX/empty-project
RECENTS=$(recents "$P" "$E")
sfx() { [ "$1" = dark ] && echo -dark; }
open_project() { # <mode> <path> [launch args]
  local mode=$1 path=$2; shift 2
  "$here/../../fixtures/matched-state/make-box-fixture.sh" "$FX" >/dev/null
  launch "$mode" -recentProjects "$(recents "$path")" -partsLayout exploded "$@"
  $D press "$(basename "$path")" && $D wait "^$(basename "$path")\$" 15 >/dev/null && sleep 2
}

welcome()         { launch "$1" -recentProjects "()"; sleep 1; shot cap "$OUT/welcome$(sfx $1).png" Welcome; }
welcome_recents() { launch "$1" -recentProjects "$RECENTS"; sleep 1.5; shot cap "$OUT/welcome-recents$(sfx $1).png" Welcome; }
setup_sheet()     { launch "$1" -recentProjects "()" -hasCompletedSetup NO; sleep 1.5; shot capall "$OUT/setup-sheet$(sfx $1).png" Welcome; }
new_project() {
  launch "$1" -recentProjects "()"; $D press "Create New Project"; sleep 2
  $D type "Staffa a L"; sleep 0.5; shot capall "$OUT/new-project$(sfx $1).png" Welcome
}
project_model() { open_project "$1" "$P"; shot cap "$OUT/project-model$(sfx $1).png" box-fixture; }
version_menu() {
  open_project "$1" "$P"; $D press Versions; sleep 0.8
  shot capall "$OUT/version-menu$(sfx $1).png" box-fixture; $D key 53
}
parts() { open_project "$1" "$P"; $D press Parameters 2; sleep 1; shot cap "$OUT/parts$(sfx $1).png" box-fixture; }
section() { open_project "$1" "$P"; $D press Section; sleep 1.5; shot cap "$OUT/section$(sfx $1).png" box-fixture; }
playback() { open_project "$1" "$P"; $D press Animation; sleep 2; shot cap "$OUT/playback$(sfx $1).png" box-fixture; }
settings() {
  launch "$1" -recentProjects "()"; $D key 43 cmd; $D wait 'General|Agent|Python|Advanced' 10 >/dev/null
  for t in General Agent Python Advanced; do
    $D press "$t"; sleep 1
    shot cap "$OUT/settings-$(echo $t | tr A-Z a-z)$(sfx $1).png" "^$t\$"
  done
}
about() { launch "$1" -recentProjects "()"; $D press "About CAD Studio"; sleep 1; shot cap "$OUT/about$(sfx $1).png" '^$|About'; }
# The interpreter check fails under a bogus PYTHONHOME, so the setup sheet
# opens in its failed state without touching the real environment.
python_sheet() { PYTHONHOME=/nonexistent open_project "$1" "$P"; sleep 2; shot capall "$OUT/python-sheet$(sfx $1).png" box-fixture; }
# codex is not installed, so sending a message with that engine selected opens
# the missing-agent sheet and leaves the error row in the chat.
agent_missing() {
  open_project "$1" "$P" -agentEngine codex
  local w x y; w=$($D wins | awk -F'\t' '$4=="box-fixture"{print $3}')
  IFS=, read -r x y _ _ <<<"$w"
  $D click $((x + 224)) $((y + 816)); sleep 0.5; $D type "Aggiungi due fori da 3 mm."; $D key 36; sleep 2.5
  shot capall "$OUT/agent-missing$(sfx $1).png" box-fixture
  $D key 53; sleep 1; shot cap "$OUT/chat-error$(sfx $1).png" box-fixture
}
build_error() { open_project "$1" "$E"; $D press "Build Model"; sleep 1.5; shot cap "$OUT/build-error$(sfx $1).png" empty-project; }

LIGHT="new_project python_sheet agent_missing build_error playback"
DARK="welcome welcome_recents setup_sheet new_project project_model version_menu parts section playback settings about python_sheet agent_missing build_error"
if [ $# -gt 0 ]; then for s in "$@"; do ${s%:*} ${s#*:}; done
else
  for s in $LIGHT; do $s light; done
  for s in $DARK; do $s dark; done
fi
