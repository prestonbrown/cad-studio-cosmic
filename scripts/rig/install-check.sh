#!/bin/bash
# Runs on the rig, from a synced worktree root: `just install`, validates the
# installed freedesktop packaging, then `just uninstall` and checks the file
# tree under ~/.local holds exactly the files it held before (the shared
# directories stay by design, so only files are compared).
# Needs: just, cargo, desktop-file-validate, appstreamcli, ImageMagick identify.
#   install-check.sh [profile]      profile defaults to dev (cargo profile names)
set -u
profile=${1:-dev}
app_id=net.brownhouse.CadStudio
local_dir=$HOME/.local
fail=0
log() { echo "$(date +%H:%M:%S.%3N) $*"; }
bad() { log "FAIL $*"; fail=1; }
ok() { log "ok   $*"; }

for tool in just desktop-file-validate appstreamcli identify; do
    command -v "$tool" >/dev/null || { bad "missing tool: $tool"; exit 1; }
done

before=$(mktemp); after=$(mktemp)
trap 'rm -f "$before" "$after"' EXIT
find "$local_dir" -type f | sort > "$before"

just install "$profile" || { bad "just install $profile"; exit 1; }

desktop=$local_dir/share/applications/$app_id.desktop
metainfo=$local_dir/share/metainfo/$app_id.metainfo.xml

if desktop-file-validate "$desktop"; then ok "desktop-file-validate"; else bad "desktop-file-validate"; fi
# The project has no public homepage yet, so url-homepage-missing is the one
# tag allowed to remain.
validate_out=$(appstreamcli validate --no-net "$metainfo" 2>&1)
validate_rc=$?
echo "$validate_out"
if [ "$validate_rc" = 0 ]; then
    ok "appstreamcli validate"
elif echo "$validate_out" | grep -E '^[WEIP]: ' | grep -qv 'url-homepage-missing' \
    || ! echo "$validate_out" | grep -q '^[WEIP]: .*url-homepage-missing'; then
    bad "appstreamcli validate (exit $validate_rc)"
else
    ok "appstreamcli validate (exit $validate_rc, only the homepage tag)"
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
find "$local_dir" -type f | sort > "$after"
if diff "$before" "$after"; then ok "uninstall restores ~/.local exactly"; else bad "uninstall left a different tree (diff above)"; fi

[ "$fail" = 0 ] && log "install-check PASS" || log "install-check FAILED"
exit "$fail"
