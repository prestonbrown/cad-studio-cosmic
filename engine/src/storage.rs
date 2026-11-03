//! App-wide storage primitives: the atomic file writer every persisted
//! document goes through, and the per-user directories the app keeps its
//! settings, venv and process registry in.

use std::path::{Path, PathBuf};

use directories::ProjectDirs;

/// Write via temp-file + sync + rename so a reader never sees a torn file.
/// The temp file is `<name>.tmp` next to the target, keeping the original
/// extension visible; missing parent directories are created.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| std::io::Error::other("atomic_write: path has no file name"))?;
    let tmp = path.with_file_name(format!("{name}.tmp"));
    {
        let mut file = std::fs::File::create(&tmp)?;
        std::io::Write::write_all(&mut file, bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

/// The `cad-studio` directories: `~/.config/cad-studio` and
/// `~/.local/share/cad-studio` on Linux, `~/Library/Application
/// Support/cad-studio` for both on macOS (upstream's own choice there).
fn app_dirs() -> ProjectDirs {
    ProjectDirs::from("", "", "cad-studio")
        .expect("no home directory: cannot locate the cad-studio directories")
}

/// Where settings.json lives.
pub fn config_dir() -> PathBuf {
    app_dirs().config_dir().to_path_buf()
}

/// Where the shared venv and the process registry live.
pub fn data_dir() -> PathBuf {
    app_dirs().data_dir().to_path_buf()
}
