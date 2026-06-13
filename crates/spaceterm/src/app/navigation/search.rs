//! In-pane text search — scrolls the viewport to the next/previous match.

use crate::model::input;
use crate::model::layout::PaneId;

use super::App;

// ========================================================================
// App — search
// ========================================================================

impl App {
    pub(crate) fn search_in_pane(&mut self, focused: PaneId, direction: input::BlockNav) {
        let query = match &self.search_query {
            Some(q) if !q.is_empty() => q.clone(),
            _ => return,
        };
        let pane = match self.panes.get(&focused) {
            Some(p) => p,
            None => return,
        };
        let cols = pane.grid().cols();
        let offsets = pane.scrollback().block_row_offsets(cols);
        let current = pane.grid().scroll_offset();
        let matches = pane.scrollback().search(&query);
        if matches.is_empty() {
            return;
        }
        let target = match direction {
            input::BlockNav::Next => matches
                .iter()
                .find(|&&i| offsets.get(i).is_some_and(|&row| row > current))
                .or_else(|| matches.first()),
            input::BlockNav::Previous => matches
                .iter()
                .rev()
                .find(|&&i| offsets.get(i).is_some_and(|&row| row < current))
                .or_else(|| matches.last()),
        };
        if let Some(&idx) = target {
            if let Some(&target_row) = offsets.get(idx) {
                let diff = target_row.abs_diff(current);
                let pane = self.panes.get_mut(&focused).unwrap();
                let grid = pane.grid_mut();
                if target_row > current {
                    grid.scroll_up_history(diff);
                } else {
                    grid.scroll_down_history(diff);
                }
                self.dirty = true;
            }
        }
    }
}
