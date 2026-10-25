//! `helix_vcs` provides types for working with diffs from a Version Control System (VCS).
//! Currently `git` is the only supported provider for diffs, but this architecture allows
//! for other providers to be added in the future.

use anyhow::{anyhow, bail, Result};
use arc_swap::ArcSwap;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

#[cfg(feature = "git")]
mod git;

mod diff;

pub use diff::{DiffHandle, Hunk};

mod status;

pub use status::FileChange;

/// Contains all active diff providers. Diff providers are compiled in via features. Currently
/// only `git` is supported.
#[derive(Clone)]
pub struct DiffProviderRegistry {
    providers: Vec<DiffProvider>,
}

impl DiffProviderRegistry {
    /// Get the given file from the VCS. This provides the unedited document as a "base"
    /// for a diff to be created.
    pub fn get_diff_base(&self, file: &Path, trust_full: bool) -> Option<Vec<u8>> {
        self.providers
            .iter()
            .find_map(|provider| match provider.get_diff_base(file, trust_full) {
                Ok(res) => Some(res),
                Err(err) => {
                    log::debug!("{err:#?}");
                    log::debug!("failed to open diff base for {}", file.display());
                    None
                }
            })
    }

    /// Get the current name of the current [HEAD](https://stackoverflow.com/questions/2304087/what-is-head-in-git).
    pub fn get_current_head_name(
        &self,
        file: &Path,
        trust_full: bool,
    ) -> Option<Arc<ArcSwap<Box<str>>>> {
        self.providers.iter().find_map(|provider| {
            match provider.get_current_head_name(file, trust_full) {
                Ok(res) => Some(res),
                Err(err) => {
                    log::debug!("{err:#?}");
                    log::debug!("failed to obtain current head name for {}", file.display());
                    None
                }
            }
        })
    }

    /// Returns a watcher that cheaply detects when the VCS state (HEAD, the checked out branch
    /// or the index) of the repository containing `file` changes, which invalidates the diff
    /// base and head name.
    pub fn state_watcher(&self, file: &Path) -> Option<VcsStateWatcher> {
        self.providers
            .iter()
            .find_map(|provider| provider.state_watcher(file).ok())
    }

    /// Fire-and-forget changed file iteration. Runs everything in a background task. Keeps
    /// iteration until `on_change` returns `false`.
    pub fn for_each_changed_file(
        self,
        cwd: PathBuf,
        trust_full: bool,
        f: impl Fn(Result<FileChange>) -> bool + Send + 'static,
    ) {
        tokio::task::spawn_blocking(move || {
            if self
                .providers
                .iter()
                .find_map(|provider| provider.for_each_changed_file(&cwd, trust_full, &f).ok())
                .is_none()
            {
                f(Err(anyhow!("no diff provider returns success")));
            }
        });
    }
}

impl Default for DiffProviderRegistry {
    fn default() -> Self {
        // currently only git is supported
        // TODO make this configurable when more providers are added
        let providers = vec![
            #[cfg(feature = "git")]
            DiffProvider::Git,
            DiffProvider::None,
        ];
        DiffProviderRegistry { providers }
    }
}

/// A union type that includes all types that implement [DiffProvider]. We need this type to allow
/// cloning [DiffProviderRegistry] as `Clone` cannot be used in trait objects.
///
/// `Copy` is simply to ensure the `clone()` call is the simplest it can be.
#[derive(Copy, Clone)]
enum DiffProvider {
    #[cfg(feature = "git")]
    Git,
    None,
}

impl DiffProvider {
    fn get_diff_base(&self, file: &Path, trust_full: bool) -> Result<Vec<u8>> {
        match self {
            #[cfg(feature = "git")]
            Self::Git => git::get_diff_base(file, trust_full),
            Self::None => bail!("No diff support compiled in"),
        }
    }

    fn get_current_head_name(
        &self,
        file: &Path,
        trust_full: bool,
    ) -> Result<Arc<ArcSwap<Box<str>>>> {
        match self {
            #[cfg(feature = "git")]
            Self::Git => git::get_current_head_name(file, trust_full),
            Self::None => bail!("No diff support compiled in"),
        }
    }

    fn state_watcher(&self, file: &Path) -> Result<VcsStateWatcher> {
        match self {
            #[cfg(feature = "git")]
            Self::Git => {
                let (git_dir, common_dir) = git::get_git_dirs(file)?;
                Ok(VcsStateWatcher::new(git_dir, common_dir))
            }
            Self::None => bail!("No diff support compiled in"),
        }
    }

    fn for_each_changed_file(
        &self,
        cwd: &Path,
        trust_full: bool,
        f: impl Fn(Result<FileChange>) -> bool,
    ) -> Result<()> {
        match self {
            #[cfg(feature = "git")]
            Self::Git => git::for_each_changed_file(cwd, trust_full, f),
            Self::None => bail!("No diff support compiled in"),
        }
    }
}

/// Detects changes to a git repository's HEAD, checked out branch and index by comparing the
/// modification times of the files git rewrites when they change. Checking costs a few `stat`
/// calls and reading `HEAD`, so it is cheap enough to poll.
#[derive(Debug, Clone)]
pub struct VcsStateWatcher {
    git_dir: PathBuf,
    common_dir: PathBuf,
    stamp: [Option<SystemTime>; 4],
}

impl VcsStateWatcher {
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    fn new(git_dir: PathBuf, common_dir: PathBuf) -> Self {
        let mut watcher = Self {
            git_dir,
            common_dir,
            stamp: [None; 4],
        };
        watcher.stamp = watcher.current_stamp();
        watcher
    }

    fn current_stamp(&self) -> [Option<SystemTime>; 4] {
        let mtime = |path: &Path| path.metadata().and_then(|meta| meta.modified()).ok();
        let head = self.git_dir.join("HEAD");
        // A commit updates the file of the checked out branch rather than HEAD itself.
        let branch = std::fs::read_to_string(&head).ok().and_then(|head| {
            let branch = head.strip_prefix("ref:")?.trim();
            mtime(&self.common_dir.join(branch))
        });
        [
            mtime(&head),
            branch,
            mtime(&self.common_dir.join("packed-refs")),
            mtime(&self.git_dir.join("index")),
        ]
    }

    /// Returns `true` if the repository state changed since the watcher was created or this
    /// function last returned `true`.
    pub fn changed(&mut self) -> bool {
        let stamp = self.current_stamp();
        if stamp == self.stamp {
            return false;
        }
        self.stamp = stamp;
        true
    }
}
