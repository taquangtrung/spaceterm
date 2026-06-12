//! GPU text renderer: renders [`Grid`]s to a wgpu surface using `glyphon` +
//! `cosmic-text` for glyph rasterization and `wgpu` for compositing.
//!
//! The renderer draws two layers per frame:
//! 1. **Background layer** — colored quads for cells with non-default
//!    backgrounds, plus the cursor.
//! 2. **Text layer** — glyphon renders shaped glyphs with per-cell foreground
//!    colors.
//!
//! Multi-pane rendering is supported: pass a slice of [`PaneView`] items, each
//! with a viewport rect and a grid reference. Each pane is clipped to its rect.

use glyphon::{
    Attrs, BufferLine, Cache, Color, ColorMode, Family, FontSystem, Shaping, SwashCache, TextArea,
    TextAtlas, TextBounds, TextRenderer, Viewport,
};
use wgpu::{
    BufferUsages, ColorTargetState, ColorWrites, Device, DeviceDescriptor, FragmentState,
    FrontFace, LoadOp, MultisampleState, PipelineLayoutDescriptor, PolygonMode, PrimitiveState,
    PrimitiveTopology, Queue, RenderPassColorAttachment, RenderPassDescriptor, RenderPipeline,
    RenderPipelineDescriptor, ShaderModuleDescriptor, ShaderSource, StoreOp, Surface,
    SurfaceConfiguration, TextureFormat, VertexAttribute, VertexBufferLayout, VertexFormat,
    VertexState, VertexStepMode,
};

use crate::grid::{Color as GridColor, CursorShape, Grid, RgbColor};
use crate::theme::{Rgb, Theme};

// ========================================================================
// Constants
// ========================================================================

const DEFAULT_FONT_SIZE: f32 = 15.0;
const DEFAULT_LINE_HEIGHT: f32 = 20.0;
const BG_SHADER: &str = include_str!("bg.wgsl");
const CURSOR_BAR_WIDTH_RATIO: f32 = 0.15;
const CURSOR_UNDERLINE_HEIGHT_RATIO: f32 = 0.2;
const BG_BUFFER_SIZE: u64 = 4 * 1024 * 1024;

// ========================================================================
// Data Structures
// ========================================================================

// ========================================================================
// Data Structures
// ========================================================================

/// A viewport rect for one pane, in pixels from the surface top-left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaneRect {
    pub height: f32,
    pub width: f32,
    pub x: f32,
    pub y: f32,
}

/// One pane's rendering input: where to draw and what grid to draw.
pub struct PaneView<'a> {
    pub grid: &'a Grid,
    pub labels: Option<&'a [(usize, usize, char)]>,
    /// The Normal-mode traversal cursor, in viewport `(row, col)`, drawn as a
    /// block. `None` when the pane is not being navigated.
    pub nav_cursor: Option<(usize, usize)>,
    pub rect: PaneRect,
    pub selection: Option<(usize, usize, usize, usize)>,
}

/// A Vim-style bottom status bar: the `label` (e.g. " NORMAL ") is drawn over a
/// segment filled with `accent`, atop a full-width strip in the theme's status
/// colors. Occupies the bottom-most cell row of the surface.
pub struct StatusBar {
    pub accent: Rgb,
    pub label: String,
}

/// Font selection for the renderer. `family` is the primary family name (e.g.
/// "FiraCode Nerd Font"); `None` falls back to the system default monospace.
/// Glyphs missing from the primary font are filled in from the system font
/// database automatically.
#[derive(Clone, Debug)]
pub struct FontConfig {
    pub family: Option<String>,
    pub size: f32,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: None,
            size: DEFAULT_FONT_SIZE,
        }
    }
}

/// A handle to a background system-font scan. Scanning system fonts takes
/// ~150ms and needs no GPU, so it runs on its own thread that overlaps GPU
/// initialization. Created by [`start_font_load`], consumed by [`GpuRenderer::new`].
pub struct FontLoad(std::thread::JoinHandle<FontSystem>);

impl FontLoad {
    fn join(self) -> FontSystem {
        self.0.join().expect("font-load thread panicked")
    }
}

/// Start scanning system fonts on a background thread. Call this as early as
/// possible (before creating the wgpu instance/adapter/device) so the scan
/// overlaps GPU initialization instead of adding to it.
pub fn start_font_load() -> FontLoad {
    FontLoad(std::thread::spawn(FontSystem::new))
}

/// GPU text renderer: owns the wgpu device/queue, glyph atlas, and render
/// pipelines. Designed to be created once and reused across frames.
pub struct GpuRenderer {
    device: Device,
    queue: Queue,
    surface: Surface<'static>,
    config: SurfaceConfiguration,
    font_system: FontSystem,
    swash_cache: SwashCache,
    #[allow(dead_code)]
    cache: Cache,
    text_atlas: TextAtlas,
    text_renderer: TextRenderer,
    viewport: Viewport,
    bg_pipeline: RenderPipeline,
    bg_buffer: wgpu::Buffer,
    cell_width: f32,
    cell_height: f32,
    cols: usize,
    rows: usize,
    font_family: Option<String>,
    font_size: f32,
    line_height: f32,
    theme: Theme,
    /// Persistent per-pane text buffers, reused across frames so cosmic-text
    /// only re-shapes lines whose content changed (one line per keystroke
    /// instead of the whole screen).
    text_buffers: Vec<glyphon::Buffer>,
    /// Persistent one-line buffer for the bottom status bar, reused like the
    /// pane buffers so its glyphs are only reshaped when the label changes.
    status_buffer: Option<glyphon::Buffer>,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct BgVertex {
    x: f32,
    y: f32,
    r: f32,
    g: f32,
    b: f32,
}

// ========================================================================
// GpuRenderer
// ========================================================================

impl GpuRenderer {
    /// Create the renderer, the wgpu surface, device, and queue.
    ///
    /// The `surface` is created externally (by the app crate from a winit
    /// window) and moved in. Call [`Self::resize`] before the first
    /// [`Self::render`].
    pub fn new(
        surface: Surface<'static>,
        adapter: wgpu::Adapter,
        width: u32,
        height: u32,
        font: FontConfig,
        font_load: FontLoad,
    ) -> Self {
        let (device, queue) =
            pollster::block_on(adapter.request_device(&DeviceDescriptor::default()))
                .expect("request wgpu device");

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .find(|f| matches!(f, TextureFormat::Bgra8Unorm | TextureFormat::Rgba8Unorm))
            .copied()
            .unwrap_or(caps.formats[0]);

        let config = SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let font_size = font.size;
        let line_height = font_size * (DEFAULT_LINE_HEIGHT / DEFAULT_FONT_SIZE);
        let font_family = font.family;
        let swash_cache = SwashCache::new();
        let cache = Cache::new(&device);
        // The surface is a non-sRGB Unorm format and the background/cell quads
        // write sRGB values directly. ColorMode::Web makes glyphon do the same
        // (no sRGB->linear conversion) so text isn't rendered too dark.
        let mut text_atlas =
            TextAtlas::with_color_mode(&device, &queue, &cache, format, ColorMode::Web);
        let text_renderer =
            TextRenderer::new(&mut text_atlas, &device, MultisampleState::default(), None);
        let viewport = Viewport::new(&device, &cache);

        let mut font_system = font_load.join();
        let (cell_width, cell_height) =
            measure_cell(&mut font_system, font_size, line_height, font_family.as_deref());

        let bg_pipeline = create_bg_pipeline(&device, format);
        let bg_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("spaceterm bg vertices"),
            size: BG_BUFFER_SIZE,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let cols = (width as f32 / cell_width).floor() as usize;
        let rows = (height as f32 / cell_height).floor() as usize;

        Self {
            device,
            queue,
            surface,
            config,
            font_system,
            swash_cache,
            cache,
            text_atlas,
            text_renderer,
            viewport,
            bg_pipeline,
            bg_buffer,
            cell_width,
            cell_height,
            cols: cols.max(1),
            rows: rows.max(1),
            font_family,
            font_size,
            line_height,
            theme: Theme::default(),
            text_buffers: Vec::new(),
            status_buffer: None,
        }
    }

    /// Apply a new color theme.
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    /// Current theme colors.
    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// The number of terminal columns and rows that fit the full viewport.
    pub fn grid_size(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    /// The number of columns and rows that fit within a specific rect.
    pub fn grid_size_for(&self, rect: PaneRect) -> (usize, usize) {
        let cols = (rect.width / self.cell_width).floor() as usize;
        let rows = (rect.height / self.cell_height).floor() as usize;
        (cols.max(1), rows.max(1))
    }

    /// The cell dimensions in pixels.
    pub fn cell_size(&self) -> (f32, f32) {
        (self.cell_width, self.cell_height)
    }

    /// Resize the surface and recompute grid dimensions. Returns `(cols, rows)`.
    pub fn resize(&mut self, width: u32, height: u32) -> (usize, usize) {
        let width = width.max(1);
        let height = height.max(1);
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.viewport
            .update(&self.queue, glyphon::Resolution { width, height });
        self.cols = (width as f32 / self.cell_width).floor() as usize;
        self.rows = (height as f32 / self.cell_height).floor() as usize;
        (self.cols.max(1), self.rows.max(1))
    }

    /// Render multiple panes to the surface. Each `PaneView` specifies a grid
    /// and its viewport rect. Pane dividers are drawn between adjacent panes.
    /// When `status` is set, a status bar is drawn across the bottom cell row;
    /// callers must leave that row free of panes (see [`Self::cell_size`]).
    pub fn render(&mut self, panes: &[PaneView], status: Option<&StatusBar>, bell_active: bool) {
        let surface_texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) => t,
            wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            _ => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
        };

        let view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let surface_w = self.config.width as f32;
        let surface_h = self.config.height as f32;

        let resolution = glyphon::Resolution {
            width: self.config.width,
            height: self.config.height,
        };
        self.viewport.update(&self.queue, resolution);

        let mut all_bg_verts = Vec::new();
        // Reuse last frame's buffers so unchanged lines keep their cached shaping.
        let mut text_buffers = std::mem::take(&mut self.text_buffers);
        text_buffers.truncate(panes.len());

        let fam = self.font_family.as_deref();

        for (pane_idx, pane) in panes.iter().enumerate() {
            let grid = pane.grid;
            let rect = pane.rect;
            let pane_cols = (rect.width / self.cell_width).floor() as usize;
            let pane_rows = (rect.height / self.cell_height).floor() as usize;

            let bg_verts = build_bg_vertices_offset(
                grid,
                BgParams {
                    cw: self.cell_width,
                    ch: self.cell_height,
                    hide_cursor: pane.nav_cursor.is_some(),
                    surface_w,
                    surface_h,
                    offset_x: rect.x,
                    offset_y: rect.y,
                    selection: pane.selection,
                    labels: pane.labels,
                    theme: &self.theme,
                },
            );
            all_bg_verts.extend_from_slice(&bg_verts);

            if let Some((nav_row, nav_col)) = pane.nav_cursor {
                if nav_row < pane_rows && nav_col < pane_cols {
                    let px0 = rect.x + nav_col as f32 * self.cell_width;
                    let py0 = rect.y + nav_row as f32 * self.cell_height;
                    all_bg_verts.extend_from_slice(&quad_vertices(
                        px0,
                        py0,
                        px0 + self.cell_width,
                        py0 + self.cell_height,
                        self.theme.cursor_bg.as_linear(),
                        surface_w,
                        surface_h,
                    ));
                }
            }

            let default_attrs = Attrs::new().family(base_family(fam));
            let mut rows_data: Vec<(String, glyphon::AttrsList)> = Vec::with_capacity(pane_rows);

            let sel = pane.selection;
            let sel_norm = sel.map(|(r1, c1, r2, c2)| {
                if (r1, c1) > (r2, c2) {
                    (r2, c2, r1, c1)
                } else {
                    (r1, c1, r2, c2)
                }
            });

            let label_map: std::collections::HashMap<(usize, usize), char> = pane
                .labels
                .map(|l| l.iter().map(|&(r, c, ch)| ((r, c), ch)).collect())
                .unwrap_or_default();

            for row in 0..grid.rows().min(pane_rows) {
                let mut text = String::with_capacity(grid.cols());
                let mut attrs_list = glyphon::AttrsList::new(&default_attrs);

                for col in 0..grid.cols().min(pane_cols) {
                    let start = text.len();
                    let cell = grid.visible_cell(row, col);

                    let label_char = label_map.get(&(row, col)).copied();
                    let ch = label_char.unwrap_or_else(|| cell.map(|c| c.ch).unwrap_or(' '));
                    text.push(ch);

                    if label_char.is_some() {
                        let label_color = Color::rgba(255, 200, 50, 255);
                        let label_attrs = Attrs::new()
                            .family(base_family(fam))
                            .weight(glyphon::cosmic_text::Weight::BOLD)
                            .color(label_color);
                        attrs_list.add_span(start..start + 1, &label_attrs);
                    } else if sel_norm.is_some_and(|(sr1, sc1, sr2, sc2)| {
                        (row, col) >= (sr1, sc1) && (row, col) <= (sr2, sc2)
                    }) {
                        let span_attrs = Attrs::new()
                            .family(base_family(fam))
                            .color(self.theme.selection_fg.to_glyphon());
                        attrs_list.add_span(start..start + 1, &span_attrs);
                    } else if let Some(cell) = cell {
                        let mut attrs = Attrs::new().family(base_family(fam));

                        if cell.style.bold {
                            attrs = attrs.weight(glyphon::cosmic_text::Weight::BOLD);
                        }
                        if cell.style.italic {
                            attrs = attrs.style(glyphon::cosmic_text::Style::Italic);
                        }

                        match cell.style.foreground {
                            GridColor::Rgb(rgb) => {
                                attrs = attrs.color(Color::rgba(rgb.r, rgb.g, rgb.b, 255));
                            }
                            GridColor::Indexed(idx) => {
                                let (r, g, b) = theme_indexed_color(&self.theme, idx);
                                attrs = attrs.color(Color::rgba(r, g, b, 255));
                            }
                            GridColor::Default => {}
                        }

                        if cell.style.foreground != GridColor::Default
                            || cell.style.bold
                            || cell.style.italic
                        {
                            attrs_list.add_span(start..start + 1, &attrs);
                        }
                    }
                }

                rows_data.push((text, attrs_list));
            }

            if pane_idx >= text_buffers.len() {
                text_buffers.push(glyphon::Buffer::new(
                    &mut self.font_system,
                    glyphon::Metrics::new(self.font_size, self.line_height),
                ));
            }
            let buffer = &mut text_buffers[pane_idx];
            let ending = glyphon::cosmic_text::LineEnding::default();
            let row_count = rows_data.len();
            for (i, (text, attrs_list)) in rows_data.into_iter().enumerate() {
                // set_text only resets shaping when the text or attrs differ, so
                // unchanged lines keep their cached glyphs and are not reshaped.
                if i < buffer.lines.len() {
                    buffer.lines[i].set_text(&text, ending, attrs_list);
                } else {
                    buffer
                        .lines
                        .push(BufferLine::new(&text, ending, attrs_list, Shaping::Advanced));
                }
            }
            buffer.lines.truncate(row_count);
            buffer.shape_until_scroll(&mut self.font_system, false);
        }

        if panes.len() > 1 {
            for i in 0..panes.len() {
                for j in (i + 1)..panes.len() {
                    let a = panes[i].rect;
                    let b = panes[j].rect;
                    let divider =
                        compute_divider(a, b, surface_w, surface_h, self.theme.divider.as_linear());
                    if let Some(dv) = divider {
                        all_bg_verts.extend_from_slice(&dv);
                    }
                }
            }
        }

        let mut status_buffer = self.status_buffer.take().unwrap_or_else(|| {
            glyphon::Buffer::new(
                &mut self.font_system,
                glyphon::Metrics::new(self.font_size, self.line_height),
            )
        });
        let status_top = surface_h - self.cell_height;
        if let Some(status) = status {
            let label_width = status.label.chars().count() as f32 * self.cell_width;
            all_bg_verts.extend_from_slice(&quad_vertices(
                0.0,
                status_top,
                surface_w,
                surface_h,
                self.theme.status_bar_bg.as_linear(),
                surface_w,
                surface_h,
            ));
            all_bg_verts.extend_from_slice(&quad_vertices(
                0.0,
                status_top,
                label_width,
                surface_h,
                status.accent.as_linear(),
                surface_w,
                surface_h,
            ));

            let attrs = Attrs::new()
                .family(base_family(fam))
                .weight(glyphon::cosmic_text::Weight::BOLD)
                .color(self.theme.status_bar_fg.to_glyphon());
            let attrs_list = glyphon::AttrsList::new(&attrs);
            let ending = glyphon::cosmic_text::LineEnding::default();
            if status_buffer.lines.is_empty() {
                status_buffer
                    .lines
                    .push(BufferLine::new(&status.label, ending, attrs_list, Shaping::Advanced));
            } else {
                status_buffer.lines[0].set_text(&status.label, ending, attrs_list);
            }
            status_buffer.lines.truncate(1);
            status_buffer.shape_until_scroll(&mut self.font_system, false);
        }

        if bell_active {
            let (r, g, b) = self.theme.bell.as_linear();
            let ndc_x0 = -1.0_f32;
            let ndc_y0 = 1.0_f32;
            let ndc_x1 = 1.0_f32;
            let ndc_y1 = -1.0_f32;
            all_bg_verts.push(BgVertex {
                x: ndc_x0,
                y: ndc_y0,
                r,
                g,
                b,
            });
            all_bg_verts.push(BgVertex {
                x: ndc_x1,
                y: ndc_y0,
                r,
                g,
                b,
            });
            all_bg_verts.push(BgVertex {
                x: ndc_x0,
                y: ndc_y1,
                r,
                g,
                b,
            });
            all_bg_verts.push(BgVertex {
                x: ndc_x1,
                y: ndc_y0,
                r,
                g,
                b,
            });
            all_bg_verts.push(BgVertex {
                x: ndc_x1,
                y: ndc_y1,
                r,
                g,
                b,
            });
            all_bg_verts.push(BgVertex {
                x: ndc_x0,
                y: ndc_y1,
                r,
                g,
                b,
            });
        }

        let bg_count = all_bg_verts.len() as u32;
        let bg_bytes: Vec<u8> = all_bg_verts.iter().flat_map(|v| v.to_bytes()).collect();
        self.queue.write_buffer(&self.bg_buffer, 0, &bg_bytes);

        let mut text_areas: Vec<TextArea> = text_buffers
            .iter()
            .zip(panes.iter())
            .map(|(buffer, pane)| TextArea {
                buffer,
                left: pane.rect.x,
                top: pane.rect.y,
                bounds: TextBounds {
                    left: pane.rect.x as i32,
                    top: pane.rect.y as i32,
                    right: (pane.rect.x + pane.rect.width) as i32,
                    bottom: (pane.rect.y + pane.rect.height) as i32,
                },
                default_color: self.theme.foreground.to_glyphon(),
                scale: 1.0,
                custom_glyphs: &[],
            })
            .collect();

        if status.is_some() {
            text_areas.push(TextArea {
                buffer: &status_buffer,
                left: 0.0,
                top: status_top,
                bounds: TextBounds {
                    left: 0,
                    top: status_top as i32,
                    right: surface_w as i32,
                    bottom: surface_h as i32,
                },
                default_color: self.theme.status_bar_fg.to_glyphon(),
                scale: 1.0,
                custom_glyphs: &[],
            });
        }

        self.text_renderer
            .prepare(
                &self.device,
                &self.queue,
                &mut self.font_system,
                &mut self.text_atlas,
                &self.viewport,
                text_areas,
                &mut self.swash_cache,
            )
            .expect("glyphon prepare");

        // Hand the buffers back for reuse next frame (preserves shape caches).
        self.text_buffers = text_buffers;
        self.status_buffer = Some(status_buffer);

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("spaceterm frame"),
            });

        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("spaceterm clear + bg"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: LoadOp::Clear(wgpu::Color {
                            r: self.theme.background.r as f64 / 255.0,
                            g: self.theme.background.g as f64 / 255.0,
                            b: self.theme.background.b as f64 / 255.0,
                            a: 1.0,
                        }),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            if bg_count > 0 {
                pass.set_pipeline(&self.bg_pipeline);
                pass.set_vertex_buffer(0, self.bg_buffer.slice(..));
                pass.draw(0..bg_count, 0..1);
            }

            self.text_renderer
                .render(&self.text_atlas, &self.viewport, &mut pass)
                .expect("glyphon render");
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        surface_texture.present();
    }
}

// ========================================================================
// Background vertex construction
// ========================================================================

const QUICK_SELECT_BG: (f32, f32, f32) = (0.6, 0.45, 0.1);

impl BgVertex {
    fn to_bytes(self) -> [u8; 20] {
        let mut out = [0u8; 20];
        out[0..4].copy_from_slice(&self.x.to_le_bytes());
        out[4..8].copy_from_slice(&self.y.to_le_bytes());
        out[8..12].copy_from_slice(&self.r.to_le_bytes());
        out[12..16].copy_from_slice(&self.g.to_le_bytes());
        out[16..20].copy_from_slice(&self.b.to_le_bytes());
        out
    }
}

/// Parameters for building background vertices for one pane.
struct BgParams<'a> {
    cw: f32,
    ch: f32,
    hide_cursor: bool,
    labels: Option<&'a [(usize, usize, char)]>,
    offset_x: f32,
    offset_y: f32,
    selection: Option<(usize, usize, usize, usize)>,
    surface_h: f32,
    surface_w: f32,
    theme: &'a Theme,
}

fn build_bg_vertices_offset(grid: &Grid, params: BgParams) -> Vec<BgVertex> {
    let BgParams {
        cw,
        ch,
        hide_cursor,
        surface_w,
        surface_h,
        offset_x,
        offset_y,
        selection,
        labels,
        theme,
    } = params;
    let mut verts = Vec::new();
    let (cursor_row, cursor_col) = grid.cursor();

    let sel_norm = selection.map(|(r1, c1, r2, c2)| {
        if (r1, c1) > (r2, c2) {
            (r2, c2, r1, c1)
        } else {
            (r1, c1, r2, c2)
        }
    });

    let label_set: std::collections::HashSet<(usize, usize)> = labels
        .map(|l| l.iter().map(|&(r, c, _)| (r, c)).collect())
        .unwrap_or_default();

    for row in 0..grid.rows() {
        for col in 0..grid.cols() {
            let cell = grid.cell(row, col);
            let is_cursor = !hide_cursor && row == cursor_row && col == cursor_col;
            let bg = cell.map(|c| c.style.background).unwrap_or_default();
            let draw_bg = is_cursor || !matches!(bg, GridColor::Default);

            let is_selected = sel_norm.is_some_and(|(sr1, sc1, sr2, sc2)| {
                (row, col) >= (sr1, sc1) && (row, col) <= (sr2, sc2)
            });

            let is_label = label_set.contains(&(row, col));

            if draw_bg || is_selected || is_label {
                let (r, g, b) = if is_label {
                    QUICK_SELECT_BG
                } else if is_selected {
                    theme.selection_bg.as_linear()
                } else if is_cursor {
                    theme.cursor_bg.as_linear()
                } else {
                    grid_color_to_rgb(&bg, theme)
                };

                let px0 = offset_x + col as f32 * cw;
                let py0 = offset_y + row as f32 * ch;
                let (px0, py0, px1, py1) = if is_cursor {
                    let px1_full = px0 + cw;
                    let py1_full = py0 + ch;
                    match grid.cursor_shape() {
                        CursorShape::Block => (px0, py0, px1_full, py1_full),
                        CursorShape::Bar => {
                            let px1 = px0 + cw * CURSOR_BAR_WIDTH_RATIO;
                            (px0, py0, px1, py1_full)
                        }
                        CursorShape::Underline => {
                            let py0_new = py1_full - ch * CURSOR_UNDERLINE_HEIGHT_RATIO;
                            (px0, py0_new, px1_full, py1_full)
                        }
                    }
                } else {
                    (px0, py0, px0 + cw, py0 + ch)
                };

                let ndc_x0 = px0 * 2.0 / surface_w - 1.0;
                let ndc_y0 = 1.0 - py0 * 2.0 / surface_h;
                let ndc_x1 = px1 * 2.0 / surface_w - 1.0;
                let ndc_y1 = 1.0 - py1 * 2.0 / surface_h;

                verts.push(BgVertex {
                    x: ndc_x0,
                    y: ndc_y0,
                    r,
                    g,
                    b,
                });
                verts.push(BgVertex {
                    x: ndc_x1,
                    y: ndc_y0,
                    r,
                    g,
                    b,
                });
                verts.push(BgVertex {
                    x: ndc_x0,
                    y: ndc_y1,
                    r,
                    g,
                    b,
                });
                verts.push(BgVertex {
                    x: ndc_x1,
                    y: ndc_y0,
                    r,
                    g,
                    b,
                });
                verts.push(BgVertex {
                    x: ndc_x1,
                    y: ndc_y1,
                    r,
                    g,
                    b,
                });
                verts.push(BgVertex {
                    x: ndc_x0,
                    y: ndc_y1,
                    r,
                    g,
                    b,
                });
            }
        }
    }

    verts
}

const DIVIDER_THICKNESS: f32 = 1.0;

fn compute_divider(
    a: PaneRect,
    b: PaneRect,
    surface_w: f32,
    surface_h: f32,
    divider_color: (f32, f32, f32),
) -> Option<[BgVertex; 6]> {
    let vertical = (a.y - b.y).abs() < 1.0 && a.height == b.height;
    let horizontal = (a.x - b.x).abs() < 1.0 && a.width == b.width;
    if !vertical && !horizontal {
        return None;
    }

    let (px0, py0, px1, py1) = if vertical {
        let x = if a.x < b.x {
            a.x + a.width
        } else {
            b.x + b.width
        };
        let x = x - DIVIDER_THICKNESS / 2.0;
        (x, a.y.min(b.y), x + DIVIDER_THICKNESS, a.y + a.height)
    } else {
        let y = if a.y < b.y {
            a.y + a.height
        } else {
            b.y + b.height
        };
        let y = y - DIVIDER_THICKNESS / 2.0;
        (a.x.min(b.x), y, a.x + a.width, y + DIVIDER_THICKNESS)
    };

    let (r, g, b) = divider_color;
    let ndc_x0 = px0 * 2.0 / surface_w - 1.0;
    let ndc_y0 = 1.0 - py0 * 2.0 / surface_h;
    let ndc_x1 = px1 * 2.0 / surface_w - 1.0;
    let ndc_y1 = 1.0 - py1 * 2.0 / surface_h;

    Some([
        BgVertex {
            x: ndc_x0,
            y: ndc_y0,
            r,
            g,
            b,
        },
        BgVertex {
            x: ndc_x1,
            y: ndc_y0,
            r,
            g,
            b,
        },
        BgVertex {
            x: ndc_x0,
            y: ndc_y1,
            r,
            g,
            b,
        },
        BgVertex {
            x: ndc_x1,
            y: ndc_y0,
            r,
            g,
            b,
        },
        BgVertex {
            x: ndc_x1,
            y: ndc_y1,
            r,
            g,
            b,
        },
        BgVertex {
            x: ndc_x0,
            y: ndc_y1,
            r,
            g,
            b,
        },
    ])
}

// ========================================================================
// Background pipeline
// ========================================================================

fn create_bg_pipeline(device: &Device, format: TextureFormat) -> RenderPipeline {
    let shader = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("spaceterm bg shader"),
        source: ShaderSource::Wgsl(BG_SHADER.into()),
    });

    let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some("spaceterm bg layout"),
        bind_group_layouts: &[],
        immediate_size: 0,
    });

    device.create_render_pipeline(&RenderPipelineDescriptor {
        label: Some("spaceterm bg pipeline"),
        layout: Some(&layout),
        vertex: VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[VertexBufferLayout {
                array_stride: std::mem::size_of::<BgVertex>() as u64,
                step_mode: VertexStepMode::Vertex,
                attributes: &[
                    VertexAttribute {
                        offset: 0,
                        format: VertexFormat::Float32x2,
                        shader_location: 0,
                    },
                    VertexAttribute {
                        offset: 8,
                        format: VertexFormat::Float32x3,
                        shader_location: 1,
                    },
                ],
            }],
        },
        fragment: Some(FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: ColorWrites::ALL,
            })],
        }),
        primitive: PrimitiveState {
            topology: PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: FrontFace::Ccw,
            cull_mode: None,
            polygon_mode: PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        },
        depth_stencil: None,
        multisample: MultisampleState::default(),
        cache: None,
        multiview_mask: None,
    })
}

// ========================================================================
// Helpers
// ========================================================================

/// Two triangles covering the pixel rect `[px0,px1] x [py0,py1]` in `color`,
/// converted to normalized device coordinates for the bg pipeline.
fn quad_vertices(
    px0: f32,
    py0: f32,
    px1: f32,
    py1: f32,
    color: (f32, f32, f32),
    surface_w: f32,
    surface_h: f32,
) -> [BgVertex; 6] {
    let (r, g, b) = color;
    let ndc_x0 = px0 * 2.0 / surface_w - 1.0;
    let ndc_y0 = 1.0 - py0 * 2.0 / surface_h;
    let ndc_x1 = px1 * 2.0 / surface_w - 1.0;
    let ndc_y1 = 1.0 - py1 * 2.0 / surface_h;

    [
        BgVertex {
            x: ndc_x0,
            y: ndc_y0,
            r,
            g,
            b,
        },
        BgVertex {
            x: ndc_x1,
            y: ndc_y0,
            r,
            g,
            b,
        },
        BgVertex {
            x: ndc_x0,
            y: ndc_y1,
            r,
            g,
            b,
        },
        BgVertex {
            x: ndc_x1,
            y: ndc_y0,
            r,
            g,
            b,
        },
        BgVertex {
            x: ndc_x1,
            y: ndc_y1,
            r,
            g,
            b,
        },
        BgVertex {
            x: ndc_x0,
            y: ndc_y1,
            r,
            g,
            b,
        },
    ]
}

/// The cosmic-text font family for a configured family name, defaulting to the
/// system monospace when no family is set.
fn base_family(name: Option<&str>) -> Family<'_> {
    match name {
        Some(n) => Family::Name(n),
        None => Family::Monospace,
    }
}

fn measure_cell(
    font_system: &mut FontSystem,
    font_size: f32,
    line_height: f32,
    family: Option<&str>,
) -> (f32, f32) {
    let metrics = glyphon::Metrics::new(font_size, line_height);
    let mut buffer = glyphon::Buffer::new(font_system, metrics);
    buffer.set_size(font_system, Some(f32::MAX), Some(line_height));
    let attrs = Attrs::new().family(base_family(family));
    buffer.set_text(font_system, "M", &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(font_system, false);

    if let Some(run) = buffer.layout_runs().next() {
        let glyph_w = run.glyphs.first().map(|g| g.w).unwrap_or(font_size * 0.6);
        return (glyph_w, line_height);
    }

    (font_size * 0.6, line_height)
}

fn grid_color_to_rgb(color: &GridColor, theme: &Theme) -> (f32, f32, f32) {
    match color {
        GridColor::Default => theme.background.as_linear(),
        GridColor::Indexed(i) => {
            let (r, g, b) = theme_indexed_color(theme, *i);
            (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
        }
        GridColor::Rgb(RgbColor { r, g, b }) => {
            (*r as f32 / 255.0, *g as f32 / 255.0, *b as f32 / 255.0)
        }
    }
}

/// Resolve a 256-color palette index to RGB using the theme: ANSI 0-15 and any
/// custom indexed overrides come from the theme; the rest fall back to the
/// standard xterm 256-color cube and grey ramp.
fn theme_indexed_color(theme: &Theme, index: u8) -> (u8, u8, u8) {
    if (index as usize) < 16 {
        return theme.ansi_color(index);
    }
    if let Some(rgb) = theme.indexed_color(index) {
        return rgb;
    }
    xterm_256_to_rgb(index)
}

fn xterm_256_to_rgb(index: u8) -> (u8, u8, u8) {
    if index < 16 {
        return ANSI_COLORS[index as usize];
    }
    if index < 232 {
        let i = index - 16;
        let b_val = i % 6;
        let g_val = (i / 6) % 6;
        let r_val = (i / 36) % 6;
        return (
            if r_val > 0 { 55 + 40 * r_val } else { 0 },
            if g_val > 0 { 55 + 40 * g_val } else { 0 },
            if b_val > 0 { 55 + 40 * b_val } else { 0 },
        );
    }
    let grey = 8 + 10 * (index - 232);
    (grey, grey, grey)
}

const ANSI_COLORS: [(u8, u8, u8); 16] = [
    (0, 0, 0),
    (128, 0, 0),
    (0, 128, 0),
    (128, 128, 0),
    (0, 0, 128),
    (128, 0, 128),
    (0, 128, 128),
    (192, 192, 192),
    (128, 128, 128),
    (255, 0, 0),
    (0, 255, 0),
    (255, 255, 0),
    (0, 0, 255),
    (255, 0, 255),
    (0, 255, 255),
    (255, 255, 255),
];

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xterm_256_first_16_are_ansi() {
        assert_eq!(xterm_256_to_rgb(0), (0, 0, 0));
        assert_eq!(xterm_256_to_rgb(1), (128, 0, 0));
        assert_eq!(xterm_256_to_rgb(7), (192, 192, 192));
        assert_eq!(xterm_256_to_rgb(15), (255, 255, 255));
    }

    #[test]
    fn test_xterm_256_cube() {
        let (r, g, b) = xterm_256_to_rgb(16 + 36 + 6 + 1);
        assert!(r > 0);
        assert!(g > 0);
        assert!(b > 0);
    }

    #[test]
    fn test_xterm_256_grey_ramp() {
        let (r, g, b) = xterm_256_to_rgb(232);
        assert_eq!(r, g);
        assert_eq!(g, b);
        assert!(r >= 8);
    }

    #[test]
    fn test_grid_color_default() {
        let theme = Theme::default();
        let (r, g, b) = grid_color_to_rgb(&GridColor::Default, &theme);
        assert_eq!((r, g, b), theme.background.as_linear());
    }

    #[test]
    fn test_grid_color_rgb() {
        let theme = Theme::default();
        let (r, g, b) = grid_color_to_rgb(
            &GridColor::Rgb(RgbColor {
                r: 255,
                g: 128,
                b: 0,
            }),
            &theme,
        );
        assert!((r - 1.0).abs() < 0.01);
        assert!((g - 0.5).abs() < 0.01);
        assert!((b - 0.0).abs() < 0.01);
    }

    #[test]
    fn test_bg_vertex_bytes_roundtrip() {
        let v = BgVertex {
            x: 1.0,
            y: -1.0,
            r: 0.5,
            g: 0.25,
            b: 0.0,
        };
        let bytes = v.to_bytes();
        assert_eq!(bytes.len(), 20);
    }

    #[test]
    fn test_build_bg_vertices_empty_grid() {
        let grid = Grid::new(4, 2);
        let verts = build_bg_vertices_offset(
            &grid,
            BgParams {
                cw: 10.0,
                ch: 20.0,
                hide_cursor: false,
                surface_w: 800.0,
                surface_h: 600.0,
                offset_x: 0.0,
                offset_y: 0.0,
                selection: None,
                labels: None,
                theme: &Theme::default(),
            },
        );
        assert_eq!(verts.len(), 6);
    }

    #[test]
    fn test_build_bg_vertices_cursor_only() {
        let mut grid = Grid::new(4, 2);
        grid.print('a');
        let verts = build_bg_vertices_offset(
            &grid,
            BgParams {
                cw: 10.0,
                ch: 20.0,
                hide_cursor: false,
                surface_w: 800.0,
                surface_h: 600.0,
                offset_x: 0.0,
                offset_y: 0.0,
                selection: None,
                labels: None,
                theme: &Theme::default(),
            },
        );
        assert_eq!(verts.len(), 6);
    }

    #[test]
    fn test_build_bg_vertices_offset_produces_same_count() {
        let mut grid = Grid::new(4, 2);
        grid.print('a');
        let with_offset = build_bg_vertices_offset(
            &grid,
            BgParams {
                cw: 10.0,
                ch: 20.0,
                hide_cursor: false,
                surface_w: 800.0,
                surface_h: 600.0,
                offset_x: 100.0,
                offset_y: 50.0,
                selection: None,
                labels: None,
                theme: &Theme::default(),
            },
        );
        let without_offset = build_bg_vertices_offset(
            &grid,
            BgParams {
                cw: 10.0,
                ch: 20.0,
                hide_cursor: false,
                surface_w: 800.0,
                surface_h: 600.0,
                offset_x: 0.0,
                offset_y: 0.0,
                selection: None,
                labels: None,
                theme: &Theme::default(),
            },
        );
        assert_eq!(with_offset.len(), without_offset.len());
    }

    #[test]
    fn test_build_bg_vertices_with_selection() {
        let mut grid = Grid::new(4, 2);
        grid.print('a');
        let verts = build_bg_vertices_offset(
            &grid,
            BgParams {
                cw: 10.0,
                ch: 20.0,
                hide_cursor: false,
                surface_w: 800.0,
                surface_h: 600.0,
                offset_x: 0.0,
                offset_y: 0.0,
                selection: Some((0, 0, 0, 1)),
                labels: None,
                theme: &Theme::default(),
            },
        );
        assert!(verts.len() >= 12, "selection adds extra quads");
    }

    #[test]
    fn test_build_bg_vertices_with_labels() {
        let mut grid = Grid::new(4, 2);
        grid.print('a');
        let labels: &[(usize, usize, char)] = &[(0, 0, 's'), (1, 2, 'd')];
        let verts = build_bg_vertices_offset(
            &grid,
            BgParams {
                cw: 10.0,
                ch: 20.0,
                hide_cursor: false,
                surface_w: 800.0,
                surface_h: 600.0,
                offset_x: 0.0,
                offset_y: 0.0,
                selection: None,
                labels: Some(labels),
                theme: &Theme::default(),
            },
        );
        assert!(
            verts.len() >= 12,
            "label cells add extra quads beyond cursor"
        );
    }

    #[test]
    fn test_pane_rect_conversion() {
        let rect = PaneRect {
            x: 100.0,
            y: 50.0,
            width: 400.0,
            height: 300.0,
        };
        assert_eq!(rect.width, 400.0);
        assert_eq!(rect.height, 300.0);
    }
}
