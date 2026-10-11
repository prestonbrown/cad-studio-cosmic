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
