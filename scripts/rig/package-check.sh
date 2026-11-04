#!/bin/bash
# Runs on the rig (Ubuntu 24.04), from a synced worktree root: builds the
# release packages (scripts/package/build.sh), lints the .deb, installs it
# with apt, validates the installed freedesktop files, runs
# `/usr/bin/cad-studio --version`, removes it and checks none of its files
# remain; then unpacks the AppImage and runs its binary's --version, and
# checks the tarball and the AUR PKGBUILD.
# Needs: cargo, cargo-deb, appimagetool, lintian, desktop-file-validate,
# appstreamcli, sudo apt. Packages apt pulled in as dependencies are purged
# again, so the rig ends as it began.
#   package-check.sh
set -u
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
app_id=net.brownhouse.CadStudio
pkg=cad-studio-cosmic
fail=0
log() { echo "$(date +%H:%M:%S.%3N) $*"; }
bad() { log "FAIL $*"; fail=1; }
ok() { log "ok   $*"; }

for tool in cargo cargo-deb lintian desktop-file-validate appstreamcli; do
    command -v "$tool" >/dev/null || { bad "missing tool: $tool"; exit 1; }
done
[ -x scripts/package/build.sh ] || { bad "scripts/package/build.sh is missing"; exit 1; }

scripts/package/build.sh || { bad "build.sh"; exit 1; }
version=$(cargo pkgid -p app | sed 's/.*[@#]//')
deb=$(ls dist/${pkg}_*.deb 2>/dev/null | head -n1)
[ -n "$deb" ] || { bad "no .deb in dist/"; exit 1; }

# --- .deb ---
if lintian --fail-on error "$deb"; then ok "lintian: no errors"; else bad "lintian"; fi
dpkg-deb -f "$deb" Depends | grep -q 'zip' && dpkg-deb -f "$deb" Depends | grep -q 'xdg-utils' \
    && ok "Depends carry zip and xdg-utils" || bad "Depends lack zip/xdg-utils"

before=$(mktemp); after=$(mktemp); files=$(mktemp)
trap 'rm -f "$before" "$after" "$files"' EXIT
dpkg-query -W -f '${Package}\n' | sort > "$before"
if sudo apt-get install -y "./$deb" >/dev/null; then ok "apt install"; else bad "apt install"; fi
dpkg -L "$pkg" | while read -r f; do [ -f "$f" ] && echo "$f"; done > "$files"

desktop=/usr/share/applications/$app_id.desktop
metainfo=/usr/share/metainfo/$app_id.metainfo.xml
if desktop-file-validate "$desktop"; then ok "desktop-file-validate"; else bad "desktop-file-validate"; fi
if appstreamcli validate --no-net "$metainfo"; then ok "appstreamcli validate"; else bad "appstreamcli validate"; fi
[ "$(sed -n 's/^Exec=//p' "$desktop")" = /usr/bin/cad-studio ] && ok "Exec=/usr/bin/cad-studio" || bad "Exec line"
grep -q "release version=\"$version\"" "$metainfo" && ok "metainfo release $version" || bad "metainfo release version"
for size in 128 256 512; do
    icon=/usr/share/icons/hicolor/${size}x${size}/apps/$app_id.png
    if [ -f "$icon" ] && [ "$(identify -format '%w' "$icon" 2>/dev/null)" = "$size" ]; then ok "icon ${size}px"; else bad "icon ${size}px"; fi
done
out=$(env -u DISPLAY -u WAYLAND_DISPLAY /usr/bin/cad-studio --version)
[ "$out" = "cad-studio $version" ] && ok "--version: $out" || bad "--version printed: $out"

sudo apt-get remove -y "$pkg" >/dev/null || bad "apt remove"
left=$(while read -r f; do [ -e "$f" ] && echo "$f"; done < "$files")
if [ -z "$left" ] && [ -s "$files" ]; then ok "apt remove leaves none of $(wc -l < "$files") files"; else bad "files left after remove: $left"; fi
dpkg-query -W -f '${Package}\n' | sort > "$after"
extra=$(comm -13 "$before" "$after")
[ -z "$extra" ] || sudo apt-get purge -y --autoremove $extra >/dev/null
dpkg-query -W -f '${Package}\n' | sort | diff -q "$before" - >/dev/null && ok "rig package set restored" || bad "rig package set differs"

# --- AppImage ---
appimage=dist/CAD_Studio-$version-x86_64.AppImage
work=$(mktemp -d)
if [ -x "$appimage" ] && (cd "$work" && "$root/$appimage" --appimage-extract >/dev/null); then
    ok "AppImage extracts"
    out=$(env -u DISPLAY -u WAYLAND_DISPLAY "$work/squashfs-root/AppRun" --version)
    [ "$out" = "cad-studio $version" ] && ok "AppImage --version: $out" || bad "AppImage --version printed: $out"
    desktop-file-validate "$work/squashfs-root/$app_id.desktop" && ok "AppImage desktop entry validates" || bad "AppImage desktop entry"
    [ -f "$work/squashfs-root/$app_id.png" ] && ok "AppImage icon embedded" || bad "AppImage icon"
else
    bad "AppImage missing or does not extract: $appimage"
fi
rm -rf "$work"

# --- tarball and PKGBUILD ---
tarball=dist/$pkg-$version-x86_64.tar.gz
tar -tzf "$tarball" | grep -qx 'usr/bin/cad-studio' && ok "tarball carries usr/bin/cad-studio" || bad "tarball content"
pkgbuild=packaging/aur/PKGBUILD
if [ -f "$pkgbuild" ]; then
    bash -n "$pkgbuild" && ok "PKGBUILD parses" || bad "PKGBUILD syntax"
    if command -v namcap >/dev/null; then namcap "$pkgbuild" && ok "namcap PKGBUILD" || bad "namcap"; else log "note namcap not installed; PKGBUILD lint skipped"; fi
else
    bad "packaging/aur/PKGBUILD missing"
fi

[ "$fail" = 0 ] && log "package-check PASS" || log "package-check FAILED"
exit "$fail"
