//! Pointer submodule: mouse hit-testing, selection state, clipboard, and
//! PTY mouse event forwarding.

mod clipboard;
mod mouse;

use crate::model::layout::{PaneId, Rect};
use spaceterm_render::renderer::PaneRect;

use super::App;

// ========================================================================
// App — pixel hit-testing helpers
// ========================================================================

impl App {
    pub(crate) fn pixel_to_cell(&self, x: f32, y: f32, pane_rect: PaneRect) -> (usize, usize) {
        let (cw, ch) = self
            .renderer
            .as_ref()
            .map(|r| r.cell_size())
            .unwrap_or((9.0, 20.0));
        let col = ((x - pane_rect.x) / cw).floor() as usize;
        let row = ((y - pane_rect.y) / ch).floor() as usize;
        (row, col)
    }

    pub(crate) fn pane_at_pixel(&self, x: f32, y: f32) -> Option<(PaneId, PaneRect)> {
        let vp = self.viewport_rect();
        let layout_vp = Rect::new(vp.x, vp.y, vp.width, vp.height);
        for (id, rect) in self.tab.rects(layout_vp) {
            let pr = Self::layout_rect_to_pane(rect);
            if x >= pr.x && x < pr.x + pr.width && y >= pr.y && y < pr.y + pr.height {
                return Some((id, pr));
            }
        }
        None
    }
}
