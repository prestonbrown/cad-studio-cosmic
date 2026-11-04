#!/bin/bash
# Runs on the rig, from a synced worktree root: `just install`, validates the
# installed freedesktop packaging, then `just uninstall` and checks the file
# tree under ~/.local is exactly what it was before.
#   install-check.sh [profile]      profile defaults to dev (cargo profile names)
set -u
profile=${1:-dev}
app_id=net.brownhouse.CadStudio
local_dir=$HOME/.local
fail=0
log() { echo "$(date +%H:%M:%S.%3N) $*"; }
bad() { log "FAIL $*"; fail=1; }
ok() { log "ok   $*"; }

before=$(mktemp); after=$(mktemp)
trap 'rm -f "$before" "$after"' EXIT
find "$local_dir" | sort > "$before"

just install "$profile" || { bad "just install $profile"; exit 1; }

desktop=$local_dir/share/applications/$app_id.desktop
metainfo=$local_dir/share/metainfo/$app_id.metainfo.xml

if desktop-file-validate "$desktop"; then ok "desktop-file-validate"; else bad "desktop-file-validate"; fi
# The project has no public homepage yet, so url-homepage-missing is the one
# tag allowed to remain.
validate_out=$(appstreamcli validate --no-net "$metainfo" 2>&1)
echo "$validate_out"
if echo "$validate_out" | grep -E '^[WEIP]: ' | grep -qv 'url-homepage-missing'; then
    bad "appstreamcli validate"
else
    ok "appstreamcli validate (homepage tag excepted)"
fi

for size in 128 256 512; do
    icon=$local_dir/share/icons/hicolor/${size}x${size}/apps/$app_id.png
    if [ -f "$icon" ] && [ "$(identify -format '%w' "$icon")" = "$size" ]; then ok "icon ${size}px"; else bad "icon ${size}px at $icon"; fi
done

exec_line=$(sed -n 's/^Exec=//p' "$desktop")
exec_bin=${exec_line%% *}
case "$exec_bin" in
    /*) if [ -x "$exec_bin" ]; then ok "Exec is an absolute, executable path: $exec_bin"; else bad "Exec $exec_bin not executable"; fi ;;
    *) bad "Exec is not absolute: $exec_line" ;;
esac
if grep -q '@[A-Z_]*@' "$desktop" "$metainfo"; then bad "unsubstituted placeholder left in installed files"; else ok "no placeholders left"; fi

just uninstall
find "$local_dir" | sort > "$after"
if diff "$before" "$after"; then ok "uninstall restores ~/.local exactly"; else bad "uninstall left a different tree (diff above)"; fi

[ "$fail" = 0 ] && log "install-check PASS" || log "install-check FAILED"
exit "$fail"
