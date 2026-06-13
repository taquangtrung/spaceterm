//! Frame composition and WebView tile management.

use crate::model::layout::{PaneId, Rect};
use crate::model::mode::Mode;
use crate::terminal::block_queue::BlockEntry;
use crate::terminal::webview;
use spaceterm_render::gpu::PaneView;

use super::{status_bar, App};

// ========================================================================
// App — rendering
// ========================================================================

impl App {
    pub(crate) fn render_frame(&mut self) {
        let bell_active = self.is_bell_active();
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        let (full_cols, full_rows) = renderer.grid_size();
        let (cw, ch) = renderer.cell_size();

        let content_rows = super::content_rows(full_rows);
        let layout_vp = Rect::new(0.0, 0.0, full_cols as f32 * cw, content_rows as f32 * ch);
        let rects = self.tab.rects(layout_vp);

        let sel = self
            .selection
            .as_ref()
            .map(|s| (s.pane, s.start_row, s.start_col, s.end_row, s.end_col));

        let focused = self.tab.focused();
        let mode = self.modes.get(&focused).copied().unwrap_or_default();
        let qs_labels: Vec<(usize, usize, char)> = self
            .quick_select
            .as_ref()
            .map(|labels| {
                labels
                    .iter()
                    .map(|ql| (ql.row, ql.col, ql.label))
                    .collect()
            })
            .unwrap_or_default();

        let mut views: Vec<PaneView> = Vec::new();
        for (id, rect) in &rects {
            if let Some(pane) = self.panes.get(id) {
                let sel_tuple = sel.and_then(|(pid, sr, sc, er, ec)| {
                    if pid == *id {
                        Some((sr, sc, er, ec))
                    } else {
                        None
                    }
                });
                let labels = if *id == focused && !qs_labels.is_empty() {
                    Some(qs_labels.as_slice())
                } else {
                    None
                };
                let nav_cursor = if *id == focused && mode == Mode::Normal {
                    self.nav_cursor
                } else {
                    None
                };
                views.push(PaneView {
                    grid: pane.grid(),
                    labels,
                    nav_cursor,
                    rect: Self::layout_rect_to_pane(*rect),
                    selection: sel_tuple,
                });
            }
        }

        let status = status_bar(mode, renderer.theme());
        renderer.render(&views, Some(&status), bell_active);

        let focused = self.tab.focused();
        if let Some(pane) = self.panes.get(&focused) {
            let scroll_offset = pane.grid().scroll_offset();
            let (_, ch) = renderer.cell_size();
            let focused_rect = rects.iter().find(|(id, _)| *id == focused);
            let pane_y = focused_rect.map(|(_, r)| r.y).unwrap_or(0.0);

            // Repositioning every tile does a GTK round-trip per WebView; only do
            // it when the scroll position or layout actually changed, otherwise
            // plain typing (which never moves tiles) stalls on GTK IPC.
            let layout = (scroll_offset, full_rows, ch.to_bits(), pane_y.to_bits());
            if self.last_tile_layout != Some(layout) {
                self.last_tile_layout = Some(layout);
                self.webview_mgr
                    .reposition_tiles(scroll_offset, full_rows, ch, pane_y);
            }
        }
    }

    pub(crate) fn create_block_tiles(&mut self, entries: &[(PaneId, BlockEntry)]) {
        let Some(window) = &self.window else { return };
        let Some(renderer) = &self.renderer else {
            return;
        };
        let (_, ch) = renderer.cell_size();
        let layout_vp = Rect::new(
            0.0,
            0.0,
            self.viewport_rect().width,
            self.viewport_rect().height,
        );
        let rects = self.tab.rects(layout_vp);

        for (pane_id, entry) in entries {
            if self.is_block_folded(*pane_id, entry.block_index) {
                continue;
            }
            let pane_rect = match rects.iter().find(|(id, _)| *id == *pane_id) {
                Some((_, r)) => Self::layout_rect_to_pane(*r),
                None => continue,
            };

            let html = webview::render_block_html(&entry.emit);
            let block_h = webview::WebViewManager::block_pixel_height(ch);
            let params = webview::TileParams {
                grid_row: entry.grid_row,
                html,
                x: pane_rect.x as i32,
                y: pane_rect.y as i32,
                width: pane_rect.width as u32,
                height: block_h,
            };

            if let Err(e) = self.webview_mgr.create_block_tile(*pane_id, entry, params, window) {
                eprintln!("spaceterm: block WebView error: {e}");
            }
        }
        self.last_tile_layout = None;
    }

    pub(crate) fn update_live_tiles(&mut self, patched: &[(PaneId, usize)]) {
        for (pane_id, entry_idx) in patched {
            let entry = match self.panes.get(pane_id) {
                Some(p) => p.block_queue().entries().get(*entry_idx).cloned(),
                None => None,
            };
            if let Some(entry) = entry {
                let html = webview::render_block_html(&entry.emit);
                if let Err(e) = self.webview_mgr.update_tile_html(*pane_id, &entry, &html) {
                    eprintln!("spaceterm: live-block update error: {e}");
                }
            }
        }
    }
}
