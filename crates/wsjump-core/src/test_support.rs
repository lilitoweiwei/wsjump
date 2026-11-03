//! Real filesystem fixtures shared by core behavior tests.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// A unique temporary root removed when its guard is dropped.
pub struct TempTree {
    /// Absolute sandbox path for the test's containers and files.
    pub root: PathBuf,
}

impl TempTree {
    /// Create an empty root without touching existing temporary directories.
    pub fn new(label: &str) -> Self {
        static SERIAL: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "wsjump-v3-{label}-{}-{}-{}",
            std::process::id(),
            crate::workspace::now_timestamp(),
            SERIAL.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir(&root).expect("create test root");
        Self { root }
    }

    /// Create a directory tree relative to the root.
    pub fn dir(&self, relative: &str) -> PathBuf {
        let path = self.root.join(relative);
        std::fs::create_dir_all(&path).expect("create test directory");
        path
    }

    /// Write a fixture file, creating its parent directories.
    pub fn write(&self, relative: &str, text: &str) {
        let path = self.root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).expect("create file parent");
        std::fs::write(path, text).expect("write test file");
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
