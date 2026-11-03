//! project.json's `app_version` is stamped from the engine's
//! `store::APP_VERSION`, because the engine writes project.json too. That
//! is only the app's version while both crates inherit `version.workspace`.

#[test]
fn engine_app_version_is_the_app_crate_version() {
    assert_eq!(
        engine::project::store::APP_VERSION,
        env!("CARGO_PKG_VERSION")
    );
}
