#!/bin/bash
# Runs ON the rig. Captures many app states in PARALLEL: each shot gets its own
# Xvfb display and its own copy of ~/.config (so light/dark and first-run state
# never collide), and every shot launches at once. Output: <outdir>/<shot>.png
# plus <shot>.log.
#
#   capall.sh <bin> <outdir> <specfile>
#
# specfile lines:  <shot> <WxH> <light|dark> <wait-sec> <drive-file> [opts]
#   opts, comma-separated:
#     fresh     start without first-run-complete (the setup sheet shows)
#     nosize    keep the app's own window sizes (default: every window fills the screen)
#     win=Text  capture only the window whose title contains Text (default: the screen)
# RECENTS (env, optional): a file copied in as each shot's recents list, so
# welcome captures show a fixed set. MAXJOBS (env, default 8): shots in flight;
# software rendering makes first paint slow when too many start at once.
# Each shot waits for the drive time, then until the frame is no longer blank.
# Drive files are CAD_STUDIO_DRIVE scripts (app/src/drive.rs); a relative path
# resolves against the spec file's directory.
set -u
bin=$1; out=$2; spec=$3
mkdir -p "$out"
ts() { date +%H:%M:%S; }
n=200
maxjobs=${MAXJOBS:-8}
while read -r shot geom mode wait drive opts; do
  opts=",${opts:-},"
  case "$drive" in /*) ;; *) drive="$(cd "$(dirname "$spec")" && pwd)/$drive" ;; esac
  case "$shot" in ''|'#'*) continue ;; esac
  while [ -e /tmp/.X$n-lock ]; do n=$((n + 1)); done
  disp=:$n; n=$((n + 1))
  (
    cfg=$(mktemp -d /tmp/capcfg.XXXX)
    cp -a ~/.config/. "$cfg/"
    mkdir -p "$cfg/cosmic/com.system76.CosmicTheme.Mode/v1"
    [ "$mode" = dark ] && echo true > "$cfg/cosmic/com.system76.CosmicTheme.Mode/v1/is_dark" \
                       || echo false > "$cfg/cosmic/com.system76.CosmicTheme.Mode/v1/is_dark"
    [ -n "${RECENTS:-}" ] && cp "$RECENTS" "$cfg/cosmic/net.brownhouse.CadStudio/v1/recents"
    case "$opts" in *,fresh,*) rm -f "$cfg/cosmic/net.brownhouse.CadStudio/v1/first-run-complete" ;; esac
    Xvfb $disp -screen 0 ${geom}x24 -nolisten tcp >/dev/null 2>&1 & xp=$!
    sleep 0.5
    echo "$(ts) launch $shot on $disp" > "$out/$shot.log"
    DISPLAY=$disp WAYLAND_DISPLAY= XDG_CONFIG_HOME=$cfg LIBGL_ALWAYS_SOFTWARE=1 \
      WGPU_BACKEND=${WGPU_BACKEND:-gl} CAD_STUDIO_DRIVE=$drive "$bin" >> "$out/$shot.log" 2>&1 & ap=$!
    sleep 2
    case "$opts" in *,nosize,*) ;; *)
      for w in $(DISPLAY=$disp xdotool search --pid $ap 2>/dev/null); do
        DISPLAY=$disp xdotool windowmove $w 0 0 windowsize $w ${geom%x*} ${geom#*x} 2>/dev/null
      done ;; esac
    sleep "$wait"
    snap() {
      if [ -n "$title" ]; then
        wid=$(DISPLAY=$disp xdotool search --name "$title" 2>/dev/null | tail -1)
        [ -n "$wid" ] && DISPLAY=$disp xwd -id "$wid" -silent | convert xwd:- png:"$out/$shot.png"
      else
        DISPLAY=$disp xwd -root -silent | convert xwd:- png:"$out/$shot.png"
      fi
    }
    title=$(echo "$opts" | sed -n 's/.*,win=\([^,]*\),.*/\1/p')
    snap
    for _ in $(seq 30); do
      [ -s "$out/$shot.png" ] && \
        [ "$(convert "$out/$shot.png" -format '%[fx:standard_deviation>0.01]' info: 2>/dev/null)" = 1 ] && break
      sleep 1; snap
    done
    echo "$(ts) captured alive=$(kill -0 $ap 2>/dev/null && echo y || echo n)" >> "$out/$shot.log"
    kill $ap 2>/dev/null; sleep 0.5; kill $xp 2>/dev/null; rm -rf "$cfg"
  ) &
  while [ "$(jobs -rp | wc -l)" -ge "$maxjobs" ]; do sleep 0.3; done
done < "$spec"
wait
for f in "$out"/*.log; do
  s=$(basename "$f" .log)
  sed '/captured/q' "$f" | grep -q -i panic && echo "PANIC $s"
  grep -q 'alive=n' "$f" && echo "DIED $s"
done
ls "$out"/*.png | wc -l
