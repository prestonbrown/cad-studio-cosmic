use engine::watch::{ReloadKind, watch_directory};
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

fn tmp_output() -> PathBuf {
    let d = std::env::temp_dir().join(format!("cad-watch-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&d).unwrap();
    d
}
fn write(path: &std::path::Path, bytes: &[u8]) {
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(bytes).unwrap();
}

const FAST: Duration = Duration::from_millis(30);

fn drain_until(
    rx: &std::sync::mpsc::Receiver<ReloadKind>,
    want: ReloadKind,
    deadline_ms: u64,
) -> bool {
    let mut seen = std::collections::BTreeSet::new();
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_millis(deadline_ms) {
        while let Ok(k) = rx.try_recv() {
            seen.insert(k);
        }
        if seen.contains(&want) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    false
}

/// Same bounded poll for a whole set of wanted kinds; returns everything seen
/// so each kind can be asserted by name. A per-kind `drain_until` sequence
/// would not work here: each call consumes the channel, so a sibling kind
/// drained by an earlier call is gone when its own turn comes.
fn drain_until_all(
    rx: &std::sync::mpsc::Receiver<ReloadKind>,
    wants: &[ReloadKind],
    deadline_ms: u64,
) -> std::collections::BTreeSet<ReloadKind> {
    let mut seen = std::collections::BTreeSet::new();
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_millis(deadline_ms) {
        while let Ok(k) = rx.try_recv() {
            seen.insert(k);
        }
        if wants.iter().all(|w| seen.contains(w)) {
            return seen;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    seen
}

#[test]
fn maps_files_to_reload_kinds() {
    let d = tmp_output();
    let (_h, rx) = watch_directory(&d, FAST).unwrap();
    write(&d.join("manifest.json"), b"{}");
    write(&d.join("plate.3mf"), b"x");
    write(&d.join("animation.json"), b"{}");
    write(&d.join("model.3mf"), b"x");
    write(&d.join("model.stl"), b"x");
    write(&d.join("schematic.svg"), b"x");
    write(&d.join("schematic-2.svg"), b"x");
    write(&d.join("schematic.png"), b"x");
    write(&d.join("references-dont-care.txt"), b"x");
    let wants = [
        ReloadKind::Manifest,
        ReloadKind::Plate,
        ReloadKind::Motion,
        ReloadKind::Mesh,
        ReloadKind::Schematic,
    ];
    let kinds = drain_until_all(&rx, &wants, 5000);
    for want in wants {
        assert!(kinds.contains(&want), "missing reload kind {want:?}");
    }
}

#[test]
fn storm_collapses_to_one_reload_per_file() {
    let d = tmp_output();
    let (_h, rx) = watch_directory(&d, FAST).unwrap();
    for _ in 0..25 {
        write(&d.join("model.3mf"), b"storm");
        write(&d.join("manifest.json"), b"{}");
    }
    assert!(
        drain_until(&rx, ReloadKind::Mesh, 5000),
        "the storm's single Mesh reload never arrived"
    );
    assert!(
        !drain_until(&rx, ReloadKind::Mesh, 300),
        "trailing-edge debounce + mod-time dedup collapse the storm"
    );
}

#[test]
fn second_write_after_quiet_emits_again() {
    let d = tmp_output();
    let (_h, rx) = watch_directory(&d, FAST).unwrap();
    write(&d.join("model.3mf"), b"one");
    assert!(
        drain_until(&rx, ReloadKind::Mesh, 5000),
        "first write never reloaded"
    );
    write(&d.join("model.3mf"), b"two");
    assert!(
        drain_until(&rx, ReloadKind::Mesh, 5000),
        "second write after quiet never reloaded"
    );
    assert!(
        !drain_until(&rx, ReloadKind::Mesh, 300),
        "a third Mesh reload arrived for two writes"
    );
}

#[test]
fn untouched_files_never_reload() {
    let d = tmp_output();
    write(&d.join("model.3mf"), b"initial");
    // the stale-check suppresses writes up to 10ms before registration (the
    // fs-clock skew margin); on macOS FSEvents a pre-write this close can
    // land inside that band and pass the test vacuously
    std::thread::sleep(Duration::from_millis(15));
    let (_h, rx) = watch_directory(&d, FAST).unwrap();
    write(&d.join("manifest.json"), b"{}");
    std::thread::sleep(Duration::from_millis(200));
    while let Ok(k) = rx.try_recv() {
        assert_ne!(
            k,
            ReloadKind::Mesh,
            "model.3mf never changed after watch start"
        );
    }
}
