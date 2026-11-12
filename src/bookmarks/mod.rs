//! Bookmarks: named references (paths) to `.txt`/plain-text files the user
//! wants quick access to. Only the *path* is stored, never the file content.
//! Serialized to `bookmarks.json`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const BOOKMARKS_FILE: &str = "bookmarks.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bookmark {
    pub name: String,
    pub path: PathBuf,
    pub added_at: DateTime<Utc>,
    pub last_opened: Option<DateTime<Utc>>,
}

impl Bookmark {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("untitled")
            .to_string();
        Bookmark {
            name,
            path,
            added_at: Utc::now(),
            last_opened: None,
        }
    }

    /// True if the referenced file still exists on disk.
    pub fn exists(&self) -> bool {
        self.path.is_file()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Bookmarks {
    pub items: Vec<Bookmark>,
}

impl Bookmarks {
    /// Add a path if not already bookmarked; returns true if newly added.
    pub fn add(&mut self, path: impl AsRef<Path>) -> bool {
        let path = path.as_ref();
        if self.items.iter().any(|b| b.path == path) {
            return false;
        }
        self.items.push(Bookmark::new(path.to_path_buf()));
        true
    }

    pub fn remove(&mut self, path: impl AsRef<Path>) {
        let path = path.as_ref();
        self.items.retain(|b| b.path != path);
    }

    pub fn rename(&mut self, path: impl AsRef<Path>, new_name: &str) {
        let path = path.as_ref();
        if let Some(b) = self.items.iter_mut().find(|b| b.path == path) {
            b.name = new_name.to_string();
        }
    }

    pub fn mark_opened(&mut self, path: impl AsRef<Path>) {
        let path = path.as_ref();
        if let Some(b) = self.items.iter_mut().find(|b| b.path == path) {
            b.last_opened = Some(Utc::now());
        }
    }

    pub fn contains(&self, path: impl AsRef<Path>) -> bool {
        let path = path.as_ref();
        self.items.iter().any(|b| b.path == path)
    }
}
