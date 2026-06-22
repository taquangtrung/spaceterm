//! Frame composition and WebView tile management.

use base64::Engine;
use serde_json::Value;

use crate::model::layout::{PaneId, Rect};
use crate::model::mode::Mode;
use crate::model::settings_page::{Control, SettingsField, SettingsPage};
use crate::terminal::block_queue::BlockEntry;
use crate::terminal::pane::{BLOCK_RESERVE_ROWS, MAX_IMAGE_ROWS};
use crate::terminal::webview;
use spaceterm_core::spaceterm_proto::EmitBlock;
use spaceterm_render::renderer::{PaneRect, PaneView};
use spaceterm_render::{
    Color, CursorShape, Grid, ImagePlacement, PaletteItem, PaletteView, RgbColor, Style, Theme,
    ThemeRgb,
};

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

/// Settings-page layout, in cells: the label indent, the column dim notes start
/// at, the right margin values align to, and the body's first row (below the
/// header band and its divider).
const SETTINGS_LEFT_PAD: usize = 4;
const SETTINGS_NOTE_COL: usize = 28;
const SETTINGS_RIGHT_PAD: usize = 4;
const SETTINGS_FIRST_ROW: usize = 3;
/// Footer hint shown along the bottom of the settings page.
const SETTINGS_HINT: &str = "↑/↓ Move     ←/→ Change     Space Toggle     Enter/Esc Close";

// ========================================================================
// App — rendering
// ========================================================================

impl App {
    pub(crate) fn render_frame(&mut self) {
        // The settings page is a full-window modal; it replaces the panes, chrome,
        // status bar, and block tiles entirely until it closes.
        if self.settings_page.is_some() {
            self.render_settings_frame();
            return;
        }

        let bell_active = self.is_bell_active();
        let notice = self.active_error_notice().map(str::to_string);
        // Built before the renderer is borrowed, since it reads tab/menu state.
        let chrome = self.build_top_chrome();
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        let (full_cols, full_rows) = renderer.grid_size();
        let (cw, ch) = renderer.cell_size();

        // Panes sit below the top chrome (tabbar/menubar) and above the status
        // bar; both bands are reserved cell rows the panes must not cover. The
        // status bar can be disabled, in which case it reserves no row.
        let top_rows = spaceterm_render::chrome_rows(self.config.menu_style);
        let status_rows = if self.config.status_bar.enabled {
            super::STATUS_BAR_ROWS
        } else {
            0
        };
        let content_rows = full_rows.saturating_sub(top_rows + status_rows).max(1);
        let layout_vp = Rect::new(
            0.0,
            top_rows as f32 * ch,
            full_cols as f32 * cw,
            content_rows as f32 * ch,
        );
        let rects = self.tabs[self.active_tab].rects(layout_vp);

        let sel = self
            .selection
            .as_ref()
            .map(|s| (s.pane, s.start_row, s.start_col, s.end_row, s.end_col));

        let focused = self.tabs[self.active_tab].focused();
        let mode = self.modes.get(&focused).copied().unwrap_or_default();
        let qs_labels: Vec<(usize, usize, char)> = self
            .quick_select
            .as_ref()
            .map(|labels| labels.iter().map(|ql| (ql.row, ql.col, ql.label)).collect())
            .unwrap_or_default();

        // Precompute search match cell positions per pane so PaneView can borrow
        // them as slices. Built before the view loop to satisfy the borrow checker.
        let query_chars: Option<Vec<char>> = self
            .search_query
            .as_deref()
            .filter(|q| !q.is_empty())
            .map(|q| q.to_lowercase().chars().collect());
        let search_match_data: Vec<Vec<(usize, usize)>> = rects
            .iter()
            .map(|(id, _)| match (&query_chars, self.panes.get(id)) {
                (Some(qc), Some(pane)) => search_grid_matches(pane.grid(), qc),
                _ => vec![],
            })
            .collect();

        let (cx, cy) = self.cursor_pos;
        let hovered_pane: Option<PaneId> = rects.iter().find(|(_, r)| {
            let pr = Self::layout_rect_to_pane(*r);
            cx >= pr.x && cx < pr.x + pr.width && cy >= pr.y && cy < pr.y + pr.height
        }).map(|(id, _)| *id);

        let mut views: Vec<PaneView> = Vec::new();
        for (i, (id, rect)) in rects.iter().enumerate() {
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
                let nav_cursor = if *id == focused && matches!(mode, Mode::Normal | Mode::Visual) {
                    self.nav_cursor
                } else {
                    None
                };
                // Pick the cursor shape for this pane: the focused pane uses
                // its current mode's configured shape, non-focused panes act as
                // Insert. The renderer applies it to whichever cursor is drawn
                // (in-grid shell cursor or nav cursor).
                let cursor_shape = if *id == focused {
                    match mode {
                        Mode::Insert => self.config.cursor.insert,
                        Mode::Normal => self.config.cursor.normal,
                        Mode::Visual => self.config.cursor.visual,
                        Mode::BlockFocus => self.config.cursor.block_focus,
                    }
                } else {
                    self.config.cursor.insert
                };
                let hovered_link = if hovered_pane == Some(*id) {
                    self.hovered_url.as_deref()
                        .map(|url| pane.grid().find_link_id(url))
                        .unwrap_or(0)
                } else {
                    0
                };
                let cursor_visible = *id != focused
                    || !self.config.cursor.blink
                    || self.blink_phase;
                views.push(PaneView {
                    cursor_shape,
                    cursor_visible,
                    focused: *id == focused,
                    grid: pane.grid(),
                    hovered_link,
                    labels,
                    nav_cursor,
                    rect: Self::layout_rect_to_pane(*rect),
                    scroll_offset: pane.grid().scroll_offset(),
                    scrollback_len: pane.grid().scrollback_len(),
                    search_matches: &search_match_data[i],
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
        let mut status = status_bar(
            mode,
            renderer.theme(),
            pane_title,
            notice,
            &self.config.status_bar,
        );
        if self.search_query.as_deref().is_some_and(|q| !q.is_empty()) {
            status.right_label = Some(if self.search_match_total == 0 {
                "no matches".to_string()
            } else {
                format!("{}/{}", self.search_match_index, self.search_match_total)
            });
        }
        let status = self.config.status_bar.enabled.then_some(&status);
        let palette_view = self.palette.as_ref().map(|p| PaletteView {
            empty_message: if p.mode == crate::model::palette::PaletteMode::History {
                "No matching history".to_string()
            } else if p.mode == crate::model::palette::PaletteMode::RecentDirs {
                "No recent directories".to_string()
            } else {
                "No matching commands".to_string()
            },
            items: p
                .filtered
                .iter()
                .map(|&i| PaletteItem {
                    action: p.entries[i].action.clone(),
                    label: p.entries[i].label.clone(),
                    match_positions: p.entries[i].match_positions.clone(),
                })
                .collect(),
            match_underline: self.config.palette_match_underline,
            query: p.query.clone(),
            selected: p.selected,
        });
        renderer.render(
            &views,
            status,
            Some(&chrome),
            bell_active,
            &placements,
            palette_view.as_ref(),
        );

        let focused = self.tabs[self.active_tab].focused();
        // Tiles for panes outside the active tab are hidden so background tabs
        // don't show through; the active tab's tiles are positioned by scroll.
        let active_panes: std::collections::HashSet<PaneId> =
            self.tabs[self.active_tab].panes().into_iter().collect();
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
                self.webview_mgr.reposition_tiles(
                    scroll_offset,
                    full_rows,
                    ch,
                    pane_y,
                    &active_panes,
                );
            }
        }
    }

    /// Draw the settings page as a single full-window grid: no chrome, status
    /// bar, panes, or block tiles, just the modal overlay.
    fn render_settings_frame(&mut self) {
        let Some(page) = &self.settings_page else {
            return;
        };
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        let (cols, rows) = renderer.grid_size();
        let (cw, ch) = renderer.cell_size();
        let grid = build_settings_grid(page, renderer.theme(), cols, rows);
        let view = PaneView {
            cursor_shape: CursorShape::Block,
            cursor_visible: true,
            focused: true,
            grid: &grid,
            hovered_link: 0,
            labels: None,
            // An out-of-bounds nav cursor suppresses the terminal cursor (the
            // settings grid has no caret of its own) without drawing a nav block.
            nav_cursor: Some((rows, cols)),
            rect: PaneRect {
                height: rows as f32 * ch,
                width: cols as f32 * cw,
                x: 0.0,
                y: 0.0,
            },
            scroll_offset: 0,
            scrollback_len: 0,
            search_matches: &[],
            selection: None,
        };
        renderer.render(std::slice::from_ref(&view), None, None, false, &[], None);
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
        let rects = self.tabs[self.active_tab].rects(layout_vp);
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

/// The resolved theme colors the settings page paints with, bundled so the
/// row/value helpers can share one palette.
struct SettingsPalette {
    accent: Color,
    accent_fg: Color,
    fg: Color,
    muted: Color,
    selected_bg: Color,
}

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

/// Paint the settings page into a fresh `cols` x `rows` grid: an elevated header
/// band, then titled sections of rows. Each row shows a label, a dim note, and a
/// right-aligned control; the selected row gets an accent bar and a highlight. A
/// key-hint footer sits along the bottom.
fn build_settings_grid(page: &SettingsPage, theme: &Theme, cols: usize, rows: usize) -> Grid {
    let mut grid = Grid::new(cols, rows);
    let pal = SettingsPalette {
        accent: theme_rgb(theme.cursor_bg),
        accent_fg: theme_rgb(theme.cursor_fg),
        fg: theme_rgb(theme.foreground),
        muted: mix_rgb(theme.foreground, theme.background, 0.45),
        selected_bg: theme_rgb(theme.menu_hover_bg),
    };
    let header_bg = theme_rgb(theme.menu_bg);
    let divider = theme_rgb(theme.divider);

    // Header band: a "Settings" title on an elevated strip, underlined by a rule.
    let band = Style {
        background: header_bg,
        ..Style::default()
    };
    put(&mut grid, 0, 0, &" ".repeat(cols), band);
    put(
        &mut grid,
        0,
        SETTINGS_LEFT_PAD,
        "Settings",
        Style {
            background: header_bg,
            bold: true,
            foreground: pal.fg,
            ..Style::default()
        },
    );
    put(
        &mut grid,
        1,
        0,
        &"─".repeat(cols),
        Style {
            foreground: divider,
            ..Style::default()
        },
    );

    // Body: sections of field rows. Stop before the footer's divider and hint.
    let body_end = rows.saturating_sub(2);
    let mut row = SETTINGS_FIRST_ROW;
    let mut section: Option<&str> = None;
    for (i, field) in page.fields.iter().enumerate() {
        if let Some(name) = field.section.as_deref() {
            if section != Some(name) {
                section = Some(name);
                row += 1; // spacer above the section header
                if row >= body_end {
                    break;
                }
                put(
                    &mut grid,
                    row,
                    SETTINGS_LEFT_PAD,
                    &name.to_uppercase(),
                    Style {
                        bold: true,
                        foreground: pal.accent,
                        ..Style::default()
                    },
                );
                row += 1;
            }
        }
        if row >= body_end {
            break;
        }
        draw_field_row(&mut grid, row, cols, field, i == page.selected, &pal);
        row += 1;
    }

    put(
        &mut grid,
        rows.saturating_sub(2),
        0,
        &"─".repeat(cols),
        Style {
            foreground: divider,
            ..Style::default()
        },
    );
    put(
        &mut grid,
        rows.saturating_sub(1),
        center_col(cols, SETTINGS_HINT.chars().count()),
        SETTINGS_HINT,
        Style {
            foreground: pal.muted,
            ..Style::default()
        },
    );
    grid
}

/// Paint one field row: an optional accent bar and highlight when selected, the
/// label, the right-aligned control, and the dim note between them.
fn draw_field_row(
    grid: &mut Grid,
    row: usize,
    cols: usize,
    field: &SettingsField,
    selected: bool,
    pal: &SettingsPalette,
) {
    let row_bg = if selected {
        pal.selected_bg
    } else {
        Color::Default
    };
    if selected {
        put(
            grid,
            row,
            0,
            &" ".repeat(cols),
            Style {
                background: pal.selected_bg,
                ..Style::default()
            },
        );
        // A left accent bar marks the focused row, like VSCode's focused setting.
        put(
            grid,
            row,
            0,
            "▌",
            Style {
                background: pal.selected_bg,
                foreground: pal.accent,
                ..Style::default()
            },
        );
    }
    put(
        grid,
        row,
        SETTINGS_LEFT_PAD,
        &field.label,
        Style {
            background: row_bg,
            foreground: pal.fg,
            ..Style::default()
        },
    );

    let value_col = draw_value(grid, row, cols, &field.control, selected, row_bg, pal);
    if let Some(note) = field.note.as_deref() {
        if SETTINGS_NOTE_COL + 1 < value_col {
            let budget = value_col - SETTINGS_NOTE_COL - 1;
            let text: String = note.chars().take(budget).collect();
            put(
                grid,
                row,
                SETTINGS_NOTE_COL,
                &text,
                Style {
                    background: row_bg,
                    foreground: pal.muted,
                    ..Style::default()
                },
            );
        }
    }
}

/// Draw a field's control, right-aligned to the margin, and return the column it
/// starts at so the caller can keep the note clear of it. Toggles render as an
/// `ON` pill or dim `OFF`; choices and numbers as `‹ value ›`; text inline with a
/// caret when focused.
fn draw_value(
    grid: &mut Grid,
    row: usize,
    cols: usize,
    control: &Control,
    selected: bool,
    row_bg: Color,
    pal: &SettingsPalette,
) -> usize {
    let on_value = Style {
        background: pal.accent,
        bold: true,
        foreground: pal.accent_fg,
        ..Style::default()
    };
    let muted = Style {
        background: row_bg,
        foreground: pal.muted,
        ..Style::default()
    };
    let accent = Style {
        background: row_bg,
        foreground: pal.accent,
        ..Style::default()
    };
    let segments: Vec<(String, Style)> = match control {
        Control::Toggle(t) if t.on => vec![(" ON ".to_string(), on_value)],
        Control::Toggle(_) => vec![(" OFF ".to_string(), muted)],
        Control::Choice(c) => {
            let label = c
                .options
                .get(c.index)
                .map(|o| o.label.as_str())
                .unwrap_or("");
            bracketed(label, muted, accent)
        }
        Control::Number(n) => {
            let value = format!("{:.*}", n.decimals, n.value);
            bracketed(&value, muted, accent)
        }
        Control::Text(t) => {
            let (text, style) = if t.value.is_empty() {
                ("default".to_string(), muted)
            } else {
                (
                    t.value.clone(),
                    Style {
                        background: row_bg,
                        foreground: pal.fg,
                        ..Style::default()
                    },
                )
            };
            let mut segments = vec![(text, style)];
            if selected {
                segments.push(("▏".to_string(), accent));
            }
            segments
        }
    };

    let width: usize = segments.iter().map(|(s, _)| s.chars().count()).sum();
    let start = cols.saturating_sub(SETTINGS_RIGHT_PAD + width);
    let mut col = start;
    for (text, style) in &segments {
        put(grid, row, col, text, *style);
        col += text.chars().count();
    }
    start
}

/// The `‹ value ›` segments for a choice or number, value in `accent` and the
/// guillemets in `muted`.
fn bracketed(value: &str, muted: Style, accent: Style) -> Vec<(String, Style)> {
    vec![
        ("‹ ".to_string(), muted),
        (value.to_string(), accent),
        (" ›".to_string(), muted),
    ]
}

/// Write `text` into `grid` starting at `(row, col)`, in `style`, truncated to
/// the grid width so it never wraps onto the next row.
fn put(grid: &mut Grid, row: usize, col: usize, text: &str, style: Style) {
    if col >= grid.cols() {
        return;
    }
    let budget = grid.cols() - col;
    grid.move_to(row, col);
    grid.set_style(style);
    for ch in text.chars().take(budget) {
        grid.print(ch);
    }
}

/// The starting column that centers `len` cells within `cols`.
fn center_col(cols: usize, len: usize) -> usize {
    cols.saturating_sub(len) / 2
}

/// Convert a theme color into an explicit grid cell color.
fn theme_rgb(c: ThemeRgb) -> Color {
    Color::Rgb(RgbColor {
        r: c.r,
        g: c.g,
        b: c.b,
    })
}

/// Blend `a` toward `b` by `t` in `[0, 1]`, e.g. to derive a muted text color
/// partway between the foreground and the background.
fn mix_rgb(a: ThemeRgb, b: ThemeRgb, t: f32) -> Color {
    let blend = |x: u8, y: u8| (x as f32 * (1.0 - t) + y as f32 * t).round() as u8;
    Color::Rgb(RgbColor {
        r: blend(a.r, b.r),
        g: blend(a.g, b.g),
        b: blend(a.b, b.b),
    })
}

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

/// Scan every row of `grid`'s visible area for occurrences of `query_chars`
/// (already lowercased) and return matching `(row, col)` cell positions.
fn search_grid_matches(grid: &Grid, query_chars: &[char]) -> Vec<(usize, usize)> {
    let qlen = query_chars.len();
    if qlen == 0 {
        return vec![];
    }
    let mut matches = Vec::new();
    for row in 0..grid.rows() {
        let row_chars: Vec<char> = (0..grid.cols())
            .filter_map(|c| grid.visible_cell(row, c))
            .map(|cell| cell.ch.to_ascii_lowercase())
            .collect();
        let row_len = row_chars.len();
        if row_len < qlen {
            continue;
        }
        for start in 0..=row_len - qlen {
            if row_chars[start..start + qlen] == *query_chars {
                for k in 0..qlen {
                    matches.push((row, start + k));
                }
            }
        }
    }
    matches
}
