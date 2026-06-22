//! Native command palette: a lightweight text-mode overlay activated by
//! `Ctrl-Shift-P` (or the configured key). Renders as GPU quads at the top
//! of the focused pane, no WebView required.

use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::PathBuf;

// ========================================================================
// Constants
// ========================================================================

const HISTORY_MAX: usize = 500;

// ========================================================================
// Data Structures
// ========================================================================

/// Whether the palette is showing built-in commands, shell history, or recent dirs.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum PaletteMode {
    #[default]
    Commands,
    History,
    /// `cd` target picker: selecting an entry executes `cd <dir>` immediately.
    RecentDirs,
}

#[derive(Clone, Debug)]
pub struct PaletteEntry {
    pub action: String,
    pub label: String,
    /// Char indices in `label` that matched the current query (for highlight).
    pub match_positions: Vec<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct Palette {
    pub active: bool,
    pub entries: Vec<PaletteEntry>,
    pub filtered: Vec<usize>,
    pub mode: PaletteMode,
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
            mode: PaletteMode::Commands,
            query: String::new(),
            selected: 0,
        }
    }

    pub fn open_history() -> Self {
        let entries = load_history_entries();
        let filtered = (0..entries.len()).collect();
        Palette {
            active: true,
            entries,
            filtered,
            mode: PaletteMode::History,
            query: String::new(),
            selected: 0,
        }
    }

    /// Open the palette in recent-dirs mode. `dirs` is a deduplicated list of
    /// working directories ordered most-recently-used first.
    pub fn open_recent_dirs(dirs: Vec<String>) -> Self {
        let entries = dirs
            .into_iter()
            .map(|dir| PaletteEntry {
                action: dir.clone(),
                label: dir,
                match_positions: Vec::new(),
            })
            .collect::<Vec<_>>();
        let filtered = (0..entries.len()).collect();
        Palette {
            active: true,
            entries,
            filtered,
            mode: PaletteMode::RecentDirs,
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
        if q.is_empty() {
            for entry in &mut self.entries {
                entry.match_positions.clear();
            }
            self.filtered = (0..self.entries.len()).collect();
            self.selected = 0;
            return;
        }

        // Compute score + positions for every entry without mutating them yet.
        let scored: Vec<Option<(u8, Vec<usize>)>> =
            self.entries.iter().map(|e| score_and_positions(&e.label, &q)).collect();

        // Write positions back.
        for (entry, result) in self.entries.iter_mut().zip(scored.iter()) {
            entry.match_positions = result.as_ref().map(|(_, p)| p.clone()).unwrap_or_default();
        }

        let mut filtered: Vec<(usize, u8)> = scored
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.as_ref().map(|(s, _)| (i, *s)))
            .collect();
        filtered.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        self.filtered = filtered.into_iter().map(|(i, _)| i).collect();
        self.selected = 0;
    }
}

// ========================================================================
// Scoring
// ========================================================================

/// Score `label` against `query` (already lowercased) and return the match
/// score plus the char indices in `label` that were matched.
/// Score levels: 3 = exact, 2 = prefix, 1 = substring, 0 = subsequence.
fn score_and_positions(label: &str, query: &str) -> Option<(u8, Vec<usize>)> {
    let label_lower = label.to_lowercase();
    let qlen = query.chars().count();

    if label_lower == query {
        return Some((3, (0..label.chars().count()).collect()));
    }
    if label_lower.starts_with(query) {
        return Some((2, (0..qlen).collect()));
    }
    if let Some(byte_pos) = label_lower.find(query) {
        let char_start = label_lower[..byte_pos].chars().count();
        return Some((1, (char_start..char_start + qlen).collect()));
    }
    // Subsequence: greedily find the earliest matching char position.
    let label_chars: Vec<char> = label_lower.chars().collect();
    let query_chars: Vec<char> = query.chars().collect();
    let mut positions = Vec::with_capacity(query_chars.len());
    let mut qi = 0;
    for (ci, &lc) in label_chars.iter().enumerate() {
        if qi < query_chars.len() && lc == query_chars[qi] {
            positions.push(ci);
            qi += 1;
        }
    }
    if qi == query_chars.len() {
        Some((0, positions))
    } else {
        None
    }
}

// ========================================================================
// History
// ========================================================================

fn load_history_entries() -> Vec<PaletteEntry> {
    let content = try_read_history();
    parse_history_lines(&content)
        .into_iter()
        .map(|cmd| PaletteEntry {
            action: cmd.clone(),
            label: cmd,
            match_positions: Vec::new(),
        })
        .collect()
}

fn try_read_history() -> String {
    let candidates: Vec<PathBuf> = [
        env::var("HISTFILE").ok().map(PathBuf::from),
        home_dir().map(|h| h.join(".zsh_history")),
        home_dir().map(|h| h.join(".bash_history")),
    ]
    .into_iter()
    .flatten()
    .collect();

    for path in candidates {
        if let Ok(content) = fs::read_to_string(&path) {
            if !content.is_empty() {
                return content;
            }
        }
    }
    String::new()
}

/// Parse a history file, returning commands in most-recent-first order,
/// deduplicated. Supports bash (plain lines) and zsh extended format
/// (`: timestamp:elapsed;command`).
fn parse_history_lines(content: &str) -> Vec<String> {
    let raw: Vec<&str> = content
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            // zsh extended history: ": 1234567890:0;command"
            if let Some(rest) = line.strip_prefix(": ") {
                return rest.splitn(2, ';').nth(1).map(str::trim).filter(|s| !s.is_empty());
            }
            Some(line)
        })
        .collect();

    let mut seen = HashSet::new();
    let mut result: Vec<String> = raw
        .iter()
        .rev()
        .filter(|&&cmd| seen.insert(cmd))
        .map(|&s| s.to_string())
        .collect();
    result.truncate(HISTORY_MAX);
    result
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}

// ========================================================================
// Built-in commands
// ========================================================================

fn builtin_commands() -> Vec<PaletteEntry> {
    let entries = [
        ("cd_recent", "CD: Recent Directory"),
        ("close_pane", "Close Pane"),
        ("close_tab", "Close Tab"),
        ("focus_down", "Focus Pane Down"),
        ("focus_left", "Focus Pane Left"),
        ("focus_right", "Focus Pane Right"),
        ("focus_up", "Focus Pane Up"),
        ("new_tab", "New Tab"),
        ("next_block", "Next Block"),
        ("next_tab", "Next Tab"),
        ("open_settings", "Settings"),
        ("prev_block", "Previous Block"),
        ("prev_tab", "Previous Tab"),
        ("quick_select", "Quick Select"),
        ("recent_tab_back", "Recent Tab (Backward)"),
        ("recent_tab_forward", "Recent Tab (Forward)"),
        ("search", "Search Blocks"),
        ("split_horizontal", "Split Horizontal"),
        ("split_vertical", "Split Vertical"),
        ("theme_auto", "Theme: Auto"),
        ("theme_dark", "Theme: Dark"),
        ("theme_light", "Theme: Light"),
        ("toggle_fold", "Toggle Fold"),
        ("toggle_mode", "Toggle Mode (Insert/Normal)"),
        ("yank_block", "Yank Block Source"),
    ];
    entries
        .into_iter()
        .map(|(action, label)| PaletteEntry {
            action: action.into(),
            label: label.into(),
            match_positions: Vec::new(),
        })
        .collect()
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
        assert_eq!(p.mode, PaletteMode::Commands);
    }

    #[test]
    fn test_history_open_sets_history_mode() {
        let p = Palette::open_history();
        assert!(p.active);
        assert_eq!(p.mode, PaletteMode::History);
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
        assert_eq!(action, "cd_recent");
    }

    #[test]
    fn test_palette_backspace() {
        let mut p = Palette::open();
        p.push_char('s');
        p.push_char('p');
        p.pop_char();
        assert_eq!(p.query, "s");
    }

    #[test]
    fn test_score_and_positions_exact_highlights_all() {
        let (score, positions) = score_and_positions("Split Vertical", "split vertical").unwrap();
        assert_eq!(score, 3);
        assert_eq!(positions.len(), "split vertical".len());
    }

    #[test]
    fn test_score_and_positions_substring_returns_contiguous_range() {
        let (score, positions) = score_and_positions("Split Vertical", "vert").unwrap();
        assert_eq!(score, 1);
        assert!(positions.windows(2).all(|w| w[1] == w[0] + 1), "positions must be contiguous");
    }

    #[test]
    fn test_score_and_positions_subsequence_returns_positions() {
        let (score, positions) = score_and_positions("Split Vertical", "sv").unwrap();
        assert_eq!(score, 0);
        assert_eq!(positions.len(), 2);
    }

    #[test]
    fn test_filter_sets_match_positions() {
        let mut p = Palette::open();
        for c in "new tab".chars() {
            p.push_char(c);
        }
        assert!(!p.filtered.is_empty());
        let top = p.filtered[0];
        assert!(
            !p.entries[top].match_positions.is_empty(),
            "matched entry must have positions"
        );
    }

    #[test]
    fn test_parse_history_lines_deduplicates_most_recent_first() {
        let input = "ls\npwd\nls\ngit status\n";
        let result = parse_history_lines(input);
        assert_eq!(result[0], "git status");
        assert_eq!(result[1], "ls");
        assert_eq!(result[2], "pwd");
        assert_eq!(result.len(), 3, "duplicate 'ls' must be removed");
    }

    #[test]
    fn test_parse_history_lines_strips_zsh_timestamps() {
        let input = ": 1700000000:0;echo hello\n: 1700000001:0;ls\n";
        let result = parse_history_lines(input);
        assert_eq!(result[0], "ls");
        assert_eq!(result[1], "echo hello");
    }
}
