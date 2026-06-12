//! Native command palette: a lightweight text-mode overlay activated by
//! `Ctrl-Shift-P` (or the configured key). Renders as GPU quads at the top
//! of the focused pane, no WebView required.

// ========================================================================
// Data Structures
// ========================================================================

#[derive(Clone, Debug)]
pub struct PaletteEntry {
    pub action: String,
    pub label: String,
}

#[derive(Clone, Debug, Default)]
pub struct Palette {
    pub active: bool,
    pub entries: Vec<PaletteEntry>,
    pub filtered: Vec<usize>,
    pub query: String,
    pub selected: usize,
}

// ========================================================================
// Implementation
// ========================================================================

impl Palette {
    pub fn open() -> Self {
        let entries = builtin_commands();
        let filtered = (0..entries.len()).collect();
        Palette {
            active: true,
            entries,
            filtered,
            query: String::new(),
            selected: 0,
        }
    }

    pub fn close(&mut self) {
        self.active = false;
        self.query.clear();
        self.selected = 0;
    }

    pub fn push_char(&mut self, c: char) {
        self.query.push(c);
        self.update_filter();
    }

    pub fn pop_char(&mut self) {
        self.query.pop();
        self.update_filter();
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if self.selected + 1 < self.filtered.len() {
            self.selected += 1;
        }
    }

    pub fn selected_action(&self) -> Option<&str> {
        self.filtered
            .get(self.selected)
            .map(|&i| self.entries[i].action.as_str())
    }

    fn update_filter(&mut self) {
        let q = self.query.to_lowercase();
        self.filtered = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.label.to_lowercase().contains(&q))
            .map(|(i, _)| i)
            .collect();
        self.selected = 0;
    }
}

fn builtin_commands() -> Vec<PaletteEntry> {
    vec![
        PaletteEntry {
            label: "Toggle Mode (Insert/Normal)".into(),
            action: "toggle_mode".into(),
        },
        PaletteEntry {
            label: "Split Horizontal".into(),
            action: "split_horizontal".into(),
        },
        PaletteEntry {
            label: "Split Vertical".into(),
            action: "split_vertical".into(),
        },
        PaletteEntry {
            label: "Close Pane".into(),
            action: "close_pane".into(),
        },
        PaletteEntry {
            label: "Focus Pane Down".into(),
            action: "focus_down".into(),
        },
        PaletteEntry {
            label: "Focus Pane Up".into(),
            action: "focus_up".into(),
        },
        PaletteEntry {
            label: "Focus Pane Left".into(),
            action: "focus_left".into(),
        },
        PaletteEntry {
            label: "Focus Pane Right".into(),
            action: "focus_right".into(),
        },
        PaletteEntry {
            label: "Search Blocks".into(),
            action: "search".into(),
        },
        PaletteEntry {
            label: "Next Block".into(),
            action: "next_block".into(),
        },
        PaletteEntry {
            label: "Previous Block".into(),
            action: "prev_block".into(),
        },
        PaletteEntry {
            label: "Quick Select".into(),
            action: "quick_select".into(),
        },
        PaletteEntry {
            label: "Yank Block Source".into(),
            action: "yank_block".into(),
        },
        PaletteEntry {
            label: "Toggle Fold".into(),
            action: "toggle_fold".into(),
        },
        PaletteEntry {
            label: "Theme: Dark".into(),
            action: "theme_dark".into(),
        },
        PaletteEntry {
            label: "Theme: Light".into(),
            action: "theme_light".into(),
        },
        PaletteEntry {
            label: "Theme: Auto".into(),
            action: "theme_auto".into(),
        },
    ]
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_palette_open_has_entries() {
        let p = Palette::open();
        assert!(p.active);
        assert!(!p.entries.is_empty());
        assert_eq!(p.filtered.len(), p.entries.len());
        assert_eq!(p.selected, 0);
    }

    #[test]
    fn test_palette_filter_narrows() {
        let mut p = Palette::open();
        let total = p.entries.len();
        for c in "split".chars() {
            p.push_char(c);
        }
        assert!(p.filtered.len() < total);
        assert!(p.filtered.len() >= 2);
    }

    #[test]
    fn test_palette_selection_navigates() {
        let mut p = Palette::open();
        assert_eq!(p.selected, 0);
        p.move_down();
        assert_eq!(p.selected, 1);
        p.move_up();
        assert_eq!(p.selected, 0);
    }

    #[test]
    fn test_palette_close_resets() {
        let mut p = Palette::open();
        p.push_char('s');
        p.move_down();
        p.close();
        assert!(!p.active);
        assert!(p.query.is_empty());
    }

    #[test]
    fn test_palette_selected_action() {
        let p = Palette::open();
        let action = p.selected_action().unwrap();
        assert_eq!(action, "toggle_mode");
    }

    #[test]
    fn test_palette_backspace() {
        let mut p = Palette::open();
        p.push_char('s');
        p.push_char('p');
        p.pop_char();
        assert_eq!(p.query, "s");
    }
}
