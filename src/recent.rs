//! Recent-files history (VS Code-style): an ordered list of previously opened
//! file paths, most-recent first. Persisted to `recent.json`. On load, entries
//! whose files no longer exist are pruned so the menu never offers dead paths.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const RECENT_FILE: &str = "recent.json";
const MAX_RECENT: usize = 30;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RecentFiles {
    pub paths: Vec<PathBuf>,
}

impl RecentFiles {
    /// Record a freshly opened/saved file: move it to the front, de-duplicated.
    pub fn record(&mut self, path: impl AsRef<Path>) {
        let path = path.as_ref().to_path_buf();
        self.paths.retain(|p| p != &path);
        self.paths.insert(0, path);
        self.paths.truncate(MAX_RECENT);
    }

    pub fn remove(&mut self, path: impl AsRef<Path>) {
        let path = path.as_ref();
        self.paths.retain(|p| p != path);
    }

    pub fn clear(&mut self) {
        self.paths.clear();
    }

    /// Drop entries whose files no longer exist on disk. Returns true if any
    /// were removed (so the caller can persist the cleaned-up list).
    pub fn prune_missing(&mut self) -> bool {
        let before = self.paths.len();
        self.paths.retain(|p| p.is_file());
        self.paths.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_dedupes_and_orders_most_recent_first() {
        let mut r = RecentFiles::default();
        r.record("a.txt");
        r.record("b.txt");
        r.record("a.txt"); // re-open a: should jump to front, no duplicate
        assert_eq!(
            r.paths,
            vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")]
        );
    }

    #[test]
    fn record_caps_length() {
        let mut r = RecentFiles::default();
        for i in 0..50 {
            r.record(format!("f{i}.txt"));
        }
        assert_eq!(r.paths.len(), MAX_RECENT);
        assert_eq!(r.paths[0], PathBuf::from("f49.txt"));
    }

    #[test]
    fn prune_removes_nonexistent() {
        let mut r = RecentFiles::default();
        r.record("definitely-does-not-exist-12345.txt");
        assert!(r.prune_missing());
        assert!(r.paths.is_empty());
    }
}
