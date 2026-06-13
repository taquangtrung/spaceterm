//! Frame composition and WebView tile management.

use base64::Engine;
use serde_json::Value;

use crate::model::layout::{PaneId, Rect};
use crate::model::mode::Mode;
use crate::terminal::block_queue::BlockEntry;
use crate::terminal::pane::{BLOCK_RESERVE_ROWS, MAX_IMAGE_ROWS};
use crate::terminal::webview;
use spaceterm_core::spaceterm_proto::EmitBlock;
use spaceterm_render::renderer::PaneView;
use spaceterm_render::ImagePlacement;

use super::{status_bar, App, ImageBlock, ReflowSource};

// ========================================================================
// Constants
// ========================================================================

/// Raster image MIME types rendered natively on the GPU. Other rich types
/// (HTML, markdown, ...) still go to the WebView.
const RASTER_MIMES: [&str; 4] = ["image/gif", "image/jpeg", "image/png", "image/webp"];
const CSV_MIME: &str = "text/csv";
const JSON_MIME: &str = "application/json";
const MARKDOWN_MIME: &str = "text/markdown";
const SVG_MIME: &str = "image/svg+xml";

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
            .map(|labels| labels.iter().map(|ql| (ql.row, ql.col, ql.label)).collect())
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

        // Re-rasterize width-wrapped blocks (markdown/CSV/JSON) whose pane width
        // changed since they were last rendered, so wrapping stays correct on
        // resize. Intrinsic-size blocks (raster/SVG) have `reflow == None`.
        for i in 0..self.image_blocks.len() {
            let Some((_, rect)) = rects
                .iter()
                .find(|(id, _)| *id == self.image_blocks[i].pane_id)
            else {
                continue;
            };
            let target_w = Self::layout_rect_to_pane(*rect).width;
            let target = target_w.floor() as u32;
            if self.image_blocks[i].rastered_width == target
                || self.image_blocks[i].reflow.is_none()
            {
                continue;
            }
            let id = self.image_blocks[i].id;
            let dims = match self.image_blocks[i].reflow.as_ref() {
                Some(ReflowSource::Markdown(md)) => {
                    renderer.upload_markdown(id, &md.clone(), target_w)
                }
                Some(ReflowSource::Text(text)) => renderer.upload_text(id, &text.clone(), target_w),
                None => None,
            };
            if let Some((nat_w, nat_h)) = dims {
                let block = &mut self.image_blocks[i];
                block.nat_w = nat_w;
                block.nat_h = nat_h;
                block.rastered_width = target;
            }
        }

        // Place native image blocks at their grid row (scaled to fit the pane
        // width, preserving aspect), skipping any scrolled off the content area.
        let mut placements: Vec<ImagePlacement> = Vec::new();
        for img in &self.image_blocks {
            let Some((_, rect)) = rects.iter().find(|(id, _)| *id == img.pane_id) else {
                continue;
            };
            let pane_rect = Self::layout_rect_to_pane(*rect);
            let scroll_offset = self
                .panes
                .get(&img.pane_id)
                .map(|p| p.grid().scroll_offset())
                .unwrap_or(0);
            let visible_row = img.grid_row as isize - scroll_offset as isize;
            if visible_row < 0 || visible_row as usize >= content_rows {
                continue;
            }
            let nat_w = img.nat_w as f32;
            let nat_h = img.nat_h as f32;
            let band_h = img.max_rows as f32 * ch;
            let (display_w, display_h) = if nat_w <= 0.0 || nat_h <= 0.0 {
                (0.0, 0.0)
            } else if img.fit_to_band {
                // Images/SVG: scale down to fit the reserved band.
                let scale = (pane_rect.width / nat_w).min(band_h / nat_h).min(1.0);
                (nat_w * scale, nat_h * scale)
            } else {
                // Text/markdown: native size (wrapped to pane width).
                let w = nat_w.min(pane_rect.width);
                (w, nat_h * w / nat_w)
            };
            // Clip the bottom to the band and the content area (above the status
            // bar) so a tall block never overruns either.
            let y = pane_rect.y + visible_row as f32 * ch;
            let available = (pane_rect.y + pane_rect.height - y).max(0.0);
            let limit = band_h.min(available);
            let (height, v_max) = if display_h > limit && display_h > 0.0 {
                (limit, limit / display_h)
            } else {
                (display_h, 1.0)
            };
            placements.push(ImagePlacement {
                height,
                id: img.id,
                v_max,
                width: display_w,
                x: pane_rect.x,
                y,
            });
        }

        let pane_title = self.pane_titles.get(&focused).cloned();
        let status = status_bar(
            mode,
            renderer.theme(),
            pane_title,
            &self.config.status_bar_icons,
        );
        renderer.render(&views, Some(&status), bell_active, &placements);

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
        let Some(window) = self.window.clone() else {
            return;
        };
        let ch = match &self.renderer {
            Some(r) => r.cell_size().1,
            None => return,
        };
        let layout_vp = Rect::new(
            0.0,
            0.0,
            self.viewport_rect().width,
            self.viewport_rect().height,
        );
        let rects = self.tab.rects(layout_vp);
        let font_family = self.config.font_family.clone();
        let font_size = self.config.font_size;
        let debug = std::env::var_os("SPACETERM_BLOCK_DEBUG").is_some();

        for (pane_id, entry) in entries {
            if self.is_block_folded(*pane_id, entry.block_index) {
                continue;
            }
            let pane_rect = match rects.iter().find(|(id, _)| *id == *pane_id) {
                Some((_, r)) => Self::layout_rect_to_pane(*r),
                None => continue,
            };

            // Route images (raster + SVG) to the native GPU pass; everything
            // else (HTML, markdown, ...) renders in a WebView.
            if let Some(source) = native_image_source(&entry.emit) {
                let id = self.next_image_id;
                if let Some(renderer) = self.renderer.as_mut() {
                    // Width-wrapped blocks keep their source so they can be
                    // re-rasterized on resize; intrinsic-size ones (raster/SVG)
                    // do not need it.
                    // Images/SVG scale to fit the band; text shows at native
                    // size and clips. Width-wrapped kinds keep their source for
                    // re-rasterization on resize.
                    let (dims, reflow, fit_to_band, max_rows) = match &source {
                        NativeImage::Markdown(md) => (
                            renderer.upload_markdown(id, md, pane_rect.width),
                            Some(ReflowSource::Markdown(md.clone())),
                            false,
                            BLOCK_RESERVE_ROWS,
                        ),
                        NativeImage::Raster(bytes) => {
                            (renderer.upload_image(id, bytes), None, true, MAX_IMAGE_ROWS)
                        }
                        NativeImage::Svg(markup) => (
                            renderer.upload_svg(id, markup.as_bytes()),
                            None,
                            true,
                            BLOCK_RESERVE_ROWS,
                        ),
                        NativeImage::Text(text) => (
                            renderer.upload_text(id, text, pane_rect.width),
                            Some(ReflowSource::Text(text.clone())),
                            false,
                            BLOCK_RESERVE_ROWS,
                        ),
                    };
                    if let Some((nat_w, nat_h)) = dims {
                        self.next_image_id += 1;
                        self.image_blocks.push(ImageBlock {
                            fit_to_band,
                            grid_row: entry.grid_row,
                            id,
                            max_rows,
                            nat_h,
                            nat_w,
                            pane_id: *pane_id,
                            rastered_width: pane_rect.width.floor() as u32,
                            reflow,
                        });
                        if debug {
                            eprintln!("spaceterm: image block id={id} {nat_w}x{nat_h}");
                        }
                    } else if debug {
                        eprintln!("spaceterm: image decode failed for block");
                    }
                }
                continue;
            }

            let html = {
                let theme = self.renderer.as_ref().expect("renderer present").theme();
                webview::render_block_html(&entry.emit, theme, font_family.as_deref(), font_size)
            };
            let params = webview::TileParams {
                grid_row: entry.grid_row,
                html,
                x: pane_rect.x as i32,
                y: pane_rect.y as i32,
                width: pane_rect.width as u32,
                height: webview::WebViewManager::block_pixel_height(ch),
            };
            match self
                .webview_mgr
                .create_block_tile(*pane_id, entry, params, &window)
            {
                Ok(()) if debug => eprintln!("spaceterm: tile built ok"),
                Ok(()) => {}
                Err(e) => eprintln!("spaceterm: block WebView error: {e}"),
            }
        }
        self.last_tile_layout = None;
    }

    pub(crate) fn update_live_tiles(&mut self, patched: &[(PaneId, usize)]) {
        let Some(ref renderer) = self.renderer else {
            return;
        };
        let theme = renderer.theme();
        let font_family = self.config.font_family.as_deref();
        let font_size = self.config.font_size;

        for (pane_id, entry_idx) in patched {
            let entry = match self.panes.get(pane_id) {
                Some(p) => p.block_queue().entries().get(*entry_idx).cloned(),
                None => None,
            };
            if let Some(entry) = entry {
                let html = webview::render_block_html(&entry.emit, theme, font_family, font_size);
                if let Err(e) = self.webview_mgr.update_tile_html(*pane_id, &entry, &html) {
                    eprintln!("spaceterm: live-block update error: {e}");
                }
            }
        }
    }
}

// ========================================================================
// Data Structures
// ========================================================================

/// A block representation the GPU can render directly, bypassing the WebView.
enum NativeImage {
    /// Markdown source, laid out and rasterized by the renderer.
    Markdown(String),
    /// Encoded raster bytes (PNG/JPEG/GIF/WebP) for the `image` decoder.
    Raster(Vec<u8>),
    /// SVG markup for the `resvg` rasterizer.
    Svg(String),
    /// Preformatted monospace text (a CSV table or pretty-printed JSON).
    Text(String),
}

// ========================================================================
// Functions
// ========================================================================

/// The GPU-renderable source for a block's richest representation, or `None`
/// when it should render in the WebView (HTML, ...).
fn native_image_source(emit: &EmitBlock) -> Option<NativeImage> {
    let mime = webview::richest_mime(emit)?;
    let value = emit.bundle.get(mime)?;
    if RASTER_MIMES.contains(&mime) {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(value.as_str()?)
            .ok()?;
        Some(NativeImage::Raster(bytes))
    } else if mime == SVG_MIME {
        Some(NativeImage::Svg(value.as_str()?.to_string()))
    } else if mime == MARKDOWN_MIME {
        Some(NativeImage::Markdown(value.as_str()?.to_string()))
    } else if mime == CSV_MIME {
        Some(NativeImage::Text(csv_to_table(value.as_str()?)))
    } else if mime == JSON_MIME {
        Some(NativeImage::Text(json_to_text(value)))
    } else {
        None
    }
}

/// Format CSV rows into a column-aligned monospace table. Simple split on `,`;
/// quoted commas are not handled (acceptable for a preview).
fn csv_to_table(csv: &str) -> String {
    let rows: Vec<Vec<&str>> = csv
        .lines()
        .map(|line| line.split(',').map(str::trim).collect())
        .collect();
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths = vec![0usize; columns];
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    let mut out = String::new();
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            if i > 0 {
                out.push_str("  ");
            }
            out.push_str(cell);
            for _ in cell.chars().count()..widths[i] {
                out.push(' ');
            }
        }
        out.push('\n');
    }
    out
}

/// Pretty-print a JSON value. The bundle may carry it as a JSON string (from a
/// shell client) or as a structured value; both are normalized to pretty text.
fn json_to_text(value: &Value) -> String {
    let parsed = value
        .as_str()
        .and_then(|s| serde_json::from_str::<Value>(s).ok());
    let target = parsed.as_ref().unwrap_or(value);
    serde_json::to_string_pretty(target).unwrap_or_else(|_| value.to_string())
}
