#!/bin/bash
# Asserts the criterion-3 run's on-disk state for a project directory.
#
#   check.sh <project-dir> [step...]      steps: 1 4 5 6 7 8 (default: all)
#
#   1  output/model.stl and output/schematic.svg exist and are non-empty
#   4  .cadstudio/versions/2/ exists
#   5  .cadstudio/project.json has current_version 1
#   6  after the last user row asking for the fillet, .cadstudio/chat.json
#      holds the app's failed-build row (role system, action askClaudeToFix);
#      the agent's own last tool row is reported, not asserted. The failed
#      activity is not on disk: run.sh asserts it from the mark's log line
#   7  every pid in the process registry is alive and none is an agent turn
#      (REGISTRY, default ~/.local/share/cad-studio/running-processes)
#   8  the export ZIP (ZIP, default <project-dir>.zip) lists model.py and
#      model.stl; the export zips with -j (upstream Project.swift:444), so
#      outputs sit at the archive root
#
# One line per check on stdout (`PASS <step> …` / `FAIL <step> …`); exits 1
# when any check fails, 2 on bad usage.
set -u
[ $# -ge 1 ] || { echo "usage: check.sh <project-dir> [step...]" >&2; exit 2; }
p=${1%/}; shift
steps=${*:-1 4 5 6 7 8}
registry=${REGISTRY:-$HOME/.local/share/cad-studio/running-processes}
zip=${ZIP:-$p.zip}
failed=0
pass() { echo "PASS $1 $2"; }
fail() { echo "FAIL $1 $2"; failed=1; }

check1() {
  local f missing=
  for f in output/model.stl output/schematic.svg; do
    [ -s "$p/$f" ] || missing="$missing $f"
  done
  [ -z "$missing" ] && pass 1 "model.stl and schematic.svg present" \
                    || fail 1 "missing or empty:$missing"
}

check4() {
  [ -d "$p/.cadstudio/versions/2" ] && pass 4 "versions/2 exists" \
                                    || fail 4 "no .cadstudio/versions/2"
}

check5() {
  local v
  v=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("current_version"))' \
        "$p/.cadstudio/project.json" 2>&1) || { fail 5 "project.json unreadable: $(echo "$v" | tail -1)"; return; }
  [ "$v" = 1 ] && pass 5 "current_version 1" || fail 5 "current_version is $v, want 1"
}

check6() {
  local verdict
  verdict=$(python3 - "$p/.cadstudio/chat.json" <<'PY'
import json, sys
try:
    rows = json.load(open(sys.argv[1]))
except Exception as e:
    print(f"FAIL chat.json unreadable: {e}"); sys.exit()
asks = [i for i, r in enumerate(rows)
        if r.get("role") == "user" and "raccordo" in (r.get("text") or "")]
if not asks:
    print("FAIL no user row asking for the fillet"); sys.exit()
after = rows[asks[-1] + 1:]
# the app's own record of a failed model.py build (bridge.rs, BuildOutcome::Failed)
failed = [r for r in after if r.get("role") == "system" and r.get("action") == "askClaudeToFix"]
tools = [r for r in after if r.get("role") == "tool" and r.get("tool")]
agent = f"; the agent's last tool row is {tools[-1]['tool'].get('state')}" if tools else ""
if failed:
    print(f"PASS failed-build row '{failed[-1].get('text')}' after the fillet request{agent}")
else:
    print(f"FAIL no failed-build row (system, askClaudeToFix) after the fillet request{agent}")
PY
)
  case "$verdict" in PASS*) pass 6 "${verdict#PASS }" ;; *) fail 6 "${verdict#FAIL }" ;; esac
}

check7() {
  [ -e "$registry" ] || { pass 7 "registry $registry absent (nothing running)"; return; }
  local line pid comm bad=
  while IFS= read -r line; do
    pid=${line%%:*}
    case "$pid" in ''|*[!0-9]*) continue ;; esac
    if ! kill -0 "$pid" 2>/dev/null; then
      bad="$bad $pid(dead)"
    else
      comm=$(ps -o args= -p "$pid" 2>/dev/null)
      case "$comm" in *claude*|*codex*) bad="$bad $pid(agent: $comm)" ;; esac
    fi
  done < "$registry"
  [ -z "$bad" ] && pass 7 "registry lists no dead or agent pid" \
                || fail 7 "registry $registry lists$bad"
}

check8() {
  # the export runs off the UI thread; give it a moment to land
  local i listing missing= f
  for i in $(seq 20); do [ -s "$zip" ] && break; sleep 1; done
  listing=$(python3 -c 'import sys, zipfile; print("\n".join(zipfile.ZipFile(sys.argv[1]).namelist()))' \
              "$zip" 2>&1) || { fail 8 "cannot list $zip: $(echo "$listing" | tail -1)"; return; }
  for f in model.py model.stl; do
    printf '%s\n' "$listing" | grep -qx "$f" || missing="$missing $f"
  done
  [ -z "$missing" ] && pass 8 "$(basename "$zip") lists model.py and model.stl" \
                    || fail 8 "$(basename "$zip") lacks:$missing (has: $(echo $listing))"
}

for s in ${steps//,/ }; do
  case "$s" in
    1|4|5|6|7|8) "check$s" ;;
    *) echo "check.sh: unknown step $s (steps: 1 4 5 6 7 8)" >&2; exit 2 ;;
  esac
done
exit $failed
