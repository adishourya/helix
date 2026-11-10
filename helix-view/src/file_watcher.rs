//! Notifies the editor about changes to the directories of open files, so that
//! `auto-reload` can pick up external changes immediately instead of on its next poll.
//!
//! Directories rather than files are watched because many programs save by writing a new file
//! and renaming it over the old one, which would end a watch on the file itself. Events only
//! trigger a check: the check itself (comparing modification times) decides what changed.

use std::collections::HashMap;
use std::path::PathBuf;

use notify::event::{AccessKind, AccessMode};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

pub struct FileWatcher {
    watcher: Option<RecommendedWatcher>,
    /// Watched directories and whether they are watched recursively.
    watched: HashMap<PathBuf, bool>,
    pub(crate) events: UnboundedReceiver<()>,
}

/// Whether the event may mean that a file's contents changed. Reading files (including our
/// own reads when reloading) produces access events that must not trigger another check.
fn is_change(kind: &EventKind) -> bool {
    match kind {
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        EventKind::Access(_) => false,
        _ => true,
    }
}

impl FileWatcher {
    pub fn new() -> Self {
        let (tx, events) = unbounded_channel();
        let watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| match event {
                Ok(event) if !is_change(&event.kind) => {}
                Ok(_) => {
                    let _ = tx.send(());
                }
                Err(err) => log::warn!("file watcher error: {err}"),
            })
            .map_err(|err| log::warn!("failed to start file watcher, polling only: {err}"))
            .ok();
        Self {
            watcher,
            watched: HashMap::new(),
            events,
        }
    }

    /// Watches exactly the given directories (`true` meaning recursively), adding and removing
    /// watches as needed.
    pub fn sync(&mut self, wanted: HashMap<PathBuf, bool>) {
        let Some(watcher) = &mut self.watcher else {
            return;
        };
        self.watched.retain(|dir, recursive| {
            if wanted.get(dir) == Some(recursive) {
                return true;
            }
            if let Err(err) = watcher.unwatch(dir) {
                log::debug!("failed to unwatch {}: {err}", dir.display());
            }
            false
        });
        for (dir, recursive) in wanted {
            if self.watched.contains_key(&dir) {
                continue;
            }
            let mode = if recursive {
                RecursiveMode::Recursive
            } else {
                RecursiveMode::NonRecursive
            };
            match watcher.watch(&dir, mode) {
                Ok(()) => {
                    self.watched.insert(dir, recursive);
                }
                // e.g. the directory doesn't exist (yet); polling still covers it.
                Err(err) => log::debug!("failed to watch {}: {err}", dir.display()),
            }
        }
    }

    /// Stops watching everything.
    pub fn clear(&mut self) {
        self.sync(HashMap::new());
    }
}

impl Default for FileWatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for FileWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileWatcher")
            .field("active", &self.watcher.is_some())
            .field("watched", &self.watched)
            .finish()
    }
}
