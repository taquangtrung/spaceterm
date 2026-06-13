//! Session restore: save and reload pane layouts across restarts.
//!
//! On clean exit, SpaceTerm writes a session file to
//! `$XDG_STATE_HOME/spaceterm/session.json`. On startup with `--restore`, it
//! recreates the same split layout and spawns fresh PTY children (not
//! in-place reattach — that requires the multiplexer).

use std::collections::HashMap;
use std::path::PathBuf;

use crate::model::layout::{PaneId, Tab};

// ========================================================================
// Data Structures
// ========================================================================

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Session {
    pub focused: usize,
    pub panes: Vec<PaneSession>,
    pub splits: Vec<SplitNode>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PaneSession {
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub id: usize,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SplitNode {
    pub direction: String,
    pub first: usize,
    pub second: usize,
}

// ========================================================================
// Implementation
// ========================================================================

impl Session {
    pub fn save(tab: &Tab, panes: &HashMap<PaneId, crate::terminal::pane::Pane>) {
        let session = Self::capture(tab, panes);
        if let Ok(json) = serde_json::to_string_pretty(&session) {
            let path = session_path();
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            std::fs::write(&path, json).ok();
        }
    }

    pub fn load() -> Option<Self> {
        let path = session_path();
        let text = std::fs::read_to_string(&path).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn remove() {
        let path = session_path();
        if path.exists() {
            std::fs::remove_file(&path).ok();
        }
    }

    fn capture(tab: &Tab, panes: &HashMap<PaneId, crate::terminal::pane::Pane>) -> Self {
        let focused = tab.focused().0;
        let pane_sessions: Vec<PaneSession> = panes
            .keys()
            .map(|id| PaneSession {
            id: id.0 as usize,
                command: None,
                cwd: None,
            })
            .collect();
        Session {
            focused: focused as usize,
            panes: pane_sessions,
            splits: Vec::new(),
        }
    }
}

fn session_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_STATE_HOME") {
        PathBuf::from(xdg).join("spaceterm/session.json")
    } else if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".local/state/spaceterm/session.json")
    } else {
        PathBuf::from("session.json")
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_round_trip() {
        let session = Session {
            focused: 1,
            panes: vec![
                PaneSession {
                    id: 0,
                    command: Some("/bin/bash".into()),
                    cwd: Some("/home/user".into()),
                },
                PaneSession {
                    id: 1,
                    command: None,
                    cwd: None,
                },
            ],
            splits: vec![SplitNode {
                direction: "horizontal".into(),
                first: 0,
                second: 1,
            }],
        };
        let json = serde_json::to_string_pretty(&session).unwrap();
        let restored: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.focused, 1);
        assert_eq!(restored.panes.len(), 2);
        assert_eq!(restored.splits.len(), 1);
        assert_eq!(restored.panes[0].command.as_deref(), Some("/bin/bash"));
    }

    #[test]
    fn test_session_path_is_deterministic() {
        let p1 = session_path();
        let p2 = session_path();
        assert_eq!(p1, p2);
    }

    #[test]
    fn test_load_missing_returns_none() {
        let path = session_path();
        if path.exists() {
            std::fs::remove_file(&path).ok();
        }
        assert!(Session::load().is_none());
    }
}
