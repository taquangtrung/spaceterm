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
            _ => {
                self.search_match_index = 0;
                self.search_match_total = 0;
                return;
            }
        };
        let pane = match self.panes.get(&focused) {
            Some(p) => p,
            None => return,
        };
        let cols = pane.grid().cols();
        let offsets = pane.scrollback().block_row_offsets(cols);
        let current = pane.grid().scroll_offset();
        let block_matches = pane.scrollback().search(&query);

        // Collect scroll offsets for matched blocks, deduplicating identical offsets
        // (two blocks at the same row are one navigation target).
        let mut candidates: Vec<usize> = block_matches
            .iter()
            .filter_map(|&i| offsets.get(i).copied())
            .collect();
        candidates.sort_unstable();
        candidates.dedup();

        // Include the live grid (scroll_offset == 0) as a candidate when it
        // contains the query, so search navigation reaches on-screen content
        // that has not yet been captured into a scrollback block.
        if live_grid_has_match(pane.grid(), &query) && !candidates.contains(&0) {
            candidates.push(0);
            candidates.sort_unstable();
        }

        self.search_match_total = candidates.len();
        if candidates.is_empty() {
            self.search_match_index = 0;
            return;
        }

        let target = match direction {
            input::BlockNav::Next => candidates
                .iter()
                .position(|&off| off > current)
                .or(Some(0)),
            input::BlockNav::Previous => candidates
                .iter()
                .rposition(|&off| off < current)
                .or(Some(candidates.len() - 1)),
        };

        if let Some(pos) = target {
            self.search_match_index = pos + 1;
            let target_row = candidates[pos];
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

// ========================================================================
// Helpers
// ========================================================================

/// Return true when the live grid (row 0..rows, scroll-offset-independent)
/// contains `query` on any row. Uses `grid.cell()` which reads the main
/// buffer directly, ignoring any current scroll offset.
fn live_grid_has_match(grid: &spaceterm_render::Grid, query: &str) -> bool {
    let query_lower: Vec<char> = query.to_lowercase().chars().collect();
    let qlen = query_lower.len();
    if qlen == 0 {
        return false;
    }
    for row in 0..grid.rows() {
        let row_chars: Vec<char> = (0..grid.cols())
            .filter_map(|c| grid.cell(row, c))
            .map(|cell| cell.ch.to_ascii_lowercase())
            .collect();
        let n = row_chars.len();
        if n < qlen {
            continue;
        }
        if (0..=n - qlen).any(|s| row_chars[s..s + qlen] == *query_lower) {
            return true;
        }
    }
    false
}
