#!/bin/bash
# Runs ON the rig. The criterion-3 end-to-end run against the REAL agent: one
# app per theme on its own Xvfb, driven by e2e.drive in a fresh empty project.
# At each `mark` the drive logs, this script screenshots the screen and runs
# the check.sh steps the spec names for that mark.
#
#   run.sh <bin> <outdir> [light|dark ...]   (default: light dark)
#   run.sh --unstub        move the claude stub aside (~/.local/bin/claude.stub)
#                          and put back a real install kept at claude.real
#   run.sh --restore-stub  keep a real install at ~/.local/bin/claude.real and
#                          put the stub back as ~/.local/bin/claude
#
# Output per theme in <outdir>/<theme>/: <mark>.png per spec line, app.log,
# checks.log, summary.txt, the project (e2e-cubo/) and its export ZIP.
# E2E_TIMEOUT (seconds, default 5400) bounds each theme's run. Exits non-zero
# when the drive fails, a check fails, a mark is missing, or the app dies.
set -u
here=$(cd "$(dirname "$0")" && pwd)
bindir=$HOME/.local/bin
stub_src=$here/../claude-stub
ts() { date +%H:%M:%S; }
is_stub() { head -c 300 "$1" 2>/dev/null | grep -q 'Rig stub'; }

case "${1:-}" in
  --unstub)
    if [ -e "$bindir/claude" ] && is_stub "$bindir/claude"; then
      mv "$bindir/claude" "$bindir/claude.stub" && echo "stub moved to $bindir/claude.stub"
    fi
    if [ -e "$bindir/claude.real" ] || [ -L "$bindir/claude.real" ]; then
      mv "$bindir/claude.real" "$bindir/claude" && echo "real claude restored from claude.real"
    fi
    exit 0 ;;
  --restore-stub)
    if { [ -e "$bindir/claude" ] || [ -L "$bindir/claude" ]; } && ! is_stub "$bindir/claude"; then
      mv "$bindir/claude" "$bindir/claude.real" && echo "real claude kept at $bindir/claude.real"
    fi
    if [ -e "$bindir/claude.stub" ]; then mv "$bindir/claude.stub" "$bindir/claude"
    else mkdir -p "$bindir" && cp "$stub_src" "$bindir/claude" && chmod +x "$bindir/claude"; fi
    echo "stub restored at $bindir/claude"
    exit 0 ;;
esac

[ $# -ge 2 ] || { sed -n '2,17p' "$0" >&2; exit 2; }
bin=$1; out=$2; shift 2
themes=${*:-light dark}
geom=1320x840

# ── preflight: everything a run needs, before any Xvfb starts ───────────────
die() { echo "run.sh: $*" >&2; exit 1; }
[ -x "$bin" ] || die "no app binary at $bin"
for t in $themes; do case $t in light|dark) ;; *) die "unknown theme $t (light|dark)" ;; esac; done
drive_marks=$(awk '$1 == "mark" { print $2 }' "$here/e2e.drive" | sort)
spec_marks=$(grep -v '^#' "$here/spec" | awk 'NF { print $1 }' | sort)
[ -n "$spec_marks" ] || die "empty spec at $here/spec"
[ "$drive_marks" = "$spec_marks" ] || die "e2e.drive marks and spec lines differ: $(diff <(echo "$drive_marks") <(echo "$spec_marks") | grep '^[<>]' | tr '\n' ' ')"
claude=$(bash -lc 'command -v claude' 2>/dev/null)
[ -n "$claude" ] || die "no claude on the login PATH; log in first"
is_stub "$claude" && die "claude is the rig stub ($claude); log in first (run.sh --unstub, then claude login)"
version=$("$claude" --version 2>&1) || die "claude --version failed: $version"
for tool in Xvfb xdotool xwd convert python3; do
  command -v $tool >/dev/null || die "$tool not installed"
done
[ -x /usr/bin/zip ] || die "/usr/bin/zip not installed (the app's export runs it): sudo apt install zip"

status=0
for theme in $themes; do
  work=$out/$theme
  rm -rf "$work"; mkdir -p "$work/e2e-cubo"
  proj=$work/e2e-cubo; zip=$work/e2e-cubo.zip; png=$work/schizzo.png
  log=$work/app.log; checks=$work/checks.log; summary=$work/summary.txt
  : > "$checks"
  echo "$(ts) $theme: claude $version at $claude" > "$summary"
  convert -size 320x240 xc:orange -fill navy -draw 'circle 160,120 160,60' "$png"
  sed -e "s#@PROJECT@#$proj#g" -e "s#@PNG@#$png#g" -e "s#@ZIP@#$zip#g" \
    "$here/e2e.drive" > "$work/e2e.drive"

  cfg=$(mktemp -d /tmp/e2ecfg.XXXX)
  cp -a ~/.config/. "$cfg/"
  mkdir -p "$cfg/cosmic/com.system76.CosmicTheme.Mode/v1" "$cfg/cosmic/net.brownhouse.CadStudio/v1"
  [ "$theme" = dark ] && echo true > "$cfg/cosmic/com.system76.CosmicTheme.Mode/v1/is_dark" \
                      || echo false > "$cfg/cosmic/com.system76.CosmicTheme.Mode/v1/is_dark"
  echo '[]' > "$cfg/cosmic/net.brownhouse.CadStudio/v1/recents"
  registry=$work/running-processes

  n=300; while [ -e /tmp/.X$n-lock ] || [ -e /tmp/.X11-unix/X$n ]; do n=$((n + 1)); done
  disp=:$n
  Xvfb $disp -screen 0 ${geom}x24 -nolisten tcp >/dev/null 2>&1 & xp=$!
  sleep 1
  echo "$(ts) launch $bin on $disp ($theme)" > "$log"
  DISPLAY=$disp WAYLAND_DISPLAY= XDG_CONFIG_HOME=$cfg LIBGL_ALWAYS_SOFTWARE=1 \
    WGPU_BACKEND=${WGPU_BACKEND:-gl} CAD_STUDIO_DRIVE=$work/e2e.drive \
    CAD_STUDIO_RUNNING_PROCESSES=$registry RUST_BACKTRACE=1 "$bin" >> "$log" 2>&1 & ap=$!
  sleep 3
  for w in $(DISPLAY=$disp xdotool search --pid $ap 2>/dev/null); do
    DISPLAY=$disp xdotool windowmove $w 0 0 windowsize $w ${geom%x*} ${geom#*x} 2>/dev/null
  done

  failed=0; done_=0; seen=0; marks=
  deadline=$((SECONDS + ${E2E_TIMEOUT:-5400}))
  note() { echo "$(ts) $*" | tee -a "$summary"; }
  on_mark() {
    local label=$1 steps
    DISPLAY=$disp xwd -root -silent | convert xwd:- png:"$work/$label.png"
    marks="$marks $label"
    steps=$(awk -v m="$label" '$1 == m { print $2 }' "$here/spec")
    if [ -z "$steps" ]; then note "FAIL mark $label is not in the spec"; failed=1; return; fi
    note "mark $label captured"
    [ "$steps" = - ] && return
    local result
    result=$(REGISTRY=$registry ZIP=$zip "$here/check.sh" "$proj" ${steps//,/ }) || failed=1
    printf '%s\n' "$result" | sed "s/^/$(ts) $label /" | tee -a "$checks" >> "$summary"
  }
  while :; do
    total=$(wc -l < "$log")
    if [ "$total" -gt "$seen" ]; then
      while IFS= read -r line; do
        case "$line" in
          *"CAD_STUDIO_DRIVE: MARK "*) on_mark "${line##*MARK }" ;;
          *"CAD_STUDIO_DRIVE: FAIL"*) note "drive ${line#*CAD_STUDIO_DRIVE: }"; failed=1; done_=1 ;;
          *"CAD_STUDIO_DRIVE: DONE"*) done_=1 ;;
          *"CAD_STUDIO_DRIVE: waitturn"*|*"CAD_STUDIO_DRIVE: param"*) note "${line#*CAD_STUDIO_DRIVE: }" ;;
          *panicked*) note "app panic: $line"; failed=1 ;;
        esac
      done < <(sed -n "$((seen + 1)),${total}p" "$log")
      seen=$total
    fi
    [ $done_ = 1 ] && break
    kill -0 $ap 2>/dev/null || { note "FAIL the app exited before the drive finished"; failed=1; break; }
    [ $SECONDS -ge $deadline ] && { note "FAIL timed out after ${E2E_TIMEOUT:-5400}s"; failed=1; break; }
    sleep 0.3
  done
  [ $failed = 1 ] && DISPLAY=$disp xwd -root -silent | convert xwd:- png:"$work/zz-failure.png"

  for want in $(grep -v '^#' "$here/spec" | awk 'NF { print $1 }'); do
    case " $marks " in *" $want "*) ;; *) note "FAIL mark $want never reached"; failed=1 ;; esac
  done

  # quit as a session logout does (SIGTERM: terminate the engine's children),
  # then the registry must hold nothing of this run
  kill -TERM $ap 2>/dev/null
  for _ in $(seq 20); do kill -0 $ap 2>/dev/null || break; sleep 0.5; done
  kill -KILL $ap 2>/dev/null; kill $xp 2>/dev/null; wait 2>/dev/null
  result=$(REGISTRY=$registry "$here/check.sh" "$proj" 7) || failed=1
  printf '%s\n' "$result" | sed "s/^/$(ts) after-quit /" | tee -a "$checks" >> "$summary"
  rm -rf "$cfg"
  if [ $failed = 0 ]; then note "$theme: PASS"; else note "$theme: FAIL"; status=1; fi
done
exit $status
