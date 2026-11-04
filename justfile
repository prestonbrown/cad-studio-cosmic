default: build

build:
    cargo build --workspace

test:
    cargo test --workspace

run:
    cargo run -p app

render-spike:
    cargo run -p viewer --example spike_window

headless:
    cargo test -p viewer --test render_headless

# ─── user-local install (freedesktop packaging) ────────────────────────────

app-id := "net.brownhouse.CadStudio"
# The workspace version (app's CARGO_PKG_VERSION) substituted into the
# installed metainfo's release element.
version := `cargo pkgid -p app | sed 's/.*[@#]//'`

# Install user-local: the cad-studio binary to ~/.local/bin, the desktop
# entry to ~/.local/share/applications, metainfo to ~/.local/share/metainfo,
# and the 128/256/512 hicolor icons to ~/.local/share/icons/hicolor
# (libcosmic's icon lookup scans the theme directory, so no
# gtk-update-icon-cache pass is needed). Builds the app binary first —
# release by default; `just install dev` installs the debug build (cargo
# profile names; dev artifacts land in target/debug under $CARGO_TARGET_DIR,
# default ./target). The desktop
# entry's Exec and the metainfo's version are rewritten at install time
# (the @BIN@ / @CARGO_PKG_VERSION@ placeholders in
# app/resources/packaging/): a session PATH without ~/.local/bin would
# otherwise fail to launch from the dock.
# Builds and lays the freedesktop packaging into ~/.local.
install profile="release":
    cargo build -p app --profile {{profile}}
    mkdir -p $HOME/.local/bin \
        $HOME/.local/share/applications \
        $HOME/.local/share/metainfo \
        $HOME/.local/share/icons/hicolor/128x128/apps \
        $HOME/.local/share/icons/hicolor/256x256/apps \
        $HOME/.local/share/icons/hicolor/512x512/apps
    install -m 755 \
        ${CARGO_TARGET_DIR:-target}/{{ if profile == "dev" { "debug" } else { profile } }}/cad-studio \
        $HOME/.local/bin/cad-studio
    sed -e "s|@BIN@|$HOME/.local/bin/cad-studio|" \
        app/resources/packaging/{{app-id}}.desktop \
        > $HOME/.local/share/applications/{{app-id}}.desktop
    sed -e "s|@CARGO_PKG_VERSION@|{{version}}|" \
        app/resources/packaging/{{app-id}}.metainfo.xml \
        > $HOME/.local/share/metainfo/{{app-id}}.metainfo.xml
    # app-icon-128.png is the checked-in 128px downscale of app-icon@2x.png;
    # 256/512 install straight from the two app rasters under the icon-name
    # law (the name must equal APP_ID for the theme lookup).
    install -m 644 app/resources/app-icon-128.png \
        $HOME/.local/share/icons/hicolor/128x128/apps/{{app-id}}.png
    install -m 644 app/resources/app-icon.png \
        $HOME/.local/share/icons/hicolor/256x256/apps/{{app-id}}.png
    install -m 644 app/resources/app-icon@2x.png \
        $HOME/.local/share/icons/hicolor/512x512/apps/{{app-id}}.png

# Remove exactly what `install` laid down. The shared ~/.local/share
# directories (applications, metainfo, the hicolor tree) stay — other
# entries live in them.
# Uninstalls the user-local packaging (files only, shared dirs stay).
uninstall:
    rm -f $HOME/.local/bin/cad-studio \
        $HOME/.local/share/applications/{{app-id}}.desktop \
        $HOME/.local/share/metainfo/{{app-id}}.metainfo.xml \
        $HOME/.local/share/icons/hicolor/128x128/apps/{{app-id}}.png \
        $HOME/.local/share/icons/hicolor/256x256/apps/{{app-id}}.png \
        $HOME/.local/share/icons/hicolor/512x512/apps/{{app-id}}.png
