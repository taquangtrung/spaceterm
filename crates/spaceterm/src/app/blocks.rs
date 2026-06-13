//! Block fold / yank / focus / navigate operations.

use crate::model::input;
use crate::model::layout::PaneId;

use super::App;

// ========================================================================
// App — block operations
// ========================================================================

impl App {
    pub(crate) fn yank_block_source(&self, focused: PaneId) {
        let pane = match self.panes.get(&focused) {
            Some(p) => p,
            None => return,
        };
        let blocks = pane.scrollback().blocks();
        let cols = pane.grid().cols();
        let offsets = pane.scrollback().block_row_offsets(cols);
        let current = pane.grid().scroll_offset();
        let block_idx = offsets
            .iter()
            .enumerate()
            .rev()
            .find(|(_, &row)| row <= current)
            .map(|(i, _)| i);
        if let Some(idx) = block_idx {
            if let Some(block) = blocks.get(idx) {
                let text = block.plain_text();
                if !text.is_empty() {
                    if let Ok(mut clipboard) = arboard::Clipboard::new() {
                        let _ = clipboard.set_text(&text);
                    }
                }
            }
        }
    }

    pub(crate) fn toggle_fold(&mut self, focused: PaneId) {
        let pane = match self.panes.get(&focused) {
            Some(p) => p,
            None => return,
        };
        let cols = pane.grid().cols();
        let offsets = pane.scrollback().block_row_offsets(cols);
        let current = pane.grid().scroll_offset();
        let block_idx = offsets
            .iter()
            .enumerate()
            .rev()
            .find(|(_, &row)| row <= current)
            .map(|(i, _)| i);
        let Some(idx) = block_idx else {
            return;
        };

        let folded = self.folded_blocks.entry(focused).or_default();
        if folded.contains(&idx) {
            folded.remove(&idx);
            self.webview_mgr.unfold_block(focused, idx);
        } else {
            folded.insert(idx);
            self.webview_mgr.fold_block(focused, idx);
        }
        self.last_tile_layout = None;
        self.dirty = true;
    }

    pub(crate) fn is_block_folded(&self, pane_id: PaneId, block_index: usize) -> bool {
        self.folded_blocks
            .get(&pane_id)
            .is_some_and(|set| set.contains(&block_index))
    }

    pub(crate) fn focus_block(&mut self, nav: input::BlockNav, focused: PaneId) {
        let pane = match self.panes.get(&focused) {
            Some(p) => p,
            None => return,
        };
        let cols = pane.grid().cols();
        let offsets = pane.scrollback().block_row_offsets(cols);
        if offsets.is_empty() {
            return;
        }
        let current_offset = pane.grid().scroll_offset();
        let target_row = match nav {
            input::BlockNav::Next => offsets
                .iter()
                .find(|&&row| row > current_offset)
                .copied()
                .unwrap_or_else(|| offsets.last().copied().unwrap_or(0)),
            input::BlockNav::Previous => offsets
                .iter()
                .rev()
                .find(|&&row| row < current_offset)
                .copied()
                .unwrap_or(0),
        };
        let diff = target_row.abs_diff(current_offset);
        let grid = self
            .panes
            .get_mut(&focused)
            .unwrap()
            .grid_mut();
        if target_row > current_offset {
            grid.scroll_up_history(diff);
        } else {
            grid.scroll_down_history(diff);
        }
        self.dirty = true;
    }
}
