//! Session persistence: "open where you left off". Stores every open tab —
//! its file path (if any) and, for untitled or unsaved tabs, the live buffer
//! text itself (since there's no file to re-read it from) — so closing and
//! reopening the app restores exactly what was there, including unsaved
//! Untitled scratch text.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const SESSION_FILE: &str = "session.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionTab {
    pub path: Option<PathBuf>,
    /// Present when the tab has no path, or has unsaved changes — the live
    /// buffer text at save time, so nothing is lost across a restart.
    pub unsaved_content: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub tabs: Vec<SessionTab>,
    pub active: usize,
}

impl Session {
    /// Drop tabs that point to files which no longer exist on disk and have
    /// no unsaved content to fall back on — these are "ghosts" that would
    /// otherwise keep reappearing on every restart. Returns true if any were
    /// removed (so the caller can re-persist the cleaned-up session).
    pub fn prune_missing(&mut self) -> bool {
        let before = self.tabs.len();
        self.tabs.retain(|t| {
            t.unsaved_content.is_some() || t.path.as_ref().is_none_or(|p| p.is_file())
        });
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len().saturating_sub(1);
        }
        self.tabs.len() != before
    }

    /// Drop untitled tabs holding nothing but whitespace. An empty scratch
    /// buffer carries nothing worth restoring, and leaving it in means every
    /// launch reopens a blank "Untitled" alongside the real work. Returns true
    /// if any were removed.
    pub fn prune_blank_untitled(&mut self) -> bool {
        let keep: Vec<bool> = self
            .tabs
            .iter()
            .map(|t| {
                t.path.is_some()
                    || t.unsaved_content
                        .as_ref()
                        .is_some_and(|s| !s.trim().is_empty())
            })
            .collect();
        if keep.iter().all(|k| *k) {
            return false;
        }
        // Follow the active tab through the removals instead of just clamping,
        // so dropping a blank tab to its left doesn't shift the selection onto
        // a different document.
        self.active = keep.iter().take(self.active).filter(|k| **k).count();
        let mut keep = keep.into_iter();
        self.tabs.retain(|_| keep.next().unwrap_or(true));
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len().saturating_sub(1);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prune_removes_missing_file_tabs_without_unsaved_content() {
        let mut s = Session {
            tabs: vec![
                SessionTab {
                    path: Some(PathBuf::from("definitely-missing-xyz.txt")),
                    unsaved_content: None,
                },
                SessionTab { path: None, unsaved_content: Some("scratch".into()) },
            ],
            active: 0,
        };
        assert!(s.prune_missing());
        assert_eq!(s.tabs.len(), 1);
        assert_eq!(s.tabs[0].unsaved_content.as_deref(), Some("scratch"));
    }

    #[test]
    fn prune_keeps_missing_file_if_unsaved_content_present() {
        let mut s = Session {
            tabs: vec![SessionTab {
                path: Some(PathBuf::from("missing-but-has-content.txt")),
                unsaved_content: Some("still here".into()),
            }],
            active: 0,
        };
        assert!(!s.prune_missing());
        assert_eq!(s.tabs.len(), 1);
    }

    #[test]
    fn prune_blank_untitled_drops_empty_scratch_tabs() {
        let mut s = Session {
            tabs: vec![
                SessionTab { path: None, unsaved_content: Some(String::new()) },
                SessionTab { path: None, unsaved_content: Some("  \n ".into()) },
                SessionTab { path: None, unsaved_content: None },
                SessionTab { path: None, unsaved_content: Some("keep me".into()) },
            ],
            active: 3,
        };
        assert!(s.prune_blank_untitled());
        assert_eq!(s.tabs.len(), 1);
        assert_eq!(s.tabs[0].unsaved_content.as_deref(), Some("keep me"));
        assert_eq!(s.active, 0);
    }

    #[test]
    fn prune_blank_untitled_follows_the_active_tab() {
        // [blank, a.txt, (b.txt)] active=2 -> [a.txt, (b.txt)] active=1
        let mut s = Session {
            tabs: vec![
                SessionTab { path: None, unsaved_content: Some(String::new()) },
                SessionTab { path: Some(PathBuf::from("a.txt")), unsaved_content: None },
                SessionTab { path: Some(PathBuf::from("b.txt")), unsaved_content: None },
            ],
            active: 2,
        };
        assert!(s.prune_blank_untitled());
        assert_eq!(s.tabs.len(), 2);
        assert_eq!(s.active, 1);
    }

    #[test]
    fn prune_blank_untitled_keeps_saved_files_even_when_empty() {
        let mut s = Session {
            tabs: vec![SessionTab {
                path: Some(PathBuf::from("empty.txt")),
                unsaved_content: None,
            }],
            active: 0,
        };
        assert!(!s.prune_blank_untitled());
        assert_eq!(s.tabs.len(), 1);
    }

    #[test]
    fn prune_clamps_active_index() {
        let mut s = Session {
            tabs: vec![SessionTab {
                path: Some(PathBuf::from("missing-xyz-2.txt")),
                unsaved_content: None,
            }],
            active: 0,
        };
        s.prune_missing();
        assert_eq!(s.active, 0);
        assert!(s.tabs.is_empty());
    }
}
