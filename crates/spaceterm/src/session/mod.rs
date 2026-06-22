//! Session restore: save and reload pane layouts across restarts.
//!
//! On clean exit, SpaceTerm writes a session file to
//! `$XDG_STATE_HOME/spaceterm/session.json`. On the next launch SpaceTerm
//! automatically restores the split layout and reopens each pane at its
//! last working directory (PTY children are spawned fresh — not reattached).

use std::collections::HashMap;
use std::path::PathBuf;

use crate::model::layout::{Direction, LayoutTree, PaneId, Tab};

// ========================================================================
// Data Structures
// ========================================================================

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Session {
    /// Index of the tab that was active when the session was saved.
    #[serde(default)]
    pub active_tab: usize,
    /// Focused pane id within the active tab (legacy single-tab field).
    pub focused: usize,
    pub panes: Vec<PaneSession>,
    /// Layout of the active tab (legacy single-tab field).
    pub layout: SessionTree,
    /// All tabs. When non-empty this takes precedence over `focused`/`layout`.
    #[serde(default)]
    pub tabs: Vec<TabSnapshot>,
}

/// Snapshot of one tab for multi-tab session persistence.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TabSnapshot {
    pub focused: usize,
    pub layout: SessionTree,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PaneSession {
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub id: usize,
}

/// Serializable mirror of [`LayoutTree`].
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "t")]
pub enum SessionTree {
    Pane { id: usize },
    Split {
        dir: String,
        ratio: f32,
        first: Box<SessionTree>,
        second: Box<SessionTree>,
    },
}

// ========================================================================
// Implementation
// ========================================================================

impl Session {
    pub fn save(
        tabs: &[Tab],
        active_tab: usize,
        panes: &HashMap<PaneId, crate::terminal::pane::Pane>,
    ) {
        let session = Self::capture(tabs, active_tab, panes);
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

    fn capture(
        tabs: &[Tab],
        active_tab: usize,
        panes: &HashMap<PaneId, crate::terminal::pane::Pane>,
    ) -> Self {
        let active = tabs.get(active_tab).or_else(|| tabs.first());
        let (legacy_focused, legacy_layout) = if let Some(tab) = active {
            (
                tab.focused().0 as usize,
                layout_tree_to_session(&tab.export_tree()),
            )
        } else {
            (0, SessionTree::Pane { id: 0 })
        };
        let pane_sessions: Vec<PaneSession> = panes
            .iter()
            .map(|(id, pane)| PaneSession {
                id: id.0 as usize,
                command: Some(pane.shell_command().to_string()),
                cwd: pane.cwd(),
            })
            .collect();
        let tab_snapshots: Vec<TabSnapshot> = tabs
            .iter()
            .map(|tab| TabSnapshot {
                focused: tab.focused().0 as usize,
                layout: layout_tree_to_session(&tab.export_tree()),
            })
            .collect();
        Session {
            active_tab,
            focused: legacy_focused,
            panes: pane_sessions,
            layout: legacy_layout,
            tabs: tab_snapshots,
        }
    }

    /// Reconstruct all tabs from this session snapshot. Returns the tabs vec,
    /// the active tab index, and a map of `PaneId -> (command, cwd)`.
    pub fn into_tabs(
        self,
    ) -> (Vec<Tab>, usize, HashMap<PaneId, (Option<String>, Option<String>)>) {
        let pane_map: HashMap<PaneId, (Option<String>, Option<String>)> = self
            .panes
            .into_iter()
            .map(|p| (PaneId(p.id as u64), (p.command, p.cwd)))
            .collect();

        let tabs = if !self.tabs.is_empty() {
            self.tabs
                .into_iter()
                .map(|snap| {
                    let focused = PaneId(snap.focused as u64);
                    let layout = session_to_layout_tree(&snap.layout);
                    Tab::from_tree(layout, focused)
                })
                .collect()
        } else {
            // Legacy single-tab session.
            let focused = PaneId(self.focused as u64);
            let layout = session_to_layout_tree(&self.layout);
            vec![Tab::from_tree(layout, focused)]
        };

        let active_tab = self.active_tab.min(tabs.len().saturating_sub(1));
        (tabs, active_tab, pane_map)
    }

    /// Reconstruct a `Tab` from this session snapshot (single-tab compat).
    pub fn into_tab(self) -> (Tab, PaneId, HashMap<PaneId, (Option<String>, Option<String>)>) {
        let focused_id = PaneId(self.focused as u64);
        let (mut tabs, _, pane_map) = self.into_tabs();
        let tab = tabs.remove(0);
        (tab, focused_id, pane_map)
    }
}

// ========================================================================
// Helpers
// ========================================================================

fn layout_tree_to_session(tree: &LayoutTree) -> SessionTree {
    match tree {
        LayoutTree::Pane(id) => SessionTree::Pane { id: id.0 as usize },
        LayoutTree::Split { direction, ratio, first, second } => SessionTree::Split {
            dir: match direction {
                Direction::Horizontal => "h".to_string(),
                Direction::Vertical => "v".to_string(),
            },
            ratio: *ratio,
            first: Box::new(layout_tree_to_session(first)),
            second: Box::new(layout_tree_to_session(second)),
        },
    }
}

fn session_to_layout_tree(tree: &SessionTree) -> LayoutTree {
    match tree {
        SessionTree::Pane { id } => LayoutTree::Pane(PaneId(*id as u64)),
        SessionTree::Split { dir, ratio, first, second } => LayoutTree::Split {
            direction: if dir == "h" { Direction::Horizontal } else { Direction::Vertical },
            ratio: *ratio,
            first: Box::new(session_to_layout_tree(first)),
            second: Box::new(session_to_layout_tree(second)),
        },
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
            active_tab: 0,
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
            layout: SessionTree::Split {
                dir: "v".into(),
                ratio: 0.5,
                first: Box::new(SessionTree::Pane { id: 0 }),
                second: Box::new(SessionTree::Pane { id: 1 }),
            },
            tabs: vec![],
        };
        let json = serde_json::to_string_pretty(&session).unwrap();
        let restored: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.focused, 1);
        assert_eq!(restored.panes.len(), 2);
        assert_eq!(restored.panes[0].command.as_deref(), Some("/bin/bash"));
        assert!(matches!(restored.layout, SessionTree::Split { .. }));
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

    #[test]
    fn test_into_tab_single_pane() {
        let session = Session {
            active_tab: 0,
            focused: 0,
            panes: vec![PaneSession { id: 0, command: Some("/bin/zsh".into()), cwd: None }],
            layout: SessionTree::Pane { id: 0 },
            tabs: vec![],
        };
        let (tab, focused, pane_map) = session.into_tab();
        assert_eq!(focused, PaneId(0));
        assert_eq!(tab.focused(), PaneId(0));
        assert_eq!(pane_map.len(), 1);
        assert_eq!(pane_map[&PaneId(0)].0.as_deref(), Some("/bin/zsh"));
    }

    #[test]
    fn test_multi_tab_into_tabs() {
        let session = Session {
            active_tab: 1,
            focused: 2,
            panes: vec![
                PaneSession { id: 1, command: Some("/bin/bash".into()), cwd: None },
                PaneSession { id: 2, command: Some("/bin/zsh".into()), cwd: None },
            ],
            layout: SessionTree::Pane { id: 1 },
            tabs: vec![
                TabSnapshot { focused: 1, layout: SessionTree::Pane { id: 1 } },
                TabSnapshot { focused: 2, layout: SessionTree::Pane { id: 2 } },
            ],
        };
        let (tabs, active, pane_map) = session.into_tabs();
        assert_eq!(tabs.len(), 2);
        assert_eq!(active, 1);
        assert_eq!(tabs[0].focused(), PaneId(1));
        assert_eq!(tabs[1].focused(), PaneId(2));
        assert_eq!(pane_map.len(), 2);
    }

    #[test]
    fn test_legacy_single_tab_into_tabs() {
        let session = Session {
            active_tab: 0,
            focused: 5,
            panes: vec![PaneSession { id: 5, command: None, cwd: None }],
            layout: SessionTree::Pane { id: 5 },
            tabs: vec![],
        };
        let (tabs, active, pane_map) = session.into_tabs();
        assert_eq!(tabs.len(), 1);
        assert_eq!(active, 0);
        assert_eq!(tabs[0].focused(), PaneId(5));
        assert_eq!(pane_map.len(), 1);
    }

    #[test]
    fn test_layout_tree_round_trip() {
        let tree = SessionTree::Split {
            dir: "h".into(),
            ratio: 0.3,
            first: Box::new(SessionTree::Pane { id: 10 }),
            second: Box::new(SessionTree::Pane { id: 11 }),
        };
        let layout = session_to_layout_tree(&tree);
        let back = layout_tree_to_session(&layout);
        let json1 = serde_json::to_string(&tree).unwrap();
        let json2 = serde_json::to_string(&back).unwrap();
        assert_eq!(json1, json2);
    }
}
