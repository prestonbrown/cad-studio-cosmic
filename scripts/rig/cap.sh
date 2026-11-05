#!/bin/bash
# Runs ON the rig (cad-dev). One private Xvfb per capture, no window manager
# (the app draws its own CSD header, as on COSMIC), window sized to the
# screen, xwd -> png in ~/caps/<name>.png.
#
#   cap.sh <name> <WxH> <drive-file> <wait-sec> [light|dark] [xdo-file]
#
# BIN overrides the binary (default ~/cad-studio-cosmic/target/release/cad-studio).
# drive-file is a CAD_STUDIO_DRIVE script (app/src/drive.rs). xdo-file holds
# xdotool commands run once the window is up, one per line, e.g.
#   key ctrl+alt+i
#   mousemove 700 500 click 1
#   sleep 1
# (Xvfb has no input gating, unlike COSMIC, so real X events work here; the
# window gets X input focus before the xdo file runs, or keys never land.)
set -u
name=$1; geom=$2; drive=$3; wait=$4; mode=${5:-light}; xdo=${6:-}
ts() { date +%H:%M:%S; }
n=100; while [ -e /tmp/.X$n-lock ] || [ -e /tmp/.X11-unix/X$n ]; do n=$((n + 1)); done
disp=:$n
mkdir -p ~/caps ~/.config/cosmic/com.system76.CosmicTheme.Mode/v1
[ "$mode" = dark ] && echo true > ~/.config/cosmic/com.system76.CosmicTheme.Mode/v1/is_dark \
                   || echo false > ~/.config/cosmic/com.system76.CosmicTheme.Mode/v1/is_dark
Xvfb $disp -screen 0 ${geom}x24 -nolisten tcp >/dev/null 2>&1 & xp=$!
sleep 1
bin=${BIN:-$HOME/cad-studio-cosmic/target/release/cad-studio}
: > ~/caps/$name.log
echo "$(ts) launch $bin on $disp" >> ~/caps/$name.log
# lavapipe's X11 surface offers only Opaque/Inherit alpha; libcosmic asks for
# PreMultiplied, so the GL backend (llvmpipe) is the default here.
DISPLAY=$disp WAYLAND_DISPLAY= LIBGL_ALWAYS_SOFTWARE=1 WGPU_BACKEND=${WGPU_BACKEND:-gl} \
  CAD_STUDIO_DRIVE=$drive RUST_BACKTRACE=1 $bin >> ~/caps/$name.log 2>&1 & ap=$!
sleep 3
for w in $(DISPLAY=$disp xdotool search --pid $ap 2>/dev/null); do
  DISPLAY=$disp xdotool windowmove $w 0 0 windowsize $w ${geom%x*} ${geom#*x} 2>/dev/null
done
if [ -n "$xdo" ]; then
  sleep 2
  # With no window manager nothing gives the window X input focus, and an
  # unfocused winit window receives no key events: focus it so xdo keys land.
  for w in $(DISPLAY=$disp xdotool search --onlyvisible --pid $ap 2>/dev/null); do
    DISPLAY=$disp xdotool windowfocus $w 2>/dev/null
  done
  sleep 1
  while read -r line; do
    case "$line" in ''|'#'*) ;; sleep\ *) sleep ${line#sleep } ;; *) DISPLAY=$disp xdotool $line ;; esac
  done < "$xdo"
fi
sleep $wait
DISPLAY=$disp xwd -root -silent | convert xwd:- png:$HOME/caps/$name.png
echo "$(ts) captured $name.png alive=$(kill -0 $ap 2>/dev/null && echo y || echo n)" >> ~/caps/$name.log
kill $ap 2>/dev/null; sleep 1; kill $xp 2>/dev/null; wait 2>/dev/null
sed "/captured/q" ~/caps/$name.log | grep -m3 -i panic
tail -1 ~/caps/$name.log
