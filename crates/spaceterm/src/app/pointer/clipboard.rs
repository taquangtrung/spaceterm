//! Text selection, word selection, and clipboard copy/paste.

use crate::model::layout::PaneId;

use super::super::Selection;
use super::App;

// ========================================================================
// Constants
// ========================================================================

const WORD_CHARS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-.~/";

// ========================================================================
// App — clipboard & selection
// ========================================================================

impl App {
    pub(crate) fn selected_text(&self) -> Option<String> {
        let sel = self.selection.as_ref()?;
        let pane = self.panes.get(&sel.pane)?;
        let grid = pane.grid();

        let (sr, sc, er, ec) = if (sel.start_row, sel.start_col) <= (sel.end_row, sel.end_col) {
            (sel.start_row, sel.start_col, sel.end_row, sel.end_col)
        } else {
            (sel.end_row, sel.end_col, sel.start_row, sel.start_col)
        };

        let mut text = String::new();
        for row in sr..=er {
            let col_start = if row == sr { sc } else { 0 };
            let col_end = if row == er { ec + 1 } else { grid.cols() };
            for col in col_start..col_end.min(grid.cols()) {
                let ch = grid.visible_cell(row, col).map(|c| c.ch).unwrap_or(' ');
                text.push(ch);
            }
            if row < er {
                text.push('\n');
            }
        }
        let trimmed = text.trim_end().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    }

    /// Recompute the highlighted selection from the Visual-mode anchor and the
    /// current nav cursor. Charwise spans anchor->cursor; linewise covers every
    /// column of the rows between them.
    pub(crate) fn update_visual_selection(&mut self, focused: PaneId) {
        let (Some((ar, ac)), Some((cr, cc))) = (self.visual_anchor, self.nav_cursor) else {
            self.selection = None;
            return;
        };
        let Some(pane) = self.panes.get(&focused) else {
            return;
        };
        let last_col = pane.grid().cols().saturating_sub(1);
        self.selection = Some(if self.visual_line {
            Selection {
                start_row: ar.min(cr),
                start_col: 0,
                end_row: ar.max(cr),
                end_col: last_col,
                pane: focused,
            }
        } else {
            Selection {
                start_row: ar,
                start_col: ac,
                end_row: cr,
                end_col: cc,
                pane: focused,
            }
        });
    }

    pub(crate) fn copy_selection(&self) {
        if let Some(text) = self.selected_text() {
            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                let _ = clipboard.set_text(&text);
            }
        }
    }

    pub(crate) fn paste_from_clipboard(&mut self) {
        let text = match arboard::Clipboard::new() {
            Ok(mut cb) => cb.get_text().ok(),
            Err(_) => None,
        };
        let Some(text) = text else { return };
        let focused = self.tab().focused();
        let Some(pane) = self.panes.get_mut(&focused) else {
            return;
        };
        if pane.bracketed_paste() {
            let mut bytes = Vec::with_capacity(text.len() + 8);
            bytes.extend_from_slice(b"\x1b[200~");
            bytes.extend_from_slice(text.as_bytes());
            bytes.extend_from_slice(b"\x1b[201~");
            pane.write(&bytes);
        } else {
            pane.write(text.as_bytes());
        }
    }

    pub(crate) fn select_word_at(&mut self, pane_id: PaneId, row: usize, col: usize) {
        let Some(pane) = self.panes.get(&pane_id) else {
            return;
        };
        let grid = pane.grid();
        let ch = grid.cell(row, col).map(|c| c.ch).unwrap_or(' ');
        if !WORD_CHARS.contains(ch) {
            self.selection = Some(Selection {
                start_row: row,
                start_col: col,
                end_row: row,
                end_col: col,
                pane: pane_id,
            });
            return;
        }
        let mut start = col;
        let mut end = col;
        while start > 0 {
            if let Some(c) = grid.cell(row, start - 1).map(|c| c.ch) {
                if WORD_CHARS.contains(c) {
                    start -= 1;
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        while end < grid.cols() - 1 {
            if let Some(c) = grid.cell(row, end + 1).map(|c| c.ch) {
                if WORD_CHARS.contains(c) {
                    end += 1;
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        self.selection = Some(Selection {
            start_row: row,
            start_col: start,
            end_row: row,
            end_col: end,
            pane: pane_id,
        });
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_word_chars_contains_alphanumeric() {
        assert!(WORD_CHARS.contains('a'));
        assert!(WORD_CHARS.contains('Z'));
        assert!(WORD_CHARS.contains('0'));
        assert!(WORD_CHARS.contains('_'));
        assert!(!WORD_CHARS.contains(' '));
    }
}
