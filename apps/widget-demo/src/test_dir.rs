//! Temporary directories for the tests.

/// Prefix of the directories, to tell them apart in the temporary directory.
const PREFIX: &str = "widget-demo";

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A new, unique directory under the system temporary directory, removed on drop: also when
/// the test fails, since an assertion that panics still drops it while unwinding.
pub(crate) struct TempDir(pub(crate) PathBuf);

impl TempDir {
    /// `<PREFIX>-<label>-<pid>-<n>`: unique per process and per call.
    pub(crate) fn new(label: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("{PREFIX}-{label}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("cannot create the test directory");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
