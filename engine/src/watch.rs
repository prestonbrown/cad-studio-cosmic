//! Output-directory watcher: notify events → trailing-edge debounce →
//! mod-time dedup → `ReloadKind` channel (contract §3, FileWatcher.swift).
//!
//! Watches the passed directory non-recursively (the agent's `output/`;
//! references/, model.py and .cadstudio/ are never watched).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant, SystemTime};

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};

use crate::project::folder;

/// What the app must reload when a watched output file changes (contract §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReloadKind {
    Manifest,
    Plate,
    Motion,
    Mesh,
    Schematic,
}

/// Owns the platform watcher. Dropping it stops the watch and closes the
/// reload channel (the receiver's `recv` turns `Disconnected`).
pub struct WatchHandle {
    _watcher: RecommendedWatcher,
}

/// Watch `dir` non-recursively and report one `ReloadKind` per changed file
/// after each `debounce`-long quiet window. The platform watch is registered
/// before this returns, so writes made immediately after are never missed.
pub fn watch_directory(
    dir: &Path,
    debounce: Duration,
) -> notify::Result<(WatchHandle, Receiver<ReloadKind>)> {
    let (event_tx, event_rx) = channel::<Result<Event, notify::Error>>();
    let mut watcher = RecommendedWatcher::new(event_tx, notify::Config::default())?;
    // Change-detection boundary, captured before registration: a file whose
    // content predates the watch never emits, even when the backend leaks a
    // stale event for it (macOS FSEvents SinceNow can, for writes that
    // happened just before the stream registered; inert under inotify).
    let started_at = SystemTime::now();
    watcher.watch(dir, RecursiveMode::NonRecursive)?;

    let (reload_tx, reload_rx) = channel::<ReloadKind>();
    std::thread::Builder::new()
        .name("output-watch".into())
        .spawn(move || collect(event_rx, reload_tx, debounce, started_at))?;

    Ok((WatchHandle { _watcher: watcher }, reload_rx))
}

/// Drain notify events, one reload kind per changed file per quiet window.
fn collect(
    events: Receiver<Result<Event, notify::Error>>,
    reloads: Sender<ReloadKind>,
    debounce: Duration,
    started_at: SystemTime,
) {
    // Trailing edge: every event resets the path's timer; a path settles
    // only after `debounce` with no further events for it. A burst of writes
    // inside one window therefore collapses to exactly one reload.
    let mut quiet_at: HashMap<PathBuf, (ReloadKind, Instant)> = HashMap::new();
    // Mod-time dedup, starts empty: a settled path re-emits only when its
    // modified time moved past what was last emitted. Together with the
    // `started_at` boundary this is the "content actually changed" check
    // (upstream ProjectOutput.reload compares contentModificationDate).
    let mut emitted_mtime: HashMap<PathBuf, SystemTime> = HashMap::new();

    loop {
        let next_deadline = quiet_at.values().map(|(_, at)| *at).min();
        let message = match next_deadline {
            Some(at) => events.recv_timeout(at.saturating_duration_since(Instant::now())),
            None => events.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match message {
            Ok(Ok(event)) => {
                let settle_at = Instant::now() + debounce;
                for path in event.paths {
                    if let Some(kind) = kind_for(&path) {
                        quiet_at.insert(path, (kind, settle_at));
                    }
                }
            }
            // A backend error names no changed file; ignore it.
            Ok(Err(_)) => {}
            Err(RecvTimeoutError::Timeout) => {
                let now = Instant::now();
                let due: Vec<(PathBuf, ReloadKind)> = quiet_at
                    .iter()
                    .filter(|(_, (_, at))| *at <= now)
                    .map(|(path, (kind, _))| (path.clone(), *kind))
                    .collect();
                for (path, kind) in due {
                    quiet_at.remove(&path);
                    // Deleted or unreadable: nothing to reload from it.
                    let Ok(mtime) = std::fs::metadata(&path).and_then(|m| m.modified()) else {
                        continue;
                    };
                    // Content predates the watch: stale or leaked event. The
                    // filesystem's timestamp clock can trail the realtime
                    // clock by tens of microseconds, so a write delivered
                    // right after registration still counts as fresh — only
                    // content clearly older than the watch start is stale.
                    const MTIME_SKEW: Duration = Duration::from_millis(10);
                    if mtime + MTIME_SKEW <= started_at {
                        continue;
                    }
                    // First sight of this file, or its content clock moved.
                    let changed = emitted_mtime.insert(path, mtime) != Some(mtime);
                    if changed && reloads.send(kind).is_err() {
                        return; // receiver dropped
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => return, // watcher dropped
        }
    }
}

/// Filename → reload kind, per contract §3. Anything else (`model.step`,
/// `schematic.pdf`, references/, …) is ignored.
fn kind_for(path: &Path) -> Option<ReloadKind> {
    let name = path.file_name()?.to_str()?;
    match name {
        folder::MANIFEST => Some(ReloadKind::Manifest),
        folder::PLATE_3MF => Some(ReloadKind::Plate),
        folder::ANIMATION => Some(ReloadKind::Motion),
        folder::MODEL_3MF | folder::MODEL_STL => Some(ReloadKind::Mesh),
        folder::SCHEMATIC_PNG => Some(ReloadKind::Schematic),
        _ if name.starts_with("schematic") && name.ends_with(".svg") => Some(ReloadKind::Schematic),
        _ => None,
    }
}
