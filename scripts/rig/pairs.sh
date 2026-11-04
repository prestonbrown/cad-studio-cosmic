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
welcome-empty-light welcome.png
setup-light setup-sheet.png
set-Generale-light settings-general.png
set-Agente-light settings-agent.png
set-Python-light settings-python.png
set-Avanzate-light settings-advanced.png
about-light about.png
newproj-light new-project.png
project-dark project-model-dark.png
vcard-dark version-menu-dark.png
welcome-dark welcome-recents-dark.png
welcome-empty-dark welcome-dark.png
setup-dark setup-sheet-dark.png
newproj-dark new-project-dark.png
set-Generale-dark settings-general-dark.png
set-Agente-dark settings-agent-dark.png
set-Python-dark settings-python-dark.png
set-Avanzate-dark settings-advanced-dark.png
about-dark about-dark.png
pysheet-light python-sheet.png
agentmiss-light agent-missing.png
chaterr-light chat-error.png
builderr-light build-error.png
pysheet-dark python-sheet-dark.png
agentmiss-dark agent-missing-dark.png
chaterr-dark chat-error-dark.png
builderr-dark build-error-dark.png
parts-light parts.png
section-light section.png
playback-light playback.png
partscol-light parts-collapsed.png
parts-dark parts-dark.png
section-dark section-dark.png
playback-dark playback-dark.png
EOF
rm -f "$out/.up.png"
