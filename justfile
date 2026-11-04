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

# Install user-local: the cad-studio binary to ~/.local/bin, the desktop
# entry to ~/.local/share/applications, metainfo to ~/.local/share/metainfo,
# and the 128/256/512 hicolor icons to ~/.local/share/icons/hicolor
# (libcosmic's icon lookup scans the theme directory, so no
# gtk-update-icon-cache pass is needed). Builds the app binary first —
# release by default; `just install dev` installs the debug build (cargo
# profile names; dev artifacts land in target/debug under $CARGO_TARGET_DIR,
# default ./target). scripts/package/stage.sh renders the desktop entry
# (Exec rewritten to the absolute installed path: a session PATH without
# ~/.local/bin would otherwise fail to launch from the dock), the metainfo
# and the icons from app/resources/packaging/.
# Builds and lays the freedesktop packaging into ~/.local.
install profile="release":
    cargo build -p app --profile {{profile}}
    install -D -m 755 \
        ${CARGO_TARGET_DIR:-target}/{{ if profile == "dev" { "debug" } else { profile } }}/cad-studio \
        $HOME/.local/bin/cad-studio
    stage=$(mktemp -d) \
        && EXEC=$HOME/.local/bin/cad-studio scripts/package/stage.sh "$stage" \
        && cp -R "$stage"/usr/share/. $HOME/.local/share/ \
        && rm -rf "$stage"

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
