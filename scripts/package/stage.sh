#!/bin/bash
# Lays the freedesktop part of a release package into <dest>/usr/share from
# app/resources/packaging/ and the app rasters: the desktop entry with
# @BIN@ -> $EXEC (default /usr/bin/cad-studio), the metainfo with the
# workspace version, the 128/256/512 hicolor icons and the LICENSE. The binary itself is
# placed by each packager. `just install` renders the same templates.
#   stage.sh <dest>
set -eu
dest=${1:?usage: stage.sh <dest>}
root=$(cd "$(dirname "$0")/../.." && pwd)
app_id=net.brownhouse.CadStudio
exec_path=${EXEC:-/usr/bin/cad-studio}
version=$(cd "$root" && cargo pkgid -p app | sed 's/.*[@#]//')

rm -rf "$dest"
share=$dest/usr/share
for size in 128 256 512; do mkdir -p "$share/icons/hicolor/${size}x${size}/apps"; done
mkdir -p "$share/applications" "$share/metainfo"
sed -e "s|@BIN@|$exec_path|" "$root/app/resources/packaging/$app_id.desktop" \
    > "$share/applications/$app_id.desktop"
sed -e "s|@CARGO_PKG_VERSION@|$version|" "$root/app/resources/packaging/$app_id.metainfo.xml" \
    > "$share/metainfo/$app_id.metainfo.xml"
install -m 644 "$root/app/resources/app-icon-128.png" "$share/icons/hicolor/128x128/apps/$app_id.png"
install -m 644 "$root/app/resources/app-icon.png" "$share/icons/hicolor/256x256/apps/$app_id.png"
install -m 644 "$root/app/resources/app-icon@2x.png" "$share/icons/hicolor/512x512/apps/$app_id.png"
install -D -m 644 "$root/LICENSE" "$share/licenses/cad-studio-cosmic/LICENSE"

# Debian changelog for cargo-deb (lintian requires one for a non-native
# package); it sits beside usr/, outside the installed tree.
printf '%s\n' "cad-studio-cosmic ($version-1) unstable; urgency=medium" "" \
    "  * Release $version." "" \
    " -- Preston Brown <pbrown@brown-house.net>  $(date -R)" > "$dest/changelog"
