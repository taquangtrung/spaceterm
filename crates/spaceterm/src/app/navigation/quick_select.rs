//! Quick-select: assigns single-letter labels to visible block boundaries so
//! the user can jump to a block with one keystroke.

use crate::model::layout::PaneId;

use crate::app::{App, QuickLabel};

// ========================================================================
// Constants
// ========================================================================

const QUICK_SELECT_LABELS: &[char] = &[
    'a', 's', 'd', 'f', 'g', 'h', 'j', 'k', 'l', 'q', 'w', 'e', 'r', 't', 'y', 'u', 'i', 'o', 'p',
    'z', 'x', 'c', 'v', 'b', 'n', 'm',
];

// ========================================================================
// App — quick-select
// ========================================================================

impl App {
    pub(crate) fn enter_quick_select(&mut self, focused: PaneId) {
        self.quick_select = self.generate_quick_labels(focused);
        self.dirty = true;
    }

    fn generate_quick_labels(&self, focused: PaneId) -> Option<Vec<QuickLabel>> {
        let pane = self.panes.get(&focused)?;
        let grid = pane.grid();
        let scrollback = pane.scrollback();
        let cols = grid.cols();
        let rows = grid.rows();
        let offsets = scrollback.block_row_offsets(cols);
        let scroll_offset = grid.scroll_offset();
        let sb_len = grid.scrollback_len();

        let vis_start = sb_len.saturating_sub(scroll_offset);
        let vis_end = vis_start + rows;

        let mut labels = Vec::new();
        for &abs_row in &offsets {
            if abs_row >= vis_start && abs_row < vis_end {
                if let Some(&label) = QUICK_SELECT_LABELS.get(labels.len()) {
                    let grid_row = abs_row - vis_start;
                    labels.push(QuickLabel {
                        row: grid_row,
                        col: 0,
                        label,
                    });
                }
            }
        }

        if labels.is_empty() {
            None
        } else {
            Some(labels)
        }
    }

    pub(crate) fn quick_jump(&mut self, focused: PaneId, label: char) {
        let labels = match self.quick_select.take() {
            Some(l) => l,
            None => return,
        };
        let target = match labels.iter().find(|ql| ql.label == label) {
            Some(ql) => *ql,
            None => return,
        };

        let pane = match self.panes.get(&focused) {
            Some(p) => p,
            None => return,
        };
        let cols = pane.grid().cols();
        let offsets = pane.scrollback().block_row_offsets(cols);
        let scroll_offset = pane.grid().scroll_offset();
        let sb_len = pane.grid().scrollback_len();

        let vis_start = sb_len.saturating_sub(scroll_offset);
        let abs_target = vis_start + target.row;

        let Some(&block_start) = offsets.iter().find(|&&row| row <= abs_target) else {
            return;
        };

        let diff = abs_target.saturating_sub(block_start);
        if block_start >= sb_len {
            let live_row = block_start - sb_len;
            let current_row = pane.grid().cursor().0;
            if live_row > current_row {
                if let Some(pane) = self.panes.get_mut(&focused) {
                    pane.grid_mut().scroll_up_history(live_row - current_row);
                }
            }
        } else if block_start > 0 || diff > 0 {
            let target_scroll = sb_len.saturating_sub(block_start);
            if let Some(pane) = self.panes.get_mut(&focused) {
                pane.grid_mut().set_scroll_offset(target_scroll);
            }
        }

        self.dirty = true;
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quick_select_labels_constants() {
        assert!(!QUICK_SELECT_LABELS.is_empty());
        assert_eq!(QUICK_SELECT_LABELS[0], 'a');
    }
}
