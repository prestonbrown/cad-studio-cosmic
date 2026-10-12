use engine::watch::{watch_directory, ReloadKind};
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
    std::thread::sleep(Duration::from_millis(200));
    let mut kinds = std::collections::BTreeSet::new();
    while let Ok(k) = rx.try_recv() { kinds.insert(k); }
    assert!(kinds.contains(&ReloadKind::Manifest));
    assert!(kinds.contains(&ReloadKind::Plate));
    assert!(kinds.contains(&ReloadKind::Motion));
    assert!(kinds.contains(&ReloadKind::Mesh));
    assert!(kinds.contains(&ReloadKind::Schematic));
}

#[test]
fn storm_collapses_to_one_reload_per_file() {
    let d = tmp_output();
    let (_h, rx) = watch_directory(&d, FAST).unwrap();
    for _ in 0..25 {
        write(&d.join("model.3mf"), b"storm");
        write(&d.join("manifest.json"), b"{}");
    }
    std::thread::sleep(Duration::from_millis(300));
    let mut mesh = 0;
    while let Ok(k) = rx.try_recv() { if k == ReloadKind::Mesh { mesh += 1; } }
    assert_eq!(mesh, 1, "trailing-edge debounce + mod-time dedup collapse the storm");
}

#[test]
fn second_write_after_quiet_emits_again() {
    let d = tmp_output();
    let (_h, rx) = watch_directory(&d, FAST).unwrap();
    write(&d.join("model.3mf"), b"one");
    std::thread::sleep(Duration::from_millis(150));
    write(&d.join("model.3mf"), b"two");
    std::thread::sleep(Duration::from_millis(150));
    let mut mesh = 0;
    while let Ok(k) = rx.try_recv() { if k == ReloadKind::Mesh { mesh += 1; } }
    assert_eq!(mesh, 2);
}

#[test]
fn untouched_files_never_reload() {
    let d = tmp_output();
    write(&d.join("model.3mf"), b"initial");
    let (_h, rx) = watch_directory(&d, FAST).unwrap();
    write(&d.join("manifest.json"), b"{}");
    std::thread::sleep(Duration::from_millis(200));
    while let Ok(k) = rx.try_recv() {
        assert_ne!(k, ReloadKind::Mesh, "model.3mf never changed after watch start");
    }
}
