use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Monotonic time used by editor and watcher policy.
pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}

/// Thread-safe notification boundary used by native services to wake a
/// frontend without knowing whether it is terminal, Winit, or headless.
pub trait FrontendWake: Send + Sync {
    fn wake(&self);
}

#[derive(Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_directory: bool,
}

/// Filesystem discovery used by interactive file selection.
pub trait FileSystem: Send + Sync {
    fn current_dir(&self) -> io::Result<PathBuf>;
    fn read_directory(&self, path: &Path) -> io::Result<Vec<DirectoryEntry>>;
    fn is_directory(&self, path: &Path) -> bool;
}

#[derive(Default)]
pub struct SystemFileSystem;

impl FileSystem for SystemFileSystem {
    fn current_dir(&self) -> io::Result<PathBuf> {
        std::env::current_dir()
    }

    fn read_directory(&self, path: &Path) -> io::Result<Vec<DirectoryEntry>> {
        fs::read_dir(path)?
            .map(|entry| {
                let entry = entry?;
                let metadata = entry.metadata()?;
                Ok(DirectoryEntry {
                    name: entry.file_name().to_string_lossy().to_string(),
                    path: entry.path(),
                    is_directory: metadata.is_dir(),
                })
            })
            .collect()
    }

    fn is_directory(&self, path: &Path) -> bool {
        path.is_dir()
    }
}
