//! Navigation submodules: vim-style cursor motions, text search, and
//! quick-select block jumping.

mod quick_select;
mod search;
mod vim;

use crate::model::input::{self, FindChar};
use crate::model::layout::PaneId;
use crate::model::mode::Mode;

use super::App;

// ========================================================================
// App — normal-mode cursor
// ========================================================================

impl App {
    /// Place the traversal cursor at the focused pane's terminal cursor, where
    /// the prompt sits, when entering Normal mode. The shell cursor rests one
    /// cell past the last typed character; clamp onto it so Normal mode never
    /// starts beyond the typed text (Vim steps left off the end on `Esc`).
    pub(crate) fn init_nav_cursor(&mut self, focused: PaneId) {
        if let Some(pane) = self.panes.get(&focused) {
            let grid = pane.grid();
            let (cursor_row, cursor_col) = grid.cursor();
            let row = cursor_row.min(grid.rows().saturating_sub(1));
            let col = cursor_col.min(vim::nav_line_end(grid, row));
            self.nav_cursor = Some((row, col));
        }
    }

    /// Move the traversal cursor within the focused pane. Moves past a viewport
    /// edge scroll the grid's history instead, so the cursor reaches the whole
    /// buffer; page and jump moves scroll directly.
    pub(crate) fn move_nav_cursor(&mut self, mv: input::CursorMove, focused: PaneId) {
        use input::CursorMove;

        let Some(pane) = self.panes.get_mut(&focused) else {
            return;
        };
        let grid = pane.grid_mut();
        let rows = grid.rows();
        let cols = grid.cols();
        let (mut row, mut col) = self.nav_cursor.unwrap_or_else(|| grid.cursor());
        row = row.min(rows.saturating_sub(1));
        col = col.min(cols.saturating_sub(1));

        match mv {
            CursorMove::Left => col = col.saturating_sub(1),
            CursorMove::Right => col += 1,
            CursorMove::Up => {
                if row > 0 {
                    row -= 1;
                } else {
                    grid.scroll_up_history(1);
                }
            }
            CursorMove::Down => {
                if row < grid.last_content_row() {
                    row += 1;
                } else {
                    grid.scroll_down_history(1);
                }
            }
            CursorMove::LineStart => col = 0,
            CursorMove::LineEnd => col = cols,
            CursorMove::FirstNonBlank => col = vim::first_non_blank(&vim::line_chars(grid, row)),
            CursorMove::WordForward => {
                (row, col) = vim::motion_word_forward(grid, rows, row, col, false)
            }
            CursorMove::WordForwardBig => {
                (row, col) = vim::motion_word_forward(grid, rows, row, col, true)
            }
            CursorMove::WordBack => (row, col) = vim::motion_word_back(grid, row, col, false),
            CursorMove::WordBackBig => (row, col) = vim::motion_word_back(grid, row, col, true),
            CursorMove::WordEnd => (row, col) = vim::motion_word_end(grid, rows, row, col, false),
            CursorMove::WordEndBig => (row, col) = vim::motion_word_end(grid, rows, row, col, true),
            CursorMove::Top => {
                grid.set_scroll_offset(grid.scrollback_len());
                row = 0;
                col = 0;
            }
            CursorMove::Bottom => {
                grid.set_scroll_offset(0);
                row = grid.last_content_row();
            }
            CursorMove::PageUp => grid.scroll_up_history(rows),
            CursorMove::PageDown => grid.scroll_down_history(rows),
            CursorMove::HalfPageUp => grid.scroll_up_history(rows / 2),
            CursorMove::HalfPageDown => grid.scroll_down_history(rows / 2),
        }

        // Respect each line's real end: never sit on the blank padding past the
        // last printed character (snapping to a shorter line on vertical moves).
        // The prompt row extends to the shell cursor so typed trailing whitespace
        // stays reachable.
        col = col.min(vim::nav_line_end(grid, row));
        self.nav_cursor = Some((row, col));
        self.dirty = true;
    }

    /// Move the traversal cursor to a char-search (`f`/`F`/`t`/`T`) target on the
    /// current line. A miss leaves the cursor put. Extends the Visual selection
    /// when the focused pane is in Visual mode, mirroring [`Self::move_nav_cursor`].
    pub(crate) fn find_char_move(&mut self, find: FindChar, focused: PaneId) {
        let Some(pane) = self.panes.get(&focused) else {
            return;
        };
        let grid = pane.grid();
        let (row, col) = self.nav_cursor.unwrap_or_else(|| grid.cursor());
        let row = row.min(grid.rows().saturating_sub(1));
        let line = vim::line_chars(grid, row);
        let Some(target) = vim::find_char(&line, col, find) else {
            return;
        };
        let col = target.min(vim::nav_line_end(grid, row));

        self.nav_cursor = Some((row, col));
        if self.modes.get(&focused) == Some(&Mode::Visual) {
            self.update_visual_selection(focused);
        }
        self.dirty = true;
    }
}
