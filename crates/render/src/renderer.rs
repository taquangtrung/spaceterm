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

use crate::chrome::{self, layout as chrome_layout, DropdownLayout, MenuItem, Region, TopChrome};
use crate::grid::{Color as GridColor, CursorShape, Grid, RgbColor};
use crate::image::{ImagePass, ImagePlacement};
use crate::theme::{Rgb, Theme};

// ========================================================================
// Constants
// ========================================================================

const DEFAULT_FONT_SIZE: f32 = 15.0;
const DEFAULT_LINE_HEIGHT: f32 = 20.0;
/// The bundled default font family, used when the user has set no `font`. Its
/// faces are embedded below and loaded into the font database at startup, so
/// this resolves identically on every machine regardless of installed fonts.
const DEFAULT_FONT_FAMILY: &str = "FiraCode Nerd Font Mono";
/// Default weight for bold/bright cells when the user has set no `font-weight-bold`.
/// SemiBold reads as clearly bolder without the chunky look of a 700 face; the
/// bundled font ships this exact weight, so no cross-family substitution occurs.
const DEFAULT_BOLD_WEIGHT: glyphon::cosmic_text::Weight = glyphon::cosmic_text::Weight::SEMIBOLD;
/// FiraCode Nerd Font Mono faces embedded into the binary (SIL OFL 1.1, see
/// `assets/fonts/LICENSE-FiraCode-OFL.txt`). Only Regular (400) and SemiBold
/// (600) are bundled: the normal and default-bold weights, both kept in-family.
const BUNDLED_FONT_REGULAR: &[u8] =
    include_bytes!("../assets/fonts/FiraCodeNerdFontMono-Regular.ttf");
const BUNDLED_FONT_SEMIBOLD: &[u8] =
    include_bytes!("../assets/fonts/FiraCodeNerdFontMono-SemiBold.ttf");
const BG_SHADER: &str = include_str!("bg.wgsl");
const CURSOR_BAR_WIDTH_RATIO: f32 = 0.15;
const CURSOR_UNDERLINE_HEIGHT_RATIO: f32 = 0.2;
const BG_BUFFER_SIZE: u64 = 4 * 1024 * 1024;
const MAX_SVG_DIM: u32 = 4096;
/// Reserved image-pass texture ids for the rasterized menu overlays. Set to the
/// top of the id space so they never collide with block image ids.
const DROPDOWN_TEXTURE_ID: u64 = u64::MAX;
const SUBMENU_TEXTURE_ID: u64 = u64::MAX - 1;
/// Reserved id for the rasterized top-chrome strip (band + rounded-top tabs).
const CHROME_STRIP_TEXTURE_ID: u64 = u64::MAX - 2;
/// Reserved id for the rasterized command-palette overlay.
const PALETTE_TEXTURE_ID: u64 = u64::MAX - 3;
/// Reserved id for the rasterized bell-dot hover tooltip.
const BELL_TOOLTIP_TEXTURE_ID: u64 = u64::MAX - 4;
/// Maximum number of command results visible in the palette at once.
const PALETTE_MAX_ITEMS: usize = 8;
/// Palette panel width as a fraction of the surface width.
const PALETTE_WIDTH_RATIO: f32 = 0.62;
/// Palette panel top edge as a fraction of the surface height (VS Code style).
const PALETTE_TOP_RATIO: f32 = 0.15;
/// Corner radius of the dropdown menu panel and its hover highlight, in pixels.
const DROPDOWN_RADIUS: f32 = 12.0;
/// Width of the soft drop shadow cast around the dropdown panel, in pixels.
const DROPDOWN_SHADOW: f32 = 22.0;
/// Peak opacity of the dropdown drop shadow, fading to zero at its outer edge.
const DROPDOWN_SHADOW_ALPHA: f32 = 0.3;
/// Inset of the hover-highlight pill from the dropdown item-row edges, in pixels.
const MENU_HOVER_INSET: f32 = 6.0;
/// Strength of the dropdown panel's hairline border, mixed from the surface
/// toward white for a crisp, elevated edge against the content behind it.
const MENU_BORDER_MIX: f32 = 0.14;
/// Corner radius of the rounded tab tops, as a fraction of the cell height.
const TAB_CORNER_RADIUS_RATIO: f32 = 0.34;
/// Opacity of the hairline separating the tab band from the content below.
const CHROME_BORDER_ALPHA: f32 = 0.5;
/// Color of the dropdown drop shadow (alpha is applied per pixel).
const SHADOW_COLOR: Rgb = Rgb::new(0, 0, 0);

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
    /// The Normal-mode traversal cursor, in viewport `(row, col)`, drawn in
    /// [`Self::cursor_shape`]. `None` when the pane is not being navigated.
    pub nav_cursor: Option<(usize, usize)>,
    pub rect: PaneRect,
    pub selection: Option<(usize, usize, usize, usize)>,
    /// The shape used to draw whichever cursor is active: the in-grid shell
    /// cursor when `nav_cursor` is `None`, or the traversal cursor otherwise.
    /// Hosts pick this per pane from the active mode's configured shape.
    pub cursor_shape: CursorShape,
}

/// A Vim-style bottom status bar: the `label` (e.g. " NORMAL ") is drawn over a
/// segment filled with `accent`, atop a full-width strip in the theme's status
/// colors. Occupies the bottom-most cell row of the surface.
pub struct StatusBar {
    pub accent: Rgb,
    pub mode: String,
    /// A transient error notice; when set it takes the pane-title slot and is
    /// drawn in the theme's red to stand out.
    pub notice: Option<String>,
    pub pane_title: Option<String>,
    pub right_label: Option<String>,
}

/// One shaped text run of the top chrome (a tab title, a menu title, a glyph),
/// plus where to place and clip it. Built fresh each frame and kept alive until
/// after `glyphon` prepares the text pass.
struct ChromeText {
    bounds: TextBounds,
    buffer: glyphon::Buffer,
    color: Color,
    left: f32,
    top: f32,
}

/// Scalar font metrics needed to shape and lay out chrome text offscreen,
/// independent of the GPU. Bundled so the dropdown rasterizer (and its tests)
/// can run without a `Renderer`.
struct FontCtx<'a> {
    cell_h: f32,
    cell_w: f32,
    family: Option<&'a str>,
    font_size: f32,
    line_height: f32,
    normal_weight: Option<&'a str>,
    bold_weight: Option<&'a str>,
}

/// A rasterized dropdown overlay: its pixels and the surface position to place
/// them at (top-left, already offset to include the shadow margin).
struct DropdownImage {
    height: u32,
    rgba: Vec<u8>,
    width: u32,
    x: f32,
    y: f32,
}

/// One entry shown in the command palette results list.
pub struct PaletteItem {
    pub action: String,
    pub label: String,
    /// Char indices in `label` that matched the query, used to highlight them.
    pub match_positions: Vec<usize>,
}

/// The command palette state the renderer needs to draw its overlay.
pub struct PaletteView {
    /// Text shown when the filtered list is empty.
    pub empty_message: String,
    pub items: Vec<PaletteItem>,
    /// Draw an underline under highlighted match characters.
    pub match_underline: bool,
    pub query: String,
    pub selected: usize,
}

/// Font selection for the renderer. `family` is the primary family name (e.g.
/// "FiraCode Nerd Font"); `None` falls back to the bundled [`DEFAULT_FONT_FAMILY`].
/// Glyphs missing from the primary font are filled in from the system font
/// database automatically.
#[derive(Clone, Debug)]
pub struct FontConfig {
    pub family: Option<String>,
    pub size: f32,
    pub normal_weight: Option<String>,
    pub bold_weight: Option<String>,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: None,
            size: DEFAULT_FONT_SIZE,
            normal_weight: None,
            bold_weight: None,
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
    logical_font_size: f32,
    scale_factor: f64,
    normal_weight: Option<String>,
    bold_weight: Option<String>,
    theme: Theme,
    /// Persistent per-pane text buffers, reused across frames so cosmic-text
    /// only re-shapes lines whose content changed (one line per keystroke
    /// instead of the whole screen).
    text_buffers: Vec<glyphon::Buffer>,
    /// Persistent one-line buffer for the bottom status bar left segment.
    status_buffer: Option<glyphon::Buffer>,
    /// Persistent one-line buffer for the status bar right segment (right-aligned).
    status_right_buffer: Option<glyphon::Buffer>,
    /// Textured-quad pass for image blocks rendered natively (no webview).
    image_pass: ImagePass,
    /// Image pass for the rasterized top-chrome strip. Rendered between the bg
    /// quads and the text so the rounded tab cards sit under the tab titles.
    chrome_strip_pass: ImagePass,
    /// System fonts for SVG text, loaded lazily on first SVG with text and then
    /// reused (the scan costs ~150ms, so it is deferred off the startup path).
    svg_fontdb: Option<std::sync::Arc<resvg::usvg::fontdb::Database>>,
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
        scale_factor: f64,
        font: FontConfig,
        font_load: FontLoad,
    ) -> Self {
        let (device, queue) =
            pollster::block_on(adapter.request_device(&DeviceDescriptor::default()))
                .expect("request wgpu device");

        let caps = surface.get_capabilities(&adapter);
        // Prefer an sRGB surface so the GPU encodes linear shader output to sRGB
        // on write: glyph and overlay antialiasing then blends in linear space
        // (gamma-correct), which keeps light-on-dark text crisp instead of thin
        // and fuzzy. The bg/image shaders output linear to match.
        let format = caps
            .formats
            .iter()
            .find(|f| {
                matches!(
                    f,
                    TextureFormat::Bgra8UnormSrgb | TextureFormat::Rgba8UnormSrgb
                )
            })
            .or_else(|| {
                caps.formats
                    .iter()
                    .find(|f| matches!(f, TextureFormat::Bgra8Unorm | TextureFormat::Rgba8Unorm))
            })
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

        let font_size = font.size * scale_factor as f32;
        // Round to whole pixels so the line stride cosmic-text uses for shaping
        // matches `cell_height` (which `measure_cell` also rounds). Without this,
        // a fractional `line_height` (e.g. font_size 14 → 18.667) accumulates a
        // sub-pixel drift per row until the cursor — drawn at `row * cell_height` —
        // sits half a line below the glyphs cosmic-text laid out at
        // `row * line_height`.
        let line_height = (font_size * (DEFAULT_LINE_HEIGHT / DEFAULT_FONT_SIZE)).round();
        let font_family = font
            .family
            .or_else(|| Some(DEFAULT_FONT_FAMILY.to_string()));
        let normal_weight = font.normal_weight;
        let bold_weight = font.bold_weight;
        let swash_cache = SwashCache::new();
        let cache = Cache::new(&device);
        // ColorMode::Accurate makes glyphon convert text colors to linear and
        // blend glyph coverage in linear space, which the sRGB surface encodes
        // back on write: gamma-correct antialiasing, so text stays crisp rather
        // than thin and fuzzy on dark backgrounds.
        let color_mode = if format.is_srgb() {
            ColorMode::Accurate
        } else {
            ColorMode::Web
        };
        let mut text_atlas =
            TextAtlas::with_color_mode(&device, &queue, &cache, format, color_mode);
        let text_renderer =
            TextRenderer::new(&mut text_atlas, &device, MultisampleState::default(), None);
        let viewport = Viewport::new(&device, &cache);

        let mut font_system = font_load.join();
        load_bundled_fonts(&mut font_system);
        let (cell_width, cell_height) = measure_cell(
            &mut font_system,
            font_size,
            line_height,
            font_family.as_deref(),
            normal_weight.as_deref(),
        );

        let bg_pipeline = create_bg_pipeline(&device, format);
        let bg_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("spaceterm bg vertices"),
            size: BG_BUFFER_SIZE,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let cols = (width as f32 / cell_width).floor() as usize;
        let rows = (height as f32 / cell_height).floor() as usize;

        let image_pass = ImagePass::new(&device, format);
        let chrome_strip_pass = ImagePass::new(&device, format);

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
            logical_font_size: font.size,
            scale_factor,
            normal_weight,
            bold_weight,
            theme: Theme::default(),
            text_buffers: Vec::new(),
            status_buffer: None,
            status_right_buffer: None,
            image_pass,
            chrome_strip_pass,
            svg_fontdb: None,
        }
    }

    /// Apply a new color theme.
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    /// Decode `encoded` image bytes (PNG/JPEG/GIF/WebP) and cache them as a GPU
    /// texture under `id`. Returns the pixel dimensions, or `None` if the bytes
    /// could not be decoded.
    pub fn upload_image(&mut self, id: u64, encoded: &[u8]) -> Option<(u32, u32)> {
        let rgba = image::load_from_memory(encoded).ok()?.to_rgba8();
        let (width, height) = rgba.dimensions();
        self.image_pass
            .upload(&self.device, &self.queue, id, &rgba, width, height);
        Some((width, height))
    }

    /// Rasterize an SVG document (at its intrinsic size) and cache it as a GPU
    /// texture under `id`. Returns the rasterized pixel dimensions, or `None` if
    /// the SVG could not be parsed. (Rasterizing at intrinsic size keeps sizing
    /// consistent with raster images; display-size re-rasterization is a future
    /// refinement.)
    pub fn upload_svg(&mut self, id: u64, svg: &[u8]) -> Option<(u32, u32)> {
        let fontdb = self.svg_fontdb();
        let options = resvg::usvg::Options {
            fontdb,
            ..Default::default()
        };
        let tree = resvg::usvg::Tree::from_data(svg, &options).ok()?;
        let size = tree.size();
        if size.width() <= 0.0 || size.height() <= 0.0 {
            return None;
        }
        let width = (size.width().round() as u32).clamp(1, MAX_SVG_DIM);
        let height = (size.height().round() as u32).clamp(1, MAX_SVG_DIM);

        let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)?;
        let transform = resvg::tiny_skia::Transform::from_scale(
            width as f32 / size.width(),
            height as f32 / size.height(),
        );
        resvg::render(&tree, transform, &mut pixmap.as_mut());

        // tiny-skia stores premultiplied alpha; the image pass blends straight
        // alpha, so demultiply on the way out.
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for pixel in pixmap.pixels() {
            let color = pixel.demultiply();
            rgba.extend_from_slice(&[color.red(), color.green(), color.blue(), color.alpha()]);
        }

        self.image_pass
            .upload(&self.device, &self.queue, id, &rgba, width, height);
        Some((width, height))
    }

    /// The system-font database for SVG text, scanned once on first use and
    /// then reused.
    fn svg_fontdb(&mut self) -> std::sync::Arc<resvg::usvg::fontdb::Database> {
        if let Some(db) = &self.svg_fontdb {
            return db.clone();
        }
        let mut db = resvg::usvg::fontdb::Database::new();
        db.load_system_fonts();
        let db = std::sync::Arc::new(db);
        self.svg_fontdb = Some(db.clone());
        db
    }

    /// Lay out `markdown` with `cosmic-text` (wrapped to `wrap_width`), software-
    /// rasterize it over the theme background, and cache it as a GPU texture
    /// under `id`. Returns the rendered pixel dimensions.
    pub fn upload_markdown(
        &mut self,
        id: u64,
        markdown: &str,
        wrap_width: f32,
    ) -> Option<(u32, u32)> {
        let width = (wrap_width.floor() as u32).clamp(1, MAX_SVG_DIM);
        let spans = crate::markdown::parse(markdown);
        let fam = self.font_family.clone();
        let fg = self.theme.foreground.to_glyphon();
        let code_color = self.theme.ansi[3].to_glyphon();

        let mut buffer = glyphon::Buffer::new(
            &mut self.font_system,
            glyphon::Metrics::new(self.font_size, self.line_height),
        );
        buffer.set_size(&mut self.font_system, Some(width as f32), None);

        let default_attrs = Attrs::new().family(base_family(fam.as_deref())).color(fg);
        let attr_spans: Vec<(&str, Attrs)> = spans
            .iter()
            .map(|span| {
                let mut attrs = if span.mono {
                    Attrs::new().family(Family::Monospace).color(code_color)
                } else {
                    Attrs::new().family(base_family(fam.as_deref())).color(fg)
                };
                if span.bold {
                    attrs = attrs.weight(parse_weight(self.bold_weight.as_deref(), DEFAULT_BOLD_WEIGHT));
                }
                if span.italic {
                    attrs = attrs.style(glyphon::cosmic_text::Style::Italic);
                }
                (span.text.as_str(), attrs)
            })
            .collect();
        buffer.set_rich_text(
            &mut self.font_system,
            attr_spans,
            &default_attrs,
            Shaping::Advanced,
            None,
        );
        self.rasterize_buffer(id, buffer, width)
    }

    /// Lay out preformatted monospace text (CSV tables, pretty-printed JSON, ...)
    /// wrapped to `wrap_width` and rasterize it to a cached texture. Returns the
    /// rendered pixel dimensions.
    pub fn upload_text(&mut self, id: u64, text: &str, wrap_width: f32) -> Option<(u32, u32)> {
        let width = (wrap_width.floor() as u32).clamp(1, MAX_SVG_DIM);
        let attrs = Attrs::new()
            .family(Family::Monospace)
            .color(self.theme.foreground.to_glyphon());
        let mut buffer = glyphon::Buffer::new(
            &mut self.font_system,
            glyphon::Metrics::new(self.font_size, self.line_height),
        );
        buffer.set_size(&mut self.font_system, Some(width as f32), None);
        buffer.set_rich_text(
            &mut self.font_system,
            [(text, attrs.clone())],
            &attrs,
            Shaping::Advanced,
            None,
        );
        self.rasterize_buffer(id, buffer, width)
    }

    /// Shape `buffer`, measure its height, software-rasterize its glyphs over an
    /// opaque themed background, and cache the result as a texture under `id`.
    fn rasterize_buffer(
        &mut self,
        id: u64,
        mut buffer: glyphon::Buffer,
        width: u32,
    ) -> Option<(u32, u32)> {
        buffer.shape_until_scroll(&mut self.font_system, false);
        let mut content_h = 0.0_f32;
        for run in buffer.layout_runs() {
            content_h = content_h.max(run.line_top + run.line_height);
        }
        let height = (content_h.ceil() as u32).clamp(1, MAX_SVG_DIM);

        // Software-composite glyph coverage over an opaque themed background.
        let bg = self.theme.background;
        let fg = self.theme.foreground.to_glyphon();
        let mut rgba = vec![0u8; (width * height * 4) as usize];
        for pixel in rgba.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[bg.r, bg.g, bg.b, 255]);
        }
        let font_system = &mut self.font_system;
        let swash_cache = &mut self.swash_cache;
        buffer.draw(font_system, swash_cache, fg, |x, y, w, h, color| {
            let alpha = color.a() as f32 / 255.0;
            if alpha <= 0.0 {
                return;
            }
            let (cr, cg, cb) = (color.r() as f32, color.g() as f32, color.b() as f32);
            for py in y..y + h as i32 {
                for px in x..x + w as i32 {
                    if px < 0 || py < 0 || px >= width as i32 || py >= height as i32 {
                        continue;
                    }
                    let idx = ((py as u32 * width + px as u32) * 4) as usize;
                    rgba[idx] = (cr * alpha + rgba[idx] as f32 * (1.0 - alpha)) as u8;
                    rgba[idx + 1] = (cg * alpha + rgba[idx + 1] as f32 * (1.0 - alpha)) as u8;
                    rgba[idx + 2] = (cb * alpha + rgba[idx + 2] as f32 * (1.0 - alpha)) as u8;
                }
            }
        });

        self.image_pass
            .upload(&self.device, &self.queue, id, &rgba, width, height);
        Some((width, height))
    }

    /// Whether an image texture is already cached for `id`.
    pub fn has_image(&self, id: u64) -> bool {
        self.image_pass.has(id)
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

    /// Resize the surface and recompute grid dimensions. Returns `Some((cols, rows))`
    /// if the size or scale factor actually changed.
    pub fn resize(&mut self, width: u32, height: u32, scale_factor: f64) -> Option<(usize, usize)> {
        let width = width.max(1);
        let height = height.max(1);
        
        let size_changed = self.config.width != width || self.config.height != height;
        let scale_changed = (self.scale_factor - scale_factor).abs() > 1e-5;
        
        if !size_changed && !scale_changed {
            return None;
        }
        
        self.config.width = width;
        self.config.height = height;
        self.scale_factor = scale_factor;
        self.surface.configure(&self.device, &self.config);
        self.viewport
            .update(&self.queue, glyphon::Resolution { width, height });
            
        if scale_changed {
            // Recompute physical font size and line height
            self.font_size = self.logical_font_size * scale_factor as f32;
            self.line_height =
                (self.font_size * (DEFAULT_LINE_HEIGHT / DEFAULT_FONT_SIZE)).round();
            
            // Re-measure cell
            let (cell_width, cell_height) = measure_cell(
                &mut self.font_system,
                self.font_size,
                self.line_height,
                self.font_family.as_deref(),
                self.normal_weight.as_deref(),
            );
            self.cell_width = cell_width;
            self.cell_height = cell_height;
            
            // Clear existing buffers so they are re-allocated with updated physical metrics next frame
            self.text_buffers.clear();
            self.status_buffer = None;
            self.status_right_buffer = None;
        }
        
        let phys_cell_w = self.cell_width;
        let phys_cell_h = self.cell_height;
        self.cols = (width as f32 / phys_cell_w).floor() as usize;
        self.rows = (height as f32 / phys_cell_h).floor() as usize;
        Some((self.cols.max(1), self.rows.max(1)))
    }

    /// Acquire the next swapchain texture, recovering a stale surface in place.
    ///
    /// On `Outdated`/`Lost` (after a resize, GPU reset, or a monitor sleep/wake)
    /// the surface configuration no longer matches its swapchain. We reconfigure
    /// and retry once so this frame paints a valid image, rather than leaving the
    /// freshly reconfigured swapchain showing uninitialized garbage. Transient
    /// states (`Timeout`, `Occluded`) and validation errors skip the frame; the
    /// previously presented frame stays on screen until the next redraw.
    fn acquire_surface_texture(&mut self) -> Option<wgpu::SurfaceTexture> {
        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture)
            | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => Some(texture),
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                match self.surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(texture)
                    | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => Some(texture),
                    _ => None,
                }
            }
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Validation => None,
        }
    }

    /// Render multiple panes to the surface. Each `PaneView` specifies a grid
    /// and its viewport rect. Pane dividers are drawn between adjacent panes.
    /// When `status` is set, a status bar is drawn across the bottom cell row;
    /// callers must leave that row free of panes (see [`Self::cell_size`]).
    /// When `chrome` is set, the tabbar/menubar is drawn across the top cell
    /// row(s) (likewise reserved by the caller) and any open dropdown is
    /// composited over the content via the image pass.
    pub fn render(
        &mut self,
        panes: &[PaneView],
        status: Option<&StatusBar>,
        chrome: Option<&TopChrome>,
        bell_active: bool,
        images: &[ImagePlacement],
        palette: Option<&PaletteView>,
    ) {
        let surface_texture = match self.acquire_surface_texture() {
            Some(texture) => texture,
            None => return,
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
                    cursor_shape: pane.cursor_shape,
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
                    let (qx0, qy0, qx1, qy1) =
                        cursor_quad(pane.cursor_shape, px0, py0, self.cell_width, self.cell_height);
                    all_bg_verts.extend_from_slice(&quad_vertices(
                        qx0,
                        qy0,
                        qx1,
                        qy1,
                        self.theme.cursor_bg.as_linear(),
                        surface_w,
                        surface_h,
                    ));
                }
            }

            let default_attrs = Attrs::new()
                .family(base_family(fam))
                .weight(parse_weight(self.normal_weight.as_deref(), glyphon::cosmic_text::Weight::NORMAL));
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
                            .weight(parse_weight(self.bold_weight.as_deref(), DEFAULT_BOLD_WEIGHT))
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
                            attrs = attrs.weight(parse_weight(self.bold_weight.as_deref(), DEFAULT_BOLD_WEIGHT));
                        } else {
                            attrs = attrs.weight(parse_weight(self.normal_weight.as_deref(), glyphon::cosmic_text::Weight::NORMAL));
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
                    buffer.lines.push(BufferLine::new(
                        &text,
                        ending,
                        attrs_list,
                        Shaping::Advanced,
                    ));
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
        let mut status_right_buffer = self.status_right_buffer.take().unwrap_or_else(|| {
            glyphon::Buffer::new(
                &mut self.font_system,
                glyphon::Metrics::new(self.font_size, self.line_height),
            )
        });
        let status_top = surface_h - self.cell_height;
        if let Some(status) = status {
            all_bg_verts.extend_from_slice(&quad_vertices(
                0.0,
                status_top,
                surface_w,
                status_top + 1.0,
                self.theme.status_bar_border.as_linear(),
                surface_w,
                surface_h,
            ));

            // Status bar background spanning full width; matches the terminal
            // background so the status line blends into the content.
            all_bg_verts.extend_from_slice(&quad_vertices(
                0.0,
                status_top + 1.0,
                surface_w,
                surface_h,
                self.theme.background.as_linear(),
                surface_w,
                surface_h,
            ));

            // Build status bar text segments dynamically
            let mut status_text = String::new();
            let mut spans = Vec::new();

            // Font attributes
            let accent_attrs = Attrs::new()
                .family(base_family(fam))
                .weight(parse_weight(self.bold_weight.as_deref(), DEFAULT_BOLD_WEIGHT))
                .color(status.accent.to_glyphon());
            let foreground_attrs = Attrs::new()
                .family(base_family(fam))
                .weight(parse_weight(self.normal_weight.as_deref(), glyphon::cosmic_text::Weight::NORMAL))
                .color(self.theme.foreground.to_glyphon());
            let muted_attrs = Attrs::new()
                .family(base_family(fam))
                .weight(parse_weight(self.normal_weight.as_deref(), glyphon::cosmic_text::Weight::NORMAL))
                .color(self.theme.ansi[8].to_glyphon());
            let error_attrs = Attrs::new()
                .family(base_family(fam))
                .weight(parse_weight(self.bold_weight.as_deref(), DEFAULT_BOLD_WEIGHT))
                .color(self.theme.ansi[1].to_glyphon());

            // Mode Label (e.g. Normal, Insert, Block)
            let mode_start = status_text.len();
            status_text.push_str(&status.mode);
            let mode_end = status_text.len();
            spans.push((mode_start..mode_end, accent_attrs));

            // A transient error notice claims the pane-title slot in red;
            // otherwise show the pane title / command if present.
            if let Some(ref notice) = status.notice {
                let sep_start = status_text.len();
                status_text.push_str("  •  ");
                let sep_end = status_text.len();
                spans.push((sep_start..sep_end, muted_attrs.clone()));

                let notice_start = status_text.len();
                status_text.push_str(notice);
                let notice_end = status_text.len();
                spans.push((notice_start..notice_end, error_attrs));
            } else if let Some(ref title) = status.pane_title {
                let sep_start = status_text.len();
                status_text.push_str("  •  ");
                let sep_end = status_text.len();
                spans.push((sep_start..sep_end, muted_attrs.clone()));

                let title_start = status_text.len();
                status_text.push_str(title);
                let title_end = status_text.len();
                spans.push((title_start..title_end, foreground_attrs));
            }

            // Right segment: shaped into its own buffer so it can be
            // pixel-positioned flush with the right edge of the surface.
            let right_text_str = status
                .right_label
                .as_deref()
                .unwrap_or("\u{f0697} spaceterm");
            let right_text = right_text_str.to_string();
            let right_default_attrs = Attrs::new()
                .family(base_family(fam))
                .color(self.theme.ansi[8].to_glyphon());
            let right_ending = glyphon::cosmic_text::LineEnding::default();
            if status_right_buffer.lines.is_empty() {
                status_right_buffer.lines.push(BufferLine::new(
                    &right_text,
                    right_ending,
                    glyphon::AttrsList::new(&right_default_attrs),
                    Shaping::Advanced,
                ));
            } else {
                status_right_buffer.lines[0].set_text(
                    &right_text,
                    right_ending,
                    glyphon::AttrsList::new(&right_default_attrs),
                );
            }
            status_right_buffer.lines.truncate(1);
            status_right_buffer.shape_until_scroll(&mut self.font_system, false);

            // Apply attributes to the text buffer line
            let default_attrs = Attrs::new()
                .family(base_family(fam))
                .color(self.theme.ansi[8].to_glyphon());
            let mut attrs_list = glyphon::AttrsList::new(&default_attrs);
            for (range, attrs) in spans {
                attrs_list.add_span(range, &attrs);
            }

            let ending = glyphon::cosmic_text::LineEnding::default();
            if status_buffer.lines.is_empty() {
                status_buffer.lines.push(BufferLine::new(
                    &status_text,
                    ending,
                    attrs_list,
                    Shaping::Advanced,
                ));
            } else {
                status_buffer.lines[0].set_text(&status_text, ending, attrs_list);
            }
            status_buffer.lines.truncate(1);
            status_buffer.shape_until_scroll(&mut self.font_system, false);
        }

        // Top chrome (tabbar/menubar) bands and text. The dropdown overlay is
        // handled separately via the image pass so it sits above pane text.
        let chrome_texts = match chrome {
            Some(c) => self.draw_chrome(c, surface_w),
            None => Vec::new(),
        };

        // Bell: tint is applied in rasterize_chrome_strip, not as a bg overlay.

        let bg_count = all_bg_verts.len() as u32;
        let bg_bytes: Vec<u8> = all_bg_verts.iter().flat_map(|v| v.to_bytes()).collect();
        self.queue.write_buffer(&self.bg_buffer, 0, &bg_bytes);

        // The top-chrome strip (band + rounded-top tabs) is composited before the
        // text pass so the tab cards sit under the tab titles.
        let chrome_strip: Vec<ImagePlacement> = chrome
            .and_then(|c| self.rasterize_chrome_strip(c, surface_w, bell_active))
            .into_iter()
            .collect();
        self.chrome_strip_pass
            .prepare(&self.queue, &chrome_strip, surface_w, surface_h);

        // The open dropdown and command palette are rasterized to textures and
        // drawn by the image pass (after the text pass) so they overlay content.
        let mut all_images: Vec<ImagePlacement> = images.to_vec();
        if let Some(c) = chrome {
            all_images.extend(self.rasterize_dropdown(c, surface_w));
            if let Some(placement) = self.rasterize_bell_tooltip(c, surface_w) {
                all_images.push(placement);
            }
        }
        if let Some(p) = palette {
            all_images.extend(self.rasterize_palette(p, surface_w, surface_h));
        }
        self.image_pass
            .prepare(&self.queue, &all_images, surface_w, surface_h);

        let mut text_areas: Vec<TextArea> = text_buffers
            .iter()
            .zip(panes.iter())
            .map(|(buffer, pane)| TextArea {
                buffer,
                left: pane.rect.x.round(),
                top: pane.rect.y.round(),
                bounds: TextBounds {
                    left: pane.rect.x.round() as i32,
                    top: pane.rect.y.round() as i32,
                    right: (pane.rect.x + pane.rect.width).round() as i32,
                    bottom: (pane.rect.y + pane.rect.height).round() as i32,
                },
                default_color: self.theme.foreground.to_glyphon(),
                scale: 1.0,
                custom_glyphs: &[],
            })
            .collect();

        if status.is_some() {
            let right_w = buffer_width(&status_right_buffer).ceil();
            let right_left = (surface_w - right_w).max(0.0).round();
            text_areas.push(TextArea {
                buffer: &status_buffer,
                left: 0.0,
                top: status_top.round(),
                bounds: TextBounds {
                    left: 0,
                    top: status_top.round() as i32,
                    right: right_left as i32,
                    bottom: surface_h as i32,
                },
                default_color: self.theme.status_bar_fg.to_glyphon(),
                scale: 1.0,
                custom_glyphs: &[],
            });
            text_areas.push(TextArea {
                buffer: &status_right_buffer,
                left: right_left,
                top: status_top.round(),
                bounds: TextBounds {
                    left: right_left as i32,
                    top: status_top.round() as i32,
                    right: surface_w as i32,
                    bottom: surface_h as i32,
                },
                default_color: self.theme.ansi[8].to_glyphon(),
                scale: 1.0,
                custom_glyphs: &[],
            });
        }

        for text in &chrome_texts {
            text_areas.push(TextArea {
                buffer: &text.buffer,
                left: text.left.round(),
                top: text.top.round(),
                bounds: TextBounds {
                    left: text.bounds.left,
                    top: text.bounds.top,
                    right: text.bounds.right,
                    bottom: text.bounds.bottom,
                },
                default_color: text.color,
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
        self.status_right_buffer = Some(status_right_buffer);

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
                        // The sRGB surface expects a linear clear value, so decode
                        // the (sRGB) background to keep the displayed color exact.
                        load: LoadOp::Clear(wgpu::Color {
                            r: srgb_to_linear_f64(self.theme.background.r),
                            g: srgb_to_linear_f64(self.theme.background.g),
                            b: srgb_to_linear_f64(self.theme.background.b),
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

            // Chrome strip sits above the bg quads but below the text so the
            // rounded tab cards back the tab titles.
            self.chrome_strip_pass.render(&mut pass);

            self.text_renderer
                .render(&self.text_atlas, &self.viewport, &mut pass)
                .expect("glyphon render");

            self.image_pass.render(&mut pass);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        surface_texture.present();
    }

    /// Append the top chrome's background quads to `verts` and return its text
    /// runs (tab titles, close glyphs, the new-tab button, and either the modern
    /// hamburger or the classic menu titles). The dropdown is drawn separately.
    fn draw_chrome(&mut self, chrome: &TopChrome, surface_w: f32) -> Vec<ChromeText> {
        let cw = self.cell_width;
        let ch = self.cell_height;
        let layout = chrome_layout(chrome, surface_w, cw, ch);
        let pad = cw * 0.4;
        let muted = self.theme.ansi[8].to_glyphon();
        let foreground = self.theme.foreground.to_glyphon();
        let mut texts = Vec::new();
        // The taller modern bar is two cells high, so center a single text line
        // vertically within each element's region instead of top-aligning it.
        let line_h = self.line_height;
        let vcenter = |y: f32, h: f32| y + (h - line_h) / 2.0;

        // The band, rounded-top tab cards, and chrome border are rasterized into a
        // texture by `rasterize_chrome_strip` and composited under this text.

        // Tab titles and their close glyphs. Close button is on the left edge.
        for (i, tab) in layout.tabs.iter().enumerate() {
            let close = layout.closes[i];
            let title_x = close.x + close.w;
            let avail = ((tab.x + tab.w) - (title_x + pad)).max(cw);
            let max_chars = (avail / cw).floor() as usize;
            let title = truncate_label(&chrome.tabs[i].title, max_chars);
            let active = i == chrome.active_tab;
            let color = if active {
                self.theme.tab_active_fg.to_glyphon()
            } else {
                muted
            };
            let buffer = self.chrome_line_buffer(&title, color, active, false);
            texts.push(ChromeText {
                bounds: text_bounds(title_x, tab.y, tab.x + tab.w, tab.y + tab.h),
                buffer,
                color,
                left: title_x + pad,
                top: vcenter(tab.y, tab.h),
            });

            let close_buf = self.chrome_line_buffer("\u{00d7}", muted, false, false);
            texts.push(ChromeText {
                bounds: text_bounds(close.x, close.y, close.x + close.w, close.y + close.h),
                buffer: close_buf,
                color: muted,
                left: close.x + (close.w - cw) / 2.0,
                top: vcenter(close.y, close.h),
            });
        }

        // New-tab button.
        let new_tab = layout.new_tab;
        let plus = self.chrome_line_buffer("+", muted, false, false);
        texts.push(ChromeText {
            bounds: text_bounds(
                new_tab.x,
                new_tab.y,
                new_tab.x + new_tab.w,
                new_tab.y + new_tab.h,
            ),
            buffer: plus,
            color: muted,
            left: new_tab.x + (new_tab.w - cw) / 2.0,
            top: vcenter(new_tab.y, new_tab.h),
        });

        // Modern hamburger, or classic menu titles.
        if let Some(hb) = layout.hamburger {
            let glyph = self.chrome_line_buffer("\u{2630}", foreground, false, false);
            texts.push(ChromeText {
                bounds: text_bounds(hb.x, hb.y, hb.x + hb.w, hb.y + hb.h),
                buffer: glyph,
                color: foreground,
                left: hb.x + (hb.w - cw) / 2.0,
                top: vcenter(hb.y, hb.h),
            });
        }
        for (i, region) in layout.menu_titles.iter().enumerate() {
            let open = chrome.open_menu == Some(i);
            let buffer = self.chrome_line_buffer(&chrome.menus[i].title, foreground, open, false);
            texts.push(ChromeText {
                bounds: text_bounds(region.x, region.y, region.x + region.w, region.y + region.h),
                buffer,
                color: foreground,
                left: region.x + cw,
                top: vcenter(region.y, region.h),
            });
        }

        // Window controls (minimize / maximize / close) for the borderless title
        // bar. Glyphs: an en-dash, a hollow square, and a multiplication sign.
        // Drawn in the brighter foreground so they read as live window actions.
        if let Some(controls) = layout.controls {
            let glyphs = ["\u{2013}", "\u{25a1}", "\u{2715}"];
            for (region, glyph) in controls.iter().zip(glyphs) {
                let buffer = self.chrome_line_buffer(glyph, foreground, false, false);
                texts.push(ChromeText {
                    bounds: text_bounds(region.x, region.y, region.x + region.w, region.y + region.h),
                    buffer,
                    color: foreground,
                    left: region.x + (region.w - cw) / 2.0,
                    top: vcenter(region.y, region.h),
                });
            }
        }

        texts
    }

    /// Rasterize the command palette overlay and upload it to the GPU. Returns
    /// an empty vec when the palette has no items to display.
    fn rasterize_palette(
        &mut self,
        palette: &PaletteView,
        surface_w: f32,
        surface_h: f32,
    ) -> Vec<ImagePlacement> {
        let ctx = FontCtx {
            cell_h: self.cell_height,
            cell_w: self.cell_width,
            family: self.font_family.as_deref(),
            font_size: self.font_size,
            line_height: self.line_height,
            normal_weight: self.normal_weight.as_deref(),
            bold_weight: self.bold_weight.as_deref(),
        };
        let image = palette_rgba(
            &mut self.font_system,
            &mut self.swash_cache,
            &ctx,
            &self.theme,
            palette,
            surface_w,
            surface_h,
        );
        self.image_pass.upload(
            &self.device,
            &self.queue,
            PALETTE_TEXTURE_ID,
            &image.rgba,
            image.width,
            image.height,
        );
        vec![ImagePlacement {
            height: image.height as f32,
            id: PALETTE_TEXTURE_ID,
            v_max: 1.0,
            width: image.width as f32,
            x: image.x,
            y: image.y,
        }]
    }

    /// Rasterize a small tooltip below the hovered bell dot and upload it to
    /// the GPU. Returns `None` when no bell dot is hovered or no dot is visible.
    fn rasterize_bell_tooltip(
        &mut self,
        chrome: &TopChrome,
        surface_w: f32,
    ) -> Option<ImagePlacement> {
        let tab_idx = chrome.bell_tooltip_tab?;
        let cw = self.cell_width;
        let ch = self.cell_height;
        let layout = chrome_layout(chrome, surface_w, cw, ch);
        let dot = layout.bell_dots.get(tab_idx)?.as_ref()?;
        let ctx = FontCtx {
            cell_h: ch,
            cell_w: cw,
            family: self.font_family.as_deref(),
            font_size: self.font_size,
            line_height: self.line_height,
            normal_weight: self.normal_weight.as_deref(),
            bold_weight: self.bold_weight.as_deref(),
        };
        let image = bell_tooltip_rgba(
            &mut self.font_system,
            &mut self.swash_cache,
            &ctx,
            &self.theme,
            dot,
            surface_w,
        );
        self.image_pass.upload(
            &self.device,
            &self.queue,
            BELL_TOOLTIP_TEXTURE_ID,
            &image.rgba,
            image.width,
            image.height,
        );
        Some(ImagePlacement {
            height: image.height as f32,
            id: BELL_TOOLTIP_TEXTURE_ID,
            v_max: 1.0,
            width: image.width as f32,
            x: image.x,
            y: image.y,
        })
    }

    /// Rasterize the top-chrome strip, the recessed band, the rounded-top tab
    /// cards, and the hairline border, into a texture composited under the chrome
    /// text. The active tab is filled with the terminal background so it merges
    /// seamlessly into the content below; inactive tabs sit recessed and darker.
    fn rasterize_chrome_strip(
        &mut self,
        chrome: &TopChrome,
        surface_w: f32,
        _bell_active: bool,
    ) -> Option<ImagePlacement> {
        let cw = self.cell_width;
        let ch = self.cell_height;
        let layout = chrome_layout(chrome, surface_w, cw, ch);
        let chrome_h = chrome::chrome_rows(chrome.menu_style) as f32 * ch;
        let width = surface_w.ceil().max(1.0) as u32;
        let height = chrome_h.ceil().max(1.0) as u32;
        let canvas = (width, height);
        let band = self.theme.tabbar_bg;
        let active_bg = self.theme.tab_active_bg;
        let inactive_bg = mix_rgb(self.theme.tabbar_bg, self.theme.tab_active_bg, 0.5);

        let mut rgba = vec![0u8; (width * height * 4) as usize];

        // Recessed band across the whole strip.
        fill_rounded_rect(&mut rgba, canvas, (0.0, 0.0, width as f32, height as f32), 0.0, band, 1.0);
        // Hairline border separating the band from the content; the tab cards
        // drawn after cover it under themselves, so it shows only in empty band.
        fill_rounded_rect(
            &mut rgba,
            canvas,
            (0.0, chrome_h - 1.0, width as f32, 1.0),
            0.0,
            self.theme.divider,
            CHROME_BORDER_ALPHA,
        );
        // In classic style, separate the menubar row from the tabbar row.
        if let Some(menubar_top) = layout.menubar_top {
            let _ = menubar_top;
            fill_rounded_rect(
                &mut rgba,
                canvas,
                (0.0, layout.tab_row_top - 1.0, width as f32, 1.0),
                0.0,
                self.theme.divider,
                CHROME_BORDER_ALPHA,
            );
        }

        // Tabs and hamburger share the same pill geometry: inset from the band
        // edges so they read as floating buttons rather than full-bleed cards.
        let tab_hpad = cw * 0.25;
        let tab_vpad = ch * 0.2;
        let pill_radius = ch * TAB_CORNER_RADIUS_RATIO;

        let dot_r = (ch * 0.15).max(2.0);
        let dot_color = self.theme.bell.unwrap_or(self.theme.cursor_bg);
        for (i, tab) in layout.tabs.iter().enumerate() {
            let color = if i == chrome.active_tab { active_bg } else { inactive_bg };
            fill_rounded_rect(
                &mut rgba,
                canvas,
                (tab.x + tab_hpad, tab.y + tab_vpad, tab.w - tab_hpad * 2.0, tab.h - tab_vpad * 2.0),
                pill_radius,
                color,
                1.0,
            );
            if chrome.tabs.get(i).is_some_and(|t| t.bell) {
                // Small circle at the top-right corner of the tab pill.
                let pill_x = tab.x + tab_hpad;
                let pill_right = pill_x + tab.w - tab_hpad * 2.0;
                let pill_y = tab.y + tab_vpad;
                let cx = pill_right - dot_r - 2.0;
                let cy = pill_y + dot_r + 2.0;
                fill_rounded_rect(
                    &mut rgba,
                    canvas,
                    (cx - dot_r, cy - dot_r, dot_r * 2.0, dot_r * 2.0),
                    dot_r,
                    dot_color,
                    1.0,
                );
            }
        }

        // Hamburger pill uses the same vertical inset and radius as tabs.
        if let Some(hb) = layout.hamburger {
            let hpad = cw * 0.3;
            let btn_color = mix_rgb(band, self.theme.foreground, 0.12);
            fill_rounded_rect(
                &mut rgba,
                canvas,
                (hb.x + hpad, hb.y + tab_vpad, hb.w - hpad * 2.0, hb.h - tab_vpad * 2.0),
                pill_radius,
                btn_color,
                1.0,
            );
        }

        self.chrome_strip_pass.upload(
            &self.device,
            &self.queue,
            CHROME_STRIP_TEXTURE_ID,
            &rgba,
            width,
            height,
        );
        Some(ImagePlacement {
            height: height as f32,
            id: CHROME_STRIP_TEXTURE_ID,
            v_max: 1.0,
            width: width as f32,
            x: 0.0,
            y: 0.0,
        })
    }

    /// Rasterize the open dropdown to a texture and return its placement, or
    /// `None` when no menu is open. The pixel work lives in [`dropdown_rgba`] so
    /// it can run (and be tested) without the GPU; this only uploads the result.
    /// Rasterize and place the open menu's overlays: the parent dropdown, and the
    /// open submenu (drawn after, so it overlays the parent). Returns one
    /// placement per visible panel, or empty when no menu is open.
    fn rasterize_dropdown(&mut self, chrome: &TopChrome, surface_w: f32) -> Vec<ImagePlacement> {
        let ctx = FontCtx {
            cell_h: self.cell_height,
            cell_w: self.cell_width,
            family: self.font_family.as_deref(),
            font_size: self.font_size,
            line_height: self.line_height,
            normal_weight: self.normal_weight.as_deref(),
            bold_weight: self.bold_weight.as_deref(),
        };
        // Computed sequentially: each borrows the shared font system in turn.
        let parent = dropdown_rgba(
            &mut self.font_system,
            &mut self.swash_cache,
            &ctx,
            &self.theme,
            chrome,
            surface_w,
        );
        let submenu = submenu_rgba(
            &mut self.font_system,
            &mut self.swash_cache,
            &ctx,
            &self.theme,
            chrome,
            surface_w,
        );
        let mut placements = Vec::new();
        for (id, image) in [(DROPDOWN_TEXTURE_ID, parent), (SUBMENU_TEXTURE_ID, submenu)] {
            let Some(image) = image else { continue };
            self.image_pass.upload(
                &self.device,
                &self.queue,
                id,
                &image.rgba,
                image.width,
                image.height,
            );
            placements.push(ImagePlacement {
                height: image.height as f32,
                id,
                v_max: 1.0,
                width: image.width as f32,
                x: image.x,
                y: image.y,
            });
        }
        placements
    }

    /// Shape a single line of chrome text into a reusable buffer. With
    /// `proportional` set, it uses a sans-serif UI font (for the dropdown menu)
    /// rather than the terminal's monospace family.
    fn chrome_line_buffer(
        &mut self,
        text: &str,
        color: Color,
        bold: bool,
        proportional: bool,
    ) -> glyphon::Buffer {
        let ctx = FontCtx {
            cell_h: self.cell_height,
            cell_w: self.cell_width,
            family: self.font_family.as_deref(),
            font_size: self.font_size,
            line_height: self.line_height,
            normal_weight: self.normal_weight.as_deref(),
            bold_weight: self.bold_weight.as_deref(),
        };
        shape_chrome_line(&mut self.font_system, &ctx, text, color, bold, proportional)
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
    cursor_shape: CursorShape,
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
        cursor_shape,
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
                    cursor_quad(cursor_shape, px0, py0, cw, ch)
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

/// The pixel rect of a cursor of `shape` whose full cell starts at `(x, y)`
/// with size `(cw, ch)`. Block fills the cell; Bar covers a thin strip on the
/// left edge; Underline covers a thin strip on the bottom edge.
fn cursor_quad(shape: CursorShape, x: f32, y: f32, cw: f32, ch: f32) -> (f32, f32, f32, f32) {
    let x1_full = x + cw;
    let y1_full = y + ch;
    match shape {
        CursorShape::Block => (x, y, x1_full, y1_full),
        CursorShape::Bar => {
            let x1 = x + cw * CURSOR_BAR_WIDTH_RATIO;
            (x, y, x1, y1_full)
        }
        CursorShape::Underline => {
            let y0 = y1_full - ch * CURSOR_UNDERLINE_HEIGHT_RATIO;
            (x, y0, x1_full, y1_full)
        }
    }
}

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

/// A glyphon clip rect from pixel edges.
fn text_bounds(left: f32, top: f32, right: f32, bottom: f32) -> TextBounds {
    TextBounds {
        left: left as i32,
        top: top as i32,
        right: right as i32,
        bottom: bottom as i32,
    }
}

/// Shorten `s` to at most `max_chars`, appending an ellipsis when truncated.
fn truncate_label(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        return s.to_string();
    }
    match max_chars {
        0 => String::new(),
        1 => "\u{2026}".to_string(),
        _ => {
            let mut out: String = s.chars().take(max_chars - 1).collect();
            out.push('\u{2026}');
            out
        }
    }
}

/// Fill an opaque `(x, y, w, h)` rectangle of `color` into a `(canvas_w,
/// canvas_h)` `rgba` buffer, clipped to the canvas.
/// Alpha-composite `color` over the pixel at byte `idx` of an RGBA buffer.
fn blend_px(rgba: &mut [u8], idx: usize, color: Rgb, alpha: f32) {
    let a = alpha.clamp(0.0, 1.0);
    if a <= 0.0 {
        return;
    }
    let inv = 1.0 - a;
    rgba[idx] = (color.r as f32 * a + rgba[idx] as f32 * inv) as u8;
    rgba[idx + 1] = (color.g as f32 * a + rgba[idx + 1] as f32 * inv) as u8;
    rgba[idx + 2] = (color.b as f32 * a + rgba[idx + 2] as f32 * inv) as u8;
    let out_a = a + (rgba[idx + 3] as f32 / 255.0) * inv;
    rgba[idx + 3] = (out_a * 255.0) as u8;
}

/// Signed distance from `(px, py)` to the rounded rectangle `(x, y, w, h)` with
/// corner `radius`: negative inside, zero on the edge, positive outside.
fn rounded_rect_sdf(px: f32, py: f32, rect: (f32, f32, f32, f32), radius: f32) -> f32 {
    let (rx, ry, rw, rh) = rect;
    let half_w = rw / 2.0;
    let half_h = rh / 2.0;
    let r = radius.min(half_w).min(half_h);
    let qx = (px - (rx + half_w)).abs() - (half_w - r);
    let qy = (py - (ry + half_h)).abs() - (half_h - r);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

/// Fill a rounded rectangle into `rgba`, anti-aliasing the edge over a 1px band
/// and compositing at up to `max_alpha` opacity.
fn fill_rounded_rect(
    rgba: &mut [u8],
    canvas: (u32, u32),
    rect: (f32, f32, f32, f32),
    radius: f32,
    color: Rgb,
    max_alpha: f32,
) {
    let (canvas_w, canvas_h) = canvas;
    let (rx, ry, rw, rh) = rect;
    let x0 = rx.floor().max(0.0) as u32;
    let y0 = ry.floor().max(0.0) as u32;
    let x1 = ((rx + rw).ceil() as i64).clamp(0, canvas_w as i64) as u32;
    let y1 = ((ry + rh).ceil() as i64).clamp(0, canvas_h as i64) as u32;
    for py in y0..y1 {
        for px in x0..x1 {
            let sdf = rounded_rect_sdf(px as f32 + 0.5, py as f32 + 0.5, rect, radius);
            let coverage = (0.5 - sdf).clamp(0.0, 1.0);
            if coverage <= 0.0 {
                continue;
            }
            let idx = ((py * canvas_w + px) * 4) as usize;
            blend_px(rgba, idx, color, coverage * max_alpha);
        }
    }
}

/// Fill a rectangle whose top corners are rounded but whose bottom edge stays
/// flush. Achieved by rounding a rect extended `radius` below the real bottom, so
/// the bottom corners curve off the drawn region (the canvas clips them away).
#[cfg(test)]
fn fill_rounded_top_rect(
    rgba: &mut [u8],
    canvas: (u32, u32),
    rect: (f32, f32, f32, f32),
    radius: f32,
    color: Rgb,
    max_alpha: f32,
) {
    let (x, y, w, h) = rect;
    fill_rounded_rect(rgba, canvas, (x, y, w, h + radius), radius, color, max_alpha);
}

/// Linearly interpolate two colors channel-wise; `t` clamps to `0.0..=1.0`,
/// where `0.0` returns `a` and `1.0` returns `b`.
fn mix_rgb(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let lerp = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Rgb {
        r: lerp(a.r, b.r),
        g: lerp(a.g, b.g),
        b: lerp(a.b, b.b),
    }
}

/// Decode one 8-bit sRGB channel to a linear `0.0..=1.0` value. Used for the
/// surface clear, which an sRGB target interprets as linear.
fn srgb_to_linear_f64(channel: u8) -> f64 {
    let c = channel as f64 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// The shaped pixel width of a single-line text buffer, used to right-align the
/// dropdown shortcuts under a proportional font.
fn buffer_width(buffer: &glyphon::Buffer) -> f32 {
    buffer
        .layout_runs()
        .map(|run| run.line_w)
        .fold(0.0, f32::max)
}

/// Shape one line of chrome text into a buffer without touching the GPU. With
/// `proportional`, it uses a sans-serif UI font instead of the terminal family.
fn shape_chrome_line(
    font_system: &mut FontSystem,
    ctx: &FontCtx,
    text: &str,
    color: Color,
    bold: bool,
    proportional: bool,
) -> glyphon::Buffer {
    let mut buffer = glyphon::Buffer::new(
        font_system,
        glyphon::Metrics::new(ctx.font_size, ctx.line_height),
    );
    buffer.set_size(font_system, Some(f32::MAX), Some(ctx.line_height));
    let family = if proportional {
        Family::SansSerif
    } else {
        base_family(ctx.family)
    };
    let mut attrs = Attrs::new().family(family).color(color);
    if bold {
        attrs = attrs.weight(parse_weight(ctx.bold_weight, DEFAULT_BOLD_WEIGHT));
    } else {
        attrs = attrs.weight(parse_weight(ctx.normal_weight, glyphon::cosmic_text::Weight::NORMAL));
    }
    buffer.set_text(font_system, text, &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(font_system, false);
    buffer
}

/// Composite `buffer`'s glyph coverage onto a `(canvas_w, canvas_h)` RGBA buffer
/// at pixel `offset`, clipping to the canvas. Bakes dropdown text into its texture.
fn composite_buffer(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    rgba: &mut [u8],
    canvas: (u32, u32),
    buffer: &glyphon::Buffer,
    offset: (i32, i32),
    default_color: Color,
) {
    let (canvas_w, canvas_h) = canvas;
    let (ox, oy) = offset;
    buffer.draw(
        font_system,
        swash_cache,
        default_color,
        |x, y, w, h, color| {
            let alpha = color.a() as f32 / 255.0;
            if alpha <= 0.0 {
                return;
            }
            let (cr, cg, cb) = (color.r() as f32, color.g() as f32, color.b() as f32);
            for py in y..y + h as i32 {
                for px in x..x + w as i32 {
                    let gx = px + ox;
                    let gy = py + oy;
                    if gx < 0 || gy < 0 || gx >= canvas_w as i32 || gy >= canvas_h as i32 {
                        continue;
                    }
                    let idx = ((gy as u32 * canvas_w + gx as u32) * 4) as usize;
                    rgba[idx] = (cr * alpha + rgba[idx] as f32 * (1.0 - alpha)) as u8;
                    rgba[idx + 1] = (cg * alpha + rgba[idx + 1] as f32 * (1.0 - alpha)) as u8;
                    rgba[idx + 2] = (cb * alpha + rgba[idx + 2] as f32 * (1.0 - alpha)) as u8;
                }
            }
        },
    );
}

/// Rasterize a small tooltip bubble near `dot` (a bell-dot hit region in screen
/// coordinates). The tooltip floats below the chrome strip with its center
/// aligned to the dot center, clamped so it never escapes the left or right
/// edges of the surface.
fn bell_tooltip_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    dot: &Region,
    surface_w: f32,
) -> DropdownImage {
    const PAD_X: f32 = 8.0;
    const PAD_Y: f32 = 4.0;
    const RADIUS: f32 = 4.0;
    const SHADOW_MARGIN: f32 = 6.0;

    let label_buf = shape_chrome_line(
        font_system,
        ctx,
        "Bell notification",
        theme.foreground.to_glyphon(),
        false,
        true,
    );
    let text_w = buffer_width(&label_buf).ceil();
    let text_h = ctx.line_height;

    let panel_w = text_w + PAD_X * 2.0;
    let panel_h = text_h + PAD_Y * 2.0;
    let total_w = (panel_w + SHADOW_MARGIN * 2.0) as u32;
    let total_h = (panel_h + SHADOW_MARGIN * 2.0) as u32;
    let canvas = (total_w, total_h);

    // Place the tooltip so its center aligns with the dot center, below the strip.
    let dot_cx = dot.x + dot.w * 0.5;
    let tip_x = (dot_cx - panel_w * 0.5 - SHADOW_MARGIN)
        .max(0.0)
        .min(surface_w - total_w as f32);
    let tip_y = dot.y + dot.h + 2.0 - SHADOW_MARGIN;

    let mut rgba = vec![0u8; (total_w * total_h * 4) as usize];

    // Soft drop-shadow.
    for py in 0..total_h {
        for px in 0..total_w {
            let sdf = rounded_rect_sdf(
                px as f32 + 0.5,
                py as f32 + 0.5,
                (SHADOW_MARGIN, SHADOW_MARGIN, panel_w, panel_h),
                RADIUS,
            );
            let falloff = (1.0 - sdf / SHADOW_MARGIN).clamp(0.0, 1.0);
            if sdf <= 0.0 || falloff <= 0.0 {
                continue;
            }
            let idx = ((py * total_w + px) * 4) as usize;
            blend_px(&mut rgba, idx, SHADOW_COLOR, falloff * falloff * DROPDOWN_SHADOW_ALPHA);
        }
    }

    // Panel background.
    let border = mix_rgb(theme.menu_bg, Rgb::new(255, 255, 255), MENU_BORDER_MIX);
    fill_rounded_rect(
        &mut rgba,
        canvas,
        (SHADOW_MARGIN, SHADOW_MARGIN, panel_w, panel_h),
        RADIUS,
        border,
        1.0,
    );
    fill_rounded_rect(
        &mut rgba,
        canvas,
        (SHADOW_MARGIN + 1.0, SHADOW_MARGIN + 1.0, panel_w - 2.0, panel_h - 2.0),
        RADIUS - 1.0,
        theme.menu_bg,
        1.0,
    );

    // Label text.
    composite_buffer(
        font_system,
        swash_cache,
        &mut rgba,
        canvas,
        &label_buf,
        ((SHADOW_MARGIN + PAD_X) as i32, (SHADOW_MARGIN + PAD_Y) as i32),
        theme.foreground.to_glyphon(),
    );

    DropdownImage {
        height: total_h,
        rgba,
        width: total_w,
        x: tip_x,
        y: tip_y,
    }
}

/// Rasterize the open dropdown overlay (soft shadow, elevated rounded panel,
/// rounded hover pill, and sans-serif item text) to an RGBA image without the
/// GPU. Returns `None` when no menu is open.
/// The parent dropdown panel for the open menu, or `None` when none is open.
fn dropdown_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    chrome: &TopChrome,
    surface_w: f32,
) -> Option<DropdownImage> {
    let layout = chrome_layout(chrome, surface_w, ctx.cell_w, ctx.cell_h);
    let menu = chrome.menus.get(chrome.open_menu?)?;
    let panel = panel_rgba(
        font_system,
        swash_cache,
        ctx,
        theme,
        &menu.items,
        &layout.dropdown?,
        chrome.selected_item,
    );
    Some(panel)
}

/// The open submenu's child panel, or `None` when no submenu is open.
fn submenu_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    chrome: &TopChrome,
    surface_w: f32,
) -> Option<DropdownImage> {
    let layout = chrome_layout(chrome, surface_w, ctx.cell_w, ctx.cell_h);
    let parent = chrome
        .menus
        .get(chrome.open_menu?)?
        .items
        .get(chrome.open_submenu?)?;
    let panel = panel_rgba(
        font_system,
        swash_cache,
        ctx,
        theme,
        &parent.children,
        &layout.submenu?,
        chrome.selected_subitem,
    );
    Some(panel)
}

/// Rasterize one menu panel (`items` at `layout`) into an elevated, rounded,
/// shadowed card. `selected` is the hover-highlighted row. A submenu parent
/// (an item with children) gets a `›` chevron instead of a shortcut.
fn panel_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    items: &[MenuItem],
    layout: &DropdownLayout,
    selected: Option<usize>,
) -> DropdownImage {
    // The panel sits inside a margin that holds its soft drop shadow, so the
    // texture is larger than the panel and is placed offset by the margin. The
    // panel interior still lands exactly at origin_x/top, keeping the app's
    // hit-testing geometry valid.
    let margin = DROPDOWN_SHADOW;
    let panel_w = layout.width.floor().max(1.0);
    let panel_h = (layout.items as f32 * layout.item_h + 2.0 * layout.pad)
        .floor()
        .max(1.0);
    let width = (panel_w + 2.0 * margin) as u32;
    let height = (panel_h + 2.0 * margin) as u32;
    let canvas = (width, height);
    let panel_rect = (margin, margin, panel_w, panel_h);

    let mut rgba = vec![0u8; (width * height * 4) as usize];

    // Soft drop shadow: opacity fades with distance outside the panel.
    for py in 0..height {
        for px in 0..width {
            let sdf = rounded_rect_sdf(
                px as f32 + 0.5,
                py as f32 + 0.5,
                panel_rect,
                DROPDOWN_RADIUS,
            );
            let falloff = (1.0 - sdf / margin).clamp(0.0, 1.0);
            if sdf <= 0.0 || falloff <= 0.0 {
                continue;
            }
            let idx = ((py * width + px) * 4) as usize;
            blend_px(
                &mut rgba,
                idx,
                SHADOW_COLOR,
                falloff * falloff * DROPDOWN_SHADOW_ALPHA,
            );
        }
    }

    // Elevated rounded panel: a subtle hairline border, then the surface inset
    // one pixel within it, then the rounded hover pill for the selected row. The
    // first row begins below the panel's top padding.
    let border = mix_rgb(theme.menu_bg, Rgb::new(255, 255, 255), MENU_BORDER_MIX);
    fill_rounded_rect(&mut rgba, canvas, panel_rect, DROPDOWN_RADIUS, border, 1.0);
    fill_rounded_rect(
        &mut rgba,
        canvas,
        (margin + 1.0, margin + 1.0, panel_w - 2.0, panel_h - 2.0),
        DROPDOWN_RADIUS - 1.0,
        theme.menu_bg,
        1.0,
    );
    let row_top = margin + layout.pad;
    if let Some(sel) = selected {
        let hover_rect = (
            margin + MENU_HOVER_INSET,
            row_top + sel as f32 * layout.item_h + MENU_HOVER_INSET,
            panel_w - 2.0 * MENU_HOVER_INSET,
            layout.item_h - 2.0 * MENU_HOVER_INSET,
        );
        let radius = (DROPDOWN_RADIUS - MENU_HOVER_INSET).max(0.0);
        fill_rounded_rect(
            &mut rgba,
            canvas,
            hover_rect,
            radius,
            theme.menu_hover_bg,
            1.0,
        );
    }

    let foreground = theme.foreground.to_glyphon();
    let muted = theme.ansi[8].to_glyphon();
    let pad = (ctx.cell_w * 0.9) as i32;
    let origin = margin as i32;
    // Center each label vertically within its taller row.
    let text_dy = ((layout.item_h - ctx.line_height) / 2.0).max(0.0) as i32;
    for (i, item) in items.iter().enumerate() {
        let row_y = (row_top + i as f32 * layout.item_h) as i32;
        if item.label == "-" {
            let line_y = row_y + (layout.item_h / 2.0) as i32;
            for x in (origin + pad)..(origin + panel_w as i32 - pad) {
                if (0..width as i32).contains(&x) && (0..height as i32).contains(&line_y) {
                    let idx = ((line_y * width as i32 + x) * 4) as usize;
                    blend_px(&mut rgba, idx, theme.divider, 1.0);
                }
            }
            continue;
        }
        let text_y = row_y + text_dy;
        let label = shape_chrome_line(font_system, ctx, &item.label, foreground, false, true);
        let pos = (origin + pad, text_y);
        composite_buffer(
            font_system,
            swash_cache,
            &mut rgba,
            canvas,
            &label,
            pos,
            foreground,
        );

        // A submenu parent shows a chevron; a leaf shows its shortcut, if any.
        let trailing = if item.has_children() {
            Some("\u{203a}".to_string())
        } else if !item.shortcut.is_empty() {
            Some(item.shortcut.clone())
        } else {
            None
        };
        if let Some(text) = trailing {
            let buf = shape_chrome_line(font_system, ctx, &text, muted, false, true);
            let buf_w = buffer_width(&buf).ceil() as i32;
            let pos = (origin + panel_w as i32 - buf_w - pad, text_y);
            composite_buffer(
                font_system,
                swash_cache,
                &mut rgba,
                canvas,
                &buf,
                pos,
                muted,
            );
        }
    }

    // Snap the overlay to whole pixels. The texture is 1:1 texel-to-pixel, so a
    // fractional placement (origin_x/top derive from fractional cell sizes) would
    // make the shared linear sampler smear every baked glyph, blurring the menu.
    DropdownImage {
        height,
        rgba,
        width,
        x: (layout.origin_x - margin).round(),
        y: (layout.top - margin).round(),
    }
}

/// Render `text` into `rgba` at `offset`, coloring characters listed in
/// `match_positions` with `accent_color` and the rest with `base_color`.
/// When `underline` is true, a 1-pixel line is drawn below each matched span.
fn draw_highlighted_label(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    rgba: &mut [u8],
    canvas: (u32, u32),
    text: &str,
    match_positions: &[usize],
    base_color: Color,
    accent_color: Color,
    underline: bool,
    offset: (i32, i32),
) {
    let chars: Vec<char> = text.chars().collect();
    let (canvas_w, canvas_h) = canvas;
    let underline_y = offset.1 + ctx.line_height as i32 - 2;

    let mut x = offset.0;
    let mut i = 0;
    while i < chars.len() {
        let is_match = match_positions.binary_search(&i).is_ok();
        let color = if is_match { accent_color } else { base_color };
        let mut j = i + 1;
        while j < chars.len() && (match_positions.binary_search(&j).is_ok()) == is_match {
            j += 1;
        }
        let seg: String = chars[i..j].iter().collect();
        let buf = shape_chrome_line(font_system, ctx, &seg, color, false, true);
        let seg_w = buffer_width(&buf).ceil() as i32;
        composite_buffer(font_system, swash_cache, rgba, canvas, &buf, (x, offset.1), color);

        if is_match && underline && underline_y >= 0 && underline_y < canvas_h as i32 {
            let uy = underline_y as u32;
            for px in x.max(0)..(x + seg_w).min(canvas_w as i32) {
                let idx = (uy * canvas_w + px as u32) as usize * 4;
                rgba[idx] = accent_color.r();
                rgba[idx + 1] = accent_color.g();
                rgba[idx + 2] = accent_color.b();
                rgba[idx + 3] = 255;
            }
        }

        x += seg_w;
        i = j;
    }
}

/// Rasterize the command palette: a centered floating panel with a search input
/// at the top and a fuzzy-filtered list of commands below. Styled like the
/// dropdown but wider and vertically centered in the upper portion of the window.
fn palette_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    view: &PaletteView,
    surface_w: f32,
    surface_h: f32,
) -> DropdownImage {
    let margin = DROPDOWN_SHADOW;
    let inner_pad = ctx.cell_h * 0.5;
    let input_h = ctx.cell_h * 1.8;
    let item_h = ctx.cell_h * 1.7;
    let display_count = view.items.len().min(PALETTE_MAX_ITEMS);
    let row_count = display_count.max(1);
    let panel_w = (surface_w * PALETTE_WIDTH_RATIO).clamp(300.0, 680.0).floor();
    let panel_h = (inner_pad + input_h + 1.0 + item_h * row_count as f32 + inner_pad)
        .floor()
        .max(1.0);

    let width = (panel_w + 2.0 * margin) as u32;
    let height = (panel_h + 2.0 * margin) as u32;
    let canvas = (width, height);
    let panel_rect = (margin, margin, panel_w, panel_h);

    let mut rgba = vec![0u8; (width * height * 4) as usize];

    // Soft drop shadow.
    for py in 0..height {
        for px in 0..width {
            let sdf = rounded_rect_sdf(
                px as f32 + 0.5,
                py as f32 + 0.5,
                panel_rect,
                DROPDOWN_RADIUS,
            );
            let falloff = (1.0 - sdf / margin).clamp(0.0, 1.0);
            if sdf <= 0.0 || falloff <= 0.0 {
                continue;
            }
            let idx = ((py * width + px) * 4) as usize;
            blend_px(
                &mut rgba,
                idx,
                SHADOW_COLOR,
                falloff * falloff * DROPDOWN_SHADOW_ALPHA,
            );
        }
    }

    // Elevated rounded panel with hairline border.
    let border = mix_rgb(theme.menu_bg, Rgb::new(255, 255, 255), MENU_BORDER_MIX);
    fill_rounded_rect(&mut rgba, canvas, panel_rect, DROPDOWN_RADIUS, border, 1.0);
    fill_rounded_rect(
        &mut rgba,
        canvas,
        (margin + 1.0, margin + 1.0, panel_w - 2.0, panel_h - 2.0),
        DROPDOWN_RADIUS - 1.0,
        theme.menu_bg,
        1.0,
    );

    // Hover highlight on the selected result row.
    let divider_y = margin + inner_pad + input_h;
    let results_top = divider_y + 1.0;
    if display_count > 0 {
        let sel = view.selected.min(display_count - 1);
        let hover_rect = (
            margin + MENU_HOVER_INSET,
            results_top + sel as f32 * item_h + MENU_HOVER_INSET,
            panel_w - 2.0 * MENU_HOVER_INSET,
            item_h - 2.0 * MENU_HOVER_INSET,
        );
        let radius = (DROPDOWN_RADIUS - MENU_HOVER_INSET).max(0.0);
        fill_rounded_rect(&mut rgba, canvas, hover_rect, radius, theme.menu_hover_bg, 1.0);
    }

    // Hairline divider between the input and the results.
    fill_rounded_rect(
        &mut rgba,
        canvas,
        (margin, divider_y, panel_w, 1.0),
        0.0,
        theme.divider,
        0.5,
    );

    let foreground = theme.foreground.to_glyphon();
    let muted = theme.ansi[8].to_glyphon();
    let accent = theme.cursor_bg.to_glyphon();
    let pad_x = (ctx.cell_w * 1.0) as i32;
    let origin = margin as i32;
    let input_text_dy = ((input_h - ctx.line_height) / 2.0).max(0.0) as i32;
    let item_text_dy = ((item_h - ctx.line_height) / 2.0).max(0.0) as i32;
    let input_top = (margin + inner_pad) as i32;

    // "❯" prompt.
    let prompt_buf = shape_chrome_line(font_system, ctx, "\u{276f} ", accent, false, true);
    let prompt_w = buffer_width(&prompt_buf).ceil() as i32;
    composite_buffer(
        font_system,
        swash_cache,
        &mut rgba,
        canvas,
        &prompt_buf,
        (origin + pad_x, input_top + input_text_dy),
        accent,
    );

    // Query text or placeholder.
    let query_text = if view.query.is_empty() {
        "type to filter\u{2026}".to_string()
    } else {
        view.query.clone()
    };
    let query_color = if view.query.is_empty() { muted } else { foreground };
    let query_buf =
        shape_chrome_line(font_system, ctx, &query_text, query_color, false, true);
    let query_w = buffer_width(&query_buf).ceil() as i32;
    composite_buffer(
        font_system,
        swash_cache,
        &mut rgba,
        canvas,
        &query_buf,
        (origin + pad_x + prompt_w, input_top + input_text_dy),
        query_color,
    );

    // Blinking-cursor indicator after the query text.
    let cursor_buf =
        shape_chrome_line(font_system, ctx, "\u{2502}", accent, false, false);
    composite_buffer(
        font_system,
        swash_cache,
        &mut rgba,
        canvas,
        &cursor_buf,
        (origin + pad_x + prompt_w + query_w, input_top + input_text_dy),
        accent,
    );

    // Result rows.
    if display_count == 0 {
        let no_match =
            shape_chrome_line(font_system, ctx, &view.empty_message, muted, false, true);
        composite_buffer(
            font_system,
            swash_cache,
            &mut rgba,
            canvas,
            &no_match,
            (origin + pad_x, results_top as i32 + item_text_dy),
            muted,
        );
    } else {
        for (i, item) in view.items.iter().take(display_count).enumerate() {
            let row_y = (results_top + i as f32 * item_h) as i32 + item_text_dy;

            if item.match_positions.is_empty() || view.query.is_empty() {
                let label_buf =
                    shape_chrome_line(font_system, ctx, &item.label, foreground, false, true);
                composite_buffer(
                    font_system,
                    swash_cache,
                    &mut rgba,
                    canvas,
                    &label_buf,
                    (origin + pad_x, row_y),
                    foreground,
                );
            } else {
                draw_highlighted_label(
                    font_system,
                    swash_cache,
                    ctx,
                    &mut rgba,
                    canvas,
                    &item.label,
                    &item.match_positions,
                    foreground,
                    accent,
                    view.match_underline,
                    (origin + pad_x, row_y),
                );
            }

            let action_buf =
                shape_chrome_line(font_system, ctx, &item.action, muted, false, true);
            let action_w = buffer_width(&action_buf).ceil() as i32;
            composite_buffer(
                font_system,
                swash_cache,
                &mut rgba,
                canvas,
                &action_buf,
                (origin + panel_w as i32 - action_w - pad_x, row_y),
                muted,
            );
        }
    }

    // Center horizontally; place in the upper third of the window.
    let palette_x = ((surface_w - panel_w) / 2.0).max(0.0);
    let palette_y = surface_h * PALETTE_TOP_RATIO;

    DropdownImage {
        height,
        rgba,
        width,
        x: (palette_x - margin).round(),
        y: (palette_y - margin).round(),
    }
}

/// Register the embedded [`DEFAULT_FONT_FAMILY`] faces in the font database so
/// the default font is available even when it is not installed on the system.
/// Loading is idempotent in effect: if the user already has the same family
/// installed, both copies coexist and the weight match picks one identical face.
fn load_bundled_fonts(font_system: &mut FontSystem) {
    let db = font_system.db_mut();
    db.load_font_data(BUNDLED_FONT_REGULAR.to_vec());
    db.load_font_data(BUNDLED_FONT_SEMIBOLD.to_vec());
}

/// The cosmic-text font family for a configured family name, defaulting to the
/// system monospace when no family is set.
fn base_family(name: Option<&str>) -> Family<'_> {
    match name {
        Some(n) => Family::Name(n),
        None => Family::Monospace,
    }
}

fn parse_weight(weight: Option<&str>, default: glyphon::cosmic_text::Weight) -> glyphon::cosmic_text::Weight {
    match weight {
        None => default,
        Some(w) => match w.to_lowercase().as_str() {
            "thin" | "100" => glyphon::cosmic_text::Weight::THIN,
            "extra-light" | "extralight" | "200" => glyphon::cosmic_text::Weight::EXTRA_LIGHT,
            "light" | "300" => glyphon::cosmic_text::Weight::LIGHT,
            "normal" | "regular" | "400" => glyphon::cosmic_text::Weight::NORMAL,
            "medium" | "500" => glyphon::cosmic_text::Weight::MEDIUM,
            "semibold" | "semi-bold" | "600" => glyphon::cosmic_text::Weight::SEMIBOLD,
            "bold" | "700" => glyphon::cosmic_text::Weight::BOLD,
            "extra-bold" | "extrabold" | "800" => glyphon::cosmic_text::Weight::EXTRA_BOLD,
            "black" | "heavy" | "900" => glyphon::cosmic_text::Weight::BLACK,
            parsed => {
                if let Ok(num) = parsed.parse::<u16>() {
                    glyphon::cosmic_text::Weight(num)
                } else {
                    default
                }
            }
        }
    }
}

fn measure_cell(
    font_system: &mut FontSystem,
    font_size: f32,
    line_height: f32,
    family: Option<&str>,
    normal_weight: Option<&str>,
) -> (f32, f32) {
    let metrics = glyphon::Metrics::new(font_size, line_height);
    let mut buffer = glyphon::Buffer::new(font_system, metrics);
    buffer.set_size(font_system, Some(f32::MAX), Some(line_height));
    let attrs = Attrs::new()
        .family(base_family(family))
        .weight(parse_weight(normal_weight, glyphon::cosmic_text::Weight::NORMAL));
    buffer.set_text(font_system, "M", &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(font_system, false);

    if let Some(run) = buffer.layout_runs().next() {
        let glyph_w = run.glyphs.first().map(|g| g.w).unwrap_or(font_size * 0.6);
        return (glyph_w.round(), line_height.round());
    }

    ((font_size * 0.6).round(), line_height.round())
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

    fn sample_menu_chrome(selected: Option<usize>) -> crate::chrome::TopChrome {
        use crate::chrome::{ControlsSide, Menu, MenuItem, MenuStyle, TabLabel, TopChrome};
        let leaf = |label: &str, shortcut: &str| MenuItem {
            children: Vec::new(),
            label: label.into(),
            shortcut: shortcut.into(),
        };
        TopChrome {
            active_tab: 0,
            controls_side: ControlsSide::Right,
            menu_style: MenuStyle::Modern,
            menus: vec![Menu {
                title: "Menu".into(),
                items: vec![
                    leaf("New Tab", "Ctrl-Shift-T"),
                    leaf("Split Vertical", ""),
                    MenuItem {
                        children: vec![leaf("Dark", ""), leaf("Light", "")],
                        label: "Theme".into(),
                        shortcut: String::new(),
                    },
                ],
            }],
            bell_tooltip_tab: None,
            open_menu: Some(0),
            open_submenu: None,
            selected_item: selected,
            selected_subitem: None,
            tabs: vec![TabLabel {
                bell: false,
                title: "Terminal 1".into(),
            }],
            window_controls: false,
        }
    }

    fn sample_font_ctx() -> FontCtx<'static> {
        FontCtx {
            cell_h: 18.0,
            cell_w: 9.0,
            family: None,
            font_size: 14.0,
            line_height: 18.0,
            normal_weight: None,
            bold_weight: None,
        }
    }

    #[test]
    fn test_dropdown_rgba_is_an_elevated_rounded_card() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let chrome = sample_menu_chrome(None);

        let image = dropdown_rgba(&mut font_system, &mut swash, &ctx, &theme, &chrome, 1000.0)
            .expect("menu is open");
        let pixel = |x: u32, y: u32| {
            let i = ((y * image.width + x) * 4) as usize;
            (
                image.rgba[i],
                image.rgba[i + 1],
                image.rgba[i + 2],
                image.rgba[i + 3],
            )
        };
        let margin = DROPDOWN_SHADOW as u32;

        // The outer corner is fully transparent: the panel does not fill the
        // shadow margin (this is a floating card, not a full-bleed rectangle).
        assert_eq!(pixel(0, 0).3, 0);
        // The panel's own top-left corner is rounded away (not fully opaque)...
        assert!(pixel(margin, margin).3 < 255);
        // ...while the top padding band is the opaque elevated surface color,
        // proving both the lighter menu_bg and the vertical padding are applied.
        let (r, g, b, a) = pixel(image.width / 2, margin + 2);
        assert_eq!(
            (r, g, b, a),
            (theme.menu_bg.r, theme.menu_bg.g, theme.menu_bg.b, 255)
        );
    }

    #[test]
    fn test_rounded_top_rect_rounds_only_the_top() {
        let (w, h) = (40u32, 40u32);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        let white = Rgb {
            r: 255,
            g: 255,
            b: 255,
        };
        fill_rounded_top_rect(&mut rgba, (w, h), (0.0, 0.0, 40.0, 40.0), 12.0, white, 1.0);
        let alpha = |x: u32, y: u32| rgba[((y * w + x) * 4 + 3) as usize];
        // The top corners are rounded away (transparent), but the bottom edge is
        // flush (opaque) so the tab connects seamlessly to the content below. A
        // regression to all-corner rounding would make the bottom corner sheer off.
        assert!(alpha(0, 0) < 40, "top-left rounded, got {}", alpha(0, 0));
        assert!(alpha(39, 0) < 40, "top-right rounded, got {}", alpha(39, 0));
        assert!(alpha(0, 39) > 220, "bottom-left flush, got {}", alpha(0, 39));
        assert!(alpha(39, 39) > 220, "bottom-right flush, got {}", alpha(39, 39));
    }

    #[test]
    fn test_dropdown_overlay_is_pixel_snapped() {
        // Real displays have fractional cell metrics; without snapping, the
        // overlay lands on sub-pixel boundaries and the linear sampler blurs it.
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = FontCtx {
            cell_h: 19.4,
            cell_w: 9.6,
            family: None,
            font_size: 14.0,
            line_height: 19.4,
            normal_weight: None,
            bold_weight: None,
        };
        let chrome = sample_menu_chrome(None);
        let image = dropdown_rgba(&mut font_system, &mut swash, &ctx, &theme, &chrome, 1003.0)
            .expect("menu is open");
        assert_eq!(image.x.fract(), 0.0, "overlay x must be whole pixels");
        assert_eq!(image.y.fract(), 0.0, "overlay y must be whole pixels");
    }

    #[test]
    fn test_rounded_rect_sdf_signs() {
        let rect = (0.0, 0.0, 100.0, 100.0);
        let radius = 20.0;
        // The center is well inside (negative distance).
        assert!(rounded_rect_sdf(50.0, 50.0, rect, radius) < 0.0);
        // A point far outside is positive.
        assert!(rounded_rect_sdf(150.0, 50.0, rect, radius) > 0.0);
        // The very corner of the bounding box lies outside the rounded shape...
        assert!(rounded_rect_sdf(1.0, 1.0, rect, radius) > 0.0);
        // ...while the middle of an edge, the same depth in, is inside.
        assert!(rounded_rect_sdf(1.0, 50.0, rect, radius) < 0.0);
    }

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
                cursor_shape: CursorShape::Block,
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
                cursor_shape: CursorShape::Block,
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
                cursor_shape: CursorShape::Block,
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
                cursor_shape: CursorShape::Block,
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
                cursor_shape: CursorShape::Block,
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
                cursor_shape: CursorShape::Block,
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
