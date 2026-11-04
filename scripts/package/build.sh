#!/bin/bash
# Builds the release packages into dist/: the .deb, the AppImage and the
# tarball (usr/ tree) that the AUR package installs, plus SHA256SUMS.
# Needs: cargo, cargo-deb, appimagetool (APPIMAGETOOL, default on PATH).
# CARGO_TARGET_DIR is honoured.
#   build.sh
set -eu
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
log() { echo "$(date +%H:%M:%S.%3N) $*"; }
app_id=net.brownhouse.CadStudio
version=$(cargo pkgid -p app | sed 's/.*[@#]//')
target=${CARGO_TARGET_DIR:-$root/target}
dist=$root/dist
mkdir -p "$dist"
rm -f "$dist"/*.deb "$dist"/*.AppImage "$dist"/*.tar.gz "$dist"/SHA256SUMS

log "cargo build --release"
cargo build -p app --release
bin=$target/release/cad-studio

log "deb"
scripts/package/stage.sh "$dist/stage"
cargo deb -p app --no-build -o "$dist/"

log "tarball"
tree=$dist/tarball
scripts/package/stage.sh "$tree"
install -D -m 755 "$bin" "$tree/usr/bin/cad-studio"
tar -C "$tree" -czf "$dist/cad-studio-cosmic-$version-x86_64.tar.gz" usr

log "appimage"
appdir=$dist/CAD_Studio.AppDir
EXEC=cad-studio scripts/package/stage.sh "$appdir"
install -D -m 755 "$bin" "$appdir/usr/bin/cad-studio"
ln -s usr/bin/cad-studio "$appdir/AppRun"
cp "$appdir/usr/share/applications/$app_id.desktop" "$appdir/"
cp "$appdir/usr/share/icons/hicolor/512x512/apps/$app_id.png" "$appdir/"
cp "$root/LICENSE" "$appdir/LICENSE"
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "${APPIMAGETOOL:-appimagetool}" --no-appstream \
    "$appdir" "$dist/CAD_Studio-$version-x86_64.AppImage"

rm -rf "$dist/stage" "$tree" "$appdir"
(cd "$dist" && sha256sum *.deb *.AppImage *.tar.gz > SHA256SUMS)
log "built: $(ls "$dist" | tr '\n' ' ')"
