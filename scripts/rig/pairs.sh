#!/bin/bash
# Runs on the Mac. Puts each capall.sh capture next to its upstream gallery
# counterpart: upstream converted from its display ICC profile to sRGB and
# halved from retina 2x, port at 1x, side by side on a neutral gray.
#
#   pairs.sh <capture-dir> <out-dir>
set -eu
caps=$1; out=$2
up=$(cd "$(dirname "$0")/../../shots/upstream" && pwd)
mkdir -p "$out"
while read -r shot upstream; do
  [ -f "$caps/$shot.png" ] && [ -f "$up/$upstream" ] || { echo "skip $shot"; continue; }
  magick "$up/$upstream" -colorspace sRGB -resize 50% +repage "$out/.up.png"
  magick "$out/.up.png" "$caps/$shot.png" -background '#808080' -gravity north +smush 8 \
    "$out/$shot.png"
  echo "$out/$shot.png"
done <<'EOF'
project-light project-model.png
vcard-light version-menu.png
welcome-light welcome-recents.png
setup-light setup-sheet.png
set-Generale-light settings-general.png
set-Agente-light settings-agent.png
set-Python-light settings-python.png
set-Avanzate-light settings-advanced.png
about-light about.png
EOF
rm -f "$out/.up.png"
