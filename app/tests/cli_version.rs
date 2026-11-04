//! The binary answers `--version` without opening a window, so packaging
//! checks can run it on a display-less host.

use std::process::Command;

#[test]
fn version_flags_print_name_and_version_and_exit_zero() {
    for flag in ["--version", "-V"] {
    let out = Command::new(env!("CARGO_BIN_EXE_cad-studio"))
        .arg(flag)
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .output()
        .expect("the cad-studio binary runs");
    assert!(out.status.success(), "exit status: {:?}", out.status);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        format!("cad-studio {}", env!("CARGO_PKG_VERSION"))
    );
    }
}
