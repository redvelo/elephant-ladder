//! A file staged on disk that is removed unless something claims it.
//!
//! Staging happens outside any transaction, so an operation that fails partway
//! must not leave bytes behind. The guard deletes its file when dropped; a caller
//! that has committed the file elsewhere disarms it first and takes the path.

use std::path::{Path, PathBuf};

#[derive(Debug)]
pub(crate) struct StagedFile {
    path: Option<PathBuf>,
}

impl StagedFile {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    /// A guard over nothing, for an outcome that reused bytes already committed.
    pub(crate) fn disarmed() -> Self {
        Self { path: None }
    }

    pub(crate) fn path(&self) -> &Path {
        self.path
            .as_deref()
            .expect("a staged file has a path until it is disarmed")
    }

    /// Gives up ownership: the file survives this guard being dropped.
    pub(crate) fn disarm(&mut self) -> Option<PathBuf> {
        self.path.take()
    }
}

impl Drop for StagedFile {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}
