//! The native application: winit event loop, GPU renderer, and PTY panes wired
//! together. This is the `SpaceTerm` binary's core runtime — keyboard input drives
//! the PTY, PTY output drives the cell grid, and the grid is rendered to the
//! GPU surface every frame.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId};

use crate::config::Config;
use crate::session::Session;
use crate::input::{self, Action, KeyCode, PendingPrefix};
use crate::layout::{Direction, FocusDir, PaneId, Rect, Tab};
use crate::mode::Mode;
use crate::palette::Palette;
use crate::pane::Pane;
use crate::webview::WebViewManager;
use spaceterm_render::gpu::{GpuRenderer, PaneRect, PaneView};
use spaceterm_render::{StatusBar, Theme};

// ========================================================================
// Constants
// ========================================================================

const PTY_POLL_INTERVAL: Duration = Duration::from_millis(16);
const SPLIT_RATIO: f32 = 0.5;
/// Cell rows reserved at the bottom of the surface for the status bar.
const STATUS_BAR_ROWS: usize = 1;
const BELL_FLASH_DURATION: Duration = Duration::from_millis(150);
/// A bell arriving within this window after a cursor-navigation or edit key is
/// treated as the shell's boundary ding (e.g. Backspace with nothing to delete,
/// or Left at the start of the prompt), and its flash is suppressed.
const EDIT_KEY_BELL_WINDOW: Duration = Duration::from_millis(150);

/// Whether `bytes` are a cursor-navigation or line-edit key that the shell dings
/// when it can't act on it (at the prompt boundary): Backspace, Delete, and the
/// arrow/Home/End cursor keys.
fn is_boundary_ding_key(bytes: &[u8]) -> bool {
    matches!(
        bytes,
        [0x7f]                          // Backspace (DEL)
            | [0x1b, b'[', b'A'..=b'D'] // Up / Down / Right / Left
            | [0x1b, b'[', b'H']        // Home
            | [0x1b, b'[', b'F']        // End
            | [0x1b, b'O', b'P'] // Delete (SS3 P)
    )
}
const WORD_CHARS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-.~/";
const DEFAULT_COLS: u32 = 80;
const DEFAULT_ROWS: u32 = 24;
const APPROX_CELL_WIDTH: u32 = 9;
const APPROX_CELL_HEIGHT: u32 = 20;

const QUICK_SELECT_LABELS: &[char] = &[
    'a', 's', 'd', 'f', 'g', 'h', 'j', 'k', 'l', 'q', 'w', 'e', 'r', 't', 'y', 'u', 'i', 'o',
    'p', 'z', 'x', 'c', 'v', 'b', 'n', 'm',
];

// ========================================================================
// Data Structures
// ========================================================================

/// The application state, driven by winit's `ApplicationHandler` trait.
pub struct App {
    bell_until: Option<Instant>,
    config: Config,
    cursor_pos: (f32, f32),
    dirty: bool,
    folded_blocks: HashMap<PaneId, HashSet<usize>>,
    last_click: Option<(Instant, f32, f32)>,
    last_edit_key: Option<Instant>,
    last_tile_layout: Option<(usize, usize, u32, u32)>,
    modifiers: winit::event::Modifiers,
    mouse_down: bool,
    /// The Normal-mode traversal cursor for the focused pane, in viewport
    /// `(row, col)`. `Some` only while that pane is in Normal mode.
    nav_cursor: Option<(usize, usize)>,
    panes: HashMap<PaneId, Pane>,
    palette: Option<Palette>,
    pane_titles: HashMap<PaneId, String>,
    pending: PendingPrefix,
    quick_select: Option<Vec<QuickLabel>>,
    renderer: Option<GpuRenderer>,
    search_query: Option<String>,
    selection: Option<Selection>,
    tab: Tab,
    modes: HashMap<PaneId, Mode>,
    webview_mgr: WebViewManager,
    window: Option<Arc<Window>>,
    window_title: String,
}

#[derive(Clone, Copy, Debug)]
struct QuickLabel {
    col: usize,
    label: char,
    row: usize,
}

#[derive(Clone, Debug)]
struct Selection {
    end_col: usize,
    end_row: usize,
    pane: PaneId,
    start_col: usize,
    start_row: usize,
}

// ========================================================================
// App
// ========================================================================

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        let mut modes = HashMap::new();
        modes.insert(PaneId(0), Mode::default());

        Self {
            bell_until: None,
            config: Config::load(),
            cursor_pos: (0.0, 0.0),
            dirty: true,
            folded_blocks: HashMap::new(),
            last_click: None,
            last_edit_key: None,
            last_tile_layout: None,
            modifiers: winit::event::Modifiers::default(),
            mouse_down: false,
            nav_cursor: None,
            panes: HashMap::new(),
            palette: None,
            pane_titles: HashMap::new(),
            pending: PendingPrefix::None,
            quick_select: None,
            renderer: None,
            search_query: None,
            selection: None,
            tab: Tab::new(),
            modes,
            webview_mgr: WebViewManager::new(),
            window: None,
            window_title: String::new(),
        }
    }

    fn viewport_rect(&self) -> PaneRect {
        let (w, h) = match (&self.window, &self.renderer) {
            (Some(win), _) => {
                let size = win.inner_size();
                (size.width as f32, size.height as f32)
            }
            (None, Some(r)) => {
                let (cols, rows) = r.grid_size();
                let (cw, ch) = r.cell_size();
                (cols as f32 * cw, rows as f32 * ch)
            }
            (None, None) => (800.0, 600.0),
        };
        // Exclude the status bar row so pane hit-testing and focus geometry match
        // the area actually drawn to panes.
        let status_h = self
            .renderer
            .as_ref()
            .map(|r| r.cell_size().1 * STATUS_BAR_ROWS as f32)
            .unwrap_or(0.0);
        PaneRect {
            x: 0.0,
            y: 0.0,
            width: w,
            height: (h - status_h).max(1.0),
        }
    }

    fn layout_rect_to_pane(rect: Rect) -> PaneRect {
        PaneRect {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        }
    }

    fn pixel_to_cell(&self, x: f32, y: f32, pane_rect: PaneRect) -> (usize, usize) {
        let (cw, ch) = self
            .renderer
            .as_ref()
            .map(|r| r.cell_size())
            .unwrap_or((9.0, 20.0));
        let col = ((x - pane_rect.x) / cw).floor() as usize;
        let row = ((y - pane_rect.y) / ch).floor() as usize;
        (row, col)
    }

    fn pane_at_pixel(&self, x: f32, y: f32) -> Option<(PaneId, PaneRect)> {
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

    fn selected_text(&self) -> Option<String> {
        let sel = self.selection.as_ref()?;
        let pane = self.panes.get(&sel.pane)?;
        let grid = pane.grid();

        let (sr, sc, er, ec) = if (sel.start_row, sel.start_col) <= (sel.end_row, sel.end_col) {
            (sel.start_row, sel.start_col, sel.end_row, sel.end_col)
        } else {
            (sel.end_row, sel.end_col, sel.start_row, sel.start_col)
        };

        let mut text = String::new();
        for row in sr..=er {
            let col_start = if row == sr { sc } else { 0 };
            let col_end = if row == er { ec + 1 } else { grid.cols() };
            for col in col_start..col_end.min(grid.cols()) {
                let ch = grid.cell(row, col).map(|c| c.ch).unwrap_or(' ');
                text.push(ch);
            }
            if row < er {
                text.push('\n');
            }
        }
        let trimmed = text.trim_end().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    }

    fn copy_selection(&self) {
        if let Some(text) = self.selected_text() {
            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                let _ = clipboard.set_text(&text);
            }
        }
    }

    fn paste_from_clipboard(&mut self) {
        let text = match arboard::Clipboard::new() {
            Ok(mut cb) => cb.get_text().ok(),
            Err(_) => None,
        };
        let Some(text) = text else { return };
        let focused = self.tab.focused();
        let Some(pane) = self.panes.get_mut(&focused) else { return };
        if pane.bracketed_paste() {
            let mut bytes = Vec::with_capacity(text.len() + 8);
            bytes.extend_from_slice(b"\x1b[200~");
            bytes.extend_from_slice(text.as_bytes());
            bytes.extend_from_slice(b"\x1b[201~");
            pane.write(&bytes);
        } else {
            pane.write(text.as_bytes());
        }
    }

    fn select_word_at(&mut self, pane_id: PaneId, row: usize, col: usize) {
        let Some(pane) = self.panes.get(&pane_id) else {
            return;
        };
        let grid = pane.grid();
        let ch = grid.cell(row, col).map(|c| c.ch).unwrap_or(' ');
        if !WORD_CHARS.contains(ch) {
            self.selection = Some(Selection {
                start_row: row,
                start_col: col,
                end_row: row,
                end_col: col,
                pane: pane_id,
            });
            return;
        }
        let mut start = col;
        let mut end = col;
        while start > 0 {
            if let Some(c) = grid.cell(row, start - 1).map(|c| c.ch) {
                if WORD_CHARS.contains(c) {
                    start -= 1;
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        while end < grid.cols() - 1 {
            if let Some(c) = grid.cell(row, end + 1).map(|c| c.ch) {
                if WORD_CHARS.contains(c) {
                    end += 1;
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        self.selection = Some(Selection {
            start_row: row,
            start_col: start,
            end_row: row,
            end_col: end,
            pane: pane_id,
        });
    }

    fn update_window_title(&mut self) {
        let Some(window) = self.window.clone() else { return };

        let title = if let Some(palette) = &self.palette {
            let selected = palette.selected_action().unwrap_or("");
            format!("SpaceTerm - palette: > {} [{}]", palette.query, selected)
        } else if self.search_query.is_some() || self.pending == PendingPrefix::SearchInput {
            let query = self.search_query.as_deref().unwrap_or("");
            format!("SpaceTerm - search: /{query}")
        } else if self.quick_select.is_some() {
            "SpaceTerm - quick select".to_string()
        } else {
            match self.pane_titles.get(&self.tab.focused()) {
                Some(t) if !t.is_empty() => format!("SpaceTerm - {t}"),
                _ => "SpaceTerm".to_string(),
            }
        };

        // The window manager call is an IPC round-trip; only make it when the
        // title actually changes, not on every keystroke or PTY poll.
        if title != self.window_title {
            window.set_title(&title);
            self.window_title = title;
        }
    }

    fn flash_bell(&mut self) {
        self.bell_until = Some(Instant::now() + BELL_FLASH_DURATION);
        self.dirty = true;
    }

    fn is_bell_active(&self) -> bool {
        self.bell_until.is_some_and(|t| Instant::now() < t)
    }
}

// ========================================================================
// ApplicationHandler
// ========================================================================

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        // Kick off the system-font scan first so it runs concurrently with
        // window creation and the whole wgpu instance/adapter/device setup.
        let font_load = spaceterm_render::start_font_load();

        #[cfg(target_os = "linux")]
        {
            let _ = gtk::init();
        }

        let window = Arc::new(
            event_loop
                .create_window(
                    WindowAttributes::default()
                        .with_title("SpaceTerm")
                        .with_transparent(true)
                        .with_inner_size(winit::dpi::LogicalSize::new(
                            DEFAULT_COLS * APPROX_CELL_WIDTH,
                            DEFAULT_ROWS * APPROX_CELL_HEIGHT,
                        )),
                )
                .expect("create window"),
        );

        let size = window.inner_size();

        // GPU validation layers (enabled by `InstanceFlags::default()` in debug
        // builds) cost ~100ms at startup and slow every draw call. Off by
        // default; opt in with SPACETERM_GPU_DEBUG=1 when debugging the renderer.
        let gpu_flags = if std::env::var_os("SPACETERM_GPU_DEBUG").is_some() {
            wgpu::InstanceFlags::debugging()
        } else {
            wgpu::InstanceFlags::empty()
        };
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            flags: gpu_flags,
            backend_options: wgpu::BackendOptions::default(),
            display: None,
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
        });

        let surface = instance
            .create_surface(Arc::clone(&window))
            .expect("create wgpu surface");

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .expect("request wgpu adapter");

        let font = spaceterm_render::FontConfig {
            family: std::env::var("SPACETERM_FONT")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .or_else(|| self.config.font_family.clone()),
            size: std::env::var("SPACETERM_FONT_SIZE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(self.config.font_size),
        };
        let mut renderer =
            GpuRenderer::new(surface, adapter, size.width, size.height, font, font_load);

        let mut initial_theme = match self.config.theme {
            crate::config::ThemeSetting::Auto => window
                .theme()
                .map(|t| match t {
                    winit::window::Theme::Dark => Theme::dark(),
                    winit::window::Theme::Light => Theme::light(),
                })
                .unwrap_or_default(),
            crate::config::ThemeSetting::Dark => Theme::dark(),
            crate::config::ThemeSetting::Light => Theme::light(),
        };
        self.config.colors.apply(&mut initial_theme);
        renderer.set_theme(initial_theme);

        let side_channel_dir = std::env::temp_dir().join(format!("spaceterm-sidechannel-{}", std::process::id()));
        std::fs::create_dir_all(&side_channel_dir).ok();
        std::env::set_var("SPACETERM_SIDECHANNEL_DIR", &side_channel_dir);

        let (cols, rows) = renderer.grid_size();
        let pane = Pane::new(cols, content_rows(rows));
        let focused = self.tab.focused();
        self.panes.insert(focused, pane);

        self.window = Some(window);
        self.renderer = Some(renderer);
        self.dirty = true;
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::Resized(size) => {
                if size.width > 0 && size.height > 0 {
                    if let Some(renderer) = &mut self.renderer {
                        renderer.resize(size.width, size.height);
                        let grid_size = renderer.grid_size();
                        self.resize_all_panes(grid_size);
                    }
                    self.dirty = true;
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
            }

            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods;
            }

            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }

                let focused = self.tab.focused();
                let mode = self.modes.get(&focused).copied().unwrap_or_default();

                let mods_state = self.modifiers.state();
                let key = input::Key {
                    alt: mods_state.alt_key(),
                    code: winit_key_to_code(&event.logical_key),
                    ctrl: mods_state.control_key(),
                    shift: mods_state.shift_key(),
                };

                if mods_state.control_key() && mods_state.shift_key() {
                    if let Key::Character(c) = event.logical_key.as_ref() {
                        let lower = c.to_lowercase();
                        if lower == "v" {
                            self.paste_from_clipboard();
                            if let Some(window) = &self.window {
                                window.request_redraw();
                            }
                            return;
                        } else if lower == "c" {
                            self.copy_selection();
                            return;
                        } else if lower == "p" {
                            if self.palette.is_some() {
                                self.palette = None;
                            } else {
                                self.palette = Some(Palette::open());
                            }
                            self.dirty = true;
                            if let Some(window) = &self.window {
                                window.request_redraw();
                            }
                            return;
                        }
                    }
                }

                if self.palette.is_some() {
                    let key = event.logical_key.clone();
                    let mut palette = self.palette.take().unwrap();
                    self.handle_palette_input(&mut palette, &key, focused);
                    if palette.active {
                        self.palette = Some(palette);
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }

                let fullscreen = self
                    .panes
                    .get(&focused)
                    .is_some_and(|p| p.grid().is_alt_screen());
                let action = input::resolve(mode, &key, &mut self.pending, fullscreen);
                self.handle_action(action, focused);
                self.update_window_title();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }

            WindowEvent::MouseInput { state, button, .. } => {
                let focused = self.tab.focused();
                let mouse_active = self
                    .panes
                    .get(&focused)
                    .is_some_and(|p| p.mouse_tracking());

                if mouse_active {
                    self.forward_mouse_event(state, button, focused);
                    return;
                }

                match (state, button) {
                (ElementState::Pressed, MouseButton::Left) => {
                    self.mouse_down = true;
                    self.selection = None;

                    let (x, y) = self.cursor_pos;
                    if let Some((pane_id, pane_rect)) = self.pane_at_pixel(x, y) {
                        self.tab.focus(pane_id);
                        let (row, col) = self.pixel_to_cell(x, y, pane_rect);
                        let now = Instant::now();
                        if let Some((prev_time, prev_x, prev_y)) = self.last_click {
                            let dist = ((x - prev_x).powi(2) + (y - prev_y).powi(2)).sqrt();
                            if now.duration_since(prev_time) < Duration::from_millis(400)
                                && dist < 5.0
                            {
                                self.select_word_at(pane_id, row, col);
                            }
                        }
                        self.last_click = Some((now, x, y));
                    }

                    self.dirty = true;
                }
                (ElementState::Released, MouseButton::Left) => {
                    self.mouse_down = false;
                    self.copy_selection();
                }
                (ElementState::Pressed, MouseButton::Middle) => {
                    self.paste_from_clipboard();
                }
                _ => {}
            }
            }

            WindowEvent::CursorMoved { position, .. } => {
                let x = position.x as f32;
                let y = position.y as f32;
                self.cursor_pos = (x, y);

                let focused = self.tab.focused();
                if self.mouse_down
                    && self
                        .panes
                        .get(&focused)
                        .is_some_and(|p| p.mouse_drag_tracking())
                {
                    self.forward_mouse_motion(focused);
                    return;
                }

                if self.mouse_down {
                    if let Some((pane_id, pane_rect)) = self.pane_at_pixel(x, y) {
                        let (row, col) = self.pixel_to_cell(x, y, pane_rect);
                        if let Some(sel) = &mut self.selection {
                            sel.end_row = row;
                            sel.end_col = col;
                            sel.pane = pane_id;
                        } else {
                            self.selection = Some(Selection {
                                start_row: row,
                                start_col: col,
                                end_row: row,
                                end_col: col,
                                pane: pane_id,
                            });
                        }
                        self.dirty = true;
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                }
            }

            WindowEvent::MouseWheel { delta, .. } => {
                let scroll_lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y as isize,
                    MouseScrollDelta::PixelDelta(pos) => (-pos.y / 20.0) as isize,
                };

                let focused = self.tab.focused();
                if self
                    .panes
                    .get(&focused)
                    .is_some_and(|p| p.mouse_tracking())
                {
                    self.forward_mouse_scroll(scroll_lines, focused);
                    return;
                }

                if scroll_lines != 0 {
                    if let Some(pane) = self.panes.get_mut(&focused) {
                        let grid = pane.grid_mut();
                        if scroll_lines > 0 {
                            grid.scroll_up_history(scroll_lines as usize);
                        } else {
                            grid.scroll_down_history((-scroll_lines) as usize);
                        }
                    }
                    self.dirty = true;
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
            }

            WindowEvent::RedrawRequested => {
                self.render_frame();
            }

            WindowEvent::CloseRequested => {
                Session::save(&self.tab, &self.panes);
                self.panes.clear();
                event_loop.exit();
            }

            WindowEvent::Focused(focused) => {
                let focused_pane = self.tab.focused();
                if let Some(pane) = self.panes.get(&focused_pane) {
                    if pane.focus_event() {
                        let seq = if focused { "\x1b[I" } else { "\x1b[O" };
                        if let Some(pane) = self.panes.get_mut(&focused_pane) {
                            pane.write(seq.as_bytes());
                        }
                    }
                }
            }

            WindowEvent::ThemeChanged(theme) => {
                if let Some(renderer) = &mut self.renderer {
                    if self.config.theme == crate::config::ThemeSetting::Auto {
                        let mut colors = match theme {
                            winit::window::Theme::Dark => Theme::dark(),
                            winit::window::Theme::Light => Theme::light(),
                        };
                        self.config.colors.apply(&mut colors);
                        renderer.set_theme(colors);
                        self.dirty = true;
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                }
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        #[cfg(target_os = "linux")]
        while gtk::events_pending() {
            gtk::main_iteration_do(false);
        }

        self.reap_dead_panes();
        let any_output = self.drain_all_panes();
        if any_output {
            self.dirty = true;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            self.update_window_title();
        }

        if self.is_bell_active() {
            self.dirty = true;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        } else if self.bell_until.is_some() {
            self.bell_until = None;
            self.dirty = true;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }

        event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + PTY_POLL_INTERVAL));
    }
}

// ========================================================================
// Action handling
// ========================================================================

impl App {
    fn handle_action(&mut self, action: Action, focused: PaneId) {
        match action {
            Action::SendBytes(bytes) => {
                if is_boundary_ding_key(&bytes) {
                    self.last_edit_key = Some(Instant::now());
                }
                if let Some(pane) = self.panes.get_mut(&focused) {
                    pane.write(&bytes);
                }
            }
            Action::SwitchMode(new_mode) => {
                let old_mode = self.modes.get(&focused).copied().unwrap_or_default();
                self.modes.insert(focused, new_mode);
                if new_mode == Mode::Normal && old_mode != Mode::Normal {
                    self.init_nav_cursor(focused);
                } else if new_mode != Mode::Normal {
                    self.nav_cursor = None;
                }
            }
            Action::MoveCursor(mv) => {
                self.move_nav_cursor(mv, focused);
            }
            Action::SplitPane(direction) => {
                self.split_pane(direction);
            }
            Action::ClosePane => {
                self.close_pane(focused);
            }
            Action::FocusPane(dir) => {
                let viewport = self.viewport_rect();
                let layout_vp = Rect::new(viewport.x, viewport.y, viewport.width, viewport.height);
                self.tab.focus_in_direction(dir, layout_vp);
            }
            Action::FocusBlock(nav) => {
                self.focus_block(nav, focused);
            }
            Action::ForwardToBlock(bytes) => {
                self.webview_mgr.forward_key_event(focused, &bytes);
            }
            Action::SearchStart => {
                self.search_query = Some(String::new());
                self.dirty = true;
            }
            Action::SearchChar(c) => {
                if let Some(q) = &mut self.search_query {
                    q.push(c);
                }
                self.dirty = true;
            }
            Action::SearchBackspace => {
                if let Some(q) = &mut self.search_query {
                    q.pop();
                }
                self.dirty = true;
            }
            Action::SearchExecute => {
                self.search_in_pane(focused, input::BlockNav::Next);
                self.dirty = true;
            }
            Action::SearchCancel => {
                self.search_query = None;
                self.dirty = true;
            }
            Action::SearchNext => {
                self.search_in_pane(focused, input::BlockNav::Next);
            }
            Action::SearchPrevious => {
                self.search_in_pane(focused, input::BlockNav::Previous);
            }
            Action::YankBlock => {
                self.yank_block_source(focused);
            }
            Action::ToggleFold => {
                self.toggle_fold(focused);
            }
            Action::QuickSelect => {
                self.enter_quick_select(focused);
            }
            Action::QuickJump(c) => {
                self.quick_jump(focused, c);
            }
            Action::QuickCancel => {
                self.quick_select = None;
            }
            Action::Ignore => {}
        }
    }

    fn split_pane(&mut self, direction: Direction) {
        let new_id = self.tab.split(direction, SPLIT_RATIO);

        let (cols, rows) = if let Some(renderer) = &self.renderer {
            renderer.grid_size()
        } else {
            (80, 24)
        };

        let (pane_cols, pane_rows) = match direction {
            Direction::Vertical => (cols / 2, rows),
            Direction::Horizontal => (cols, rows / 2),
        };
        let pane = Pane::new(pane_cols.max(1), pane_rows.max(1));
        self.panes.insert(new_id, pane);
        self.modes.insert(new_id, Mode::default());

        if let Some(renderer) = &self.renderer {
            let grid_size = renderer.grid_size();
            self.resize_all_panes(grid_size);
        }
        self.dirty = true;
    }

    fn close_pane(&mut self, pane_id: PaneId) {
        if self.tab.panes().len() <= 1 {
            return;
        }
        self.panes.remove(&pane_id);
        self.modes.remove(&pane_id);
        self.pane_titles.remove(&pane_id);
        self.webview_mgr.remove_tiles_for_pane(pane_id);
        self.last_tile_layout = None;
        self.tab.close(pane_id);
        if let Some(renderer) = &self.renderer {
            let grid_size = renderer.grid_size();
            self.resize_all_panes(grid_size);
        }
        self.dirty = true;
    }

    fn reap_dead_panes(&mut self) {
        let dead: Vec<PaneId> = self
            .panes
            .iter_mut()
            .filter_map(|(id, pane)| if pane.is_alive() { None } else { Some(*id) })
            .collect();
        for id in dead {
            if self.tab.panes().len() > 1 {
                self.close_pane(id);
            }
        }
    }

    fn search_in_pane(&mut self, focused: PaneId, direction: input::BlockNav) {
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

    fn yank_block_source(&self, focused: PaneId) {
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

    fn toggle_fold(&mut self, focused: PaneId) {
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

    fn is_block_folded(&self, pane_id: PaneId, block_index: usize) -> bool {
        self.folded_blocks
            .get(&pane_id)
            .is_some_and(|set| set.contains(&block_index))
    }

    fn enter_quick_select(&mut self, focused: PaneId) {
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

    fn quick_jump(&mut self, focused: PaneId, label: char) {
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

    fn focus_block(&mut self, nav: input::BlockNav, focused: PaneId) {
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

    fn forward_mouse_event(
        &mut self,
        state: ElementState,
        button: MouseButton,
        focused: PaneId,
    ) {
        let (x, y) = self.cursor_pos;
        let Some((_, pane_rect)) = self.pane_at_pixel(x, y) else {
            return;
        };
        let (row, col) = self.pixel_to_cell(x, y, pane_rect);
        let sgr = self.panes.get(&focused).is_some_and(|p| p.mouse_sgr());

        let btn_code = match button {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            MouseButton::Forward => 4,
            MouseButton::Back => 5,
            _ => return,
        };

        let pressed = state == ElementState::Pressed;

        let bytes = if sgr {
            let cb = if pressed { btn_code } else { btn_code + 3 };
            let final_char = if pressed { 'M' } else { 'm' };
            format!("\x1b[<{};{};{}{}", cb, col + 1, row + 1, final_char).into_bytes()
        } else {
            let cb = (32 + if pressed { btn_code } else { btn_code + 3 }) as u8;
            let cv = 32u8.saturating_add((col.min(222) + 1) as u8);
            let ch = 32u8.saturating_add((row.min(222) + 1) as u8);
            format!("\x1b[M{}{}{}", cb as char, cv as char, ch as char).into_bytes()
        };

        if let Some(pane) = self.panes.get_mut(&focused) {
            pane.write(&bytes);
        }
    }

    fn forward_mouse_motion(&mut self, focused: PaneId) {
        let (x, y) = self.cursor_pos;
        let Some((_, pane_rect)) = self.pane_at_pixel(x, y) else {
            return;
        };
        let (row, col) = self.pixel_to_cell(x, y, pane_rect);
        let sgr = self.panes.get(&focused).is_some_and(|p| p.mouse_sgr());

        let btn_code = 0;
        let cb_code = 32 + btn_code;

        let bytes = if sgr {
            format!(
                "\x1b[<{};{};{}M",
                cb_code,
                col + 1,
                row + 1
            )
            .into_bytes()
        } else {
            let cb = (32 + cb_code) as u8;
            let cv = 32u8.saturating_add((col.min(222) + 1) as u8);
            let ch = 32u8.saturating_add((row.min(222) + 1) as u8);
            format!("\x1b[M{}{}{}", cb as char, cv as char, ch as char).into_bytes()
        };

        if let Some(pane) = self.panes.get_mut(&focused) {
            pane.write(&bytes);
        }
    }

    fn forward_mouse_scroll(&mut self, scroll_lines: isize, focused: PaneId) {
        let (x, y) = self.cursor_pos;
        let Some((_, pane_rect)) = self.pane_at_pixel(x, y) else {
            return;
        };
        let (row, col) = self.pixel_to_cell(x, y, pane_rect);
        let sgr = self.panes.get(&focused).is_some_and(|p| p.mouse_sgr());

        let count = scroll_lines.abs().min(10) as u8;
        let sign: u8 = if scroll_lines > 0 { 0 } else { 1 };

        for _ in 0..count {
            let cb = 64 + sign;
            let bytes = if sgr {
                format!("\x1b[<{};{};{}M", cb, col + 1, row + 1).into_bytes()
            } else {
                let b = 32 + cb;
                let cv = 32u8.saturating_add((col.min(222) + 1) as u8);
                let ch = 32u8.saturating_add((row.min(222) + 1) as u8);
                format!("\x1b[M{}{}{}", b as char, cv as char, ch as char).into_bytes()
            };
            if let Some(pane) = self.panes.get_mut(&focused) {
                pane.write(&bytes);
            }
        }
    }
}
// ========================================================================

impl App {
    fn drain_all_panes(&mut self) -> bool {
        let mut any = false;
        let mut any_bell = false;
        let mut new_entries: Vec<(PaneId, crate::block_queue::BlockEntry)> = Vec::new();
        let mut patched_tiles: Vec<(PaneId, usize)> = Vec::new();
        let mut new_titles: Vec<(PaneId, String)> = Vec::new();
        for (id, pane) in self.panes.iter_mut() {
            let prev_count = pane.block_queue().entries().len();
            if pane.drain_output() {
                any = true;
            }
            if pane.take_bell() {
                any_bell = true;
            }
            if let Some(title) = pane.take_title() {
                new_titles.push((*id, title));
            }
            let curr_entries = pane.block_queue().entries();
            if curr_entries.len() > prev_count {
                for entry in &curr_entries[prev_count..] {
                    new_entries.push((*id, entry.clone()));
                }
            }
            let patched = pane.drain_live_patches();
            for idx in patched {
                patched_tiles.push((*id, idx));
            }
        }
        if any_bell {
            // Suppress the visual flash for the shell's prompt-boundary ding
            // (a navigation/edit key the shell can't act on); keep it for
            // genuine bells.
            let from_edit_key = self
                .last_edit_key
                .is_some_and(|t| t.elapsed() < EDIT_KEY_BELL_WINDOW);
            if from_edit_key {
                self.last_edit_key = None;
            } else {
                self.flash_bell();
            }
        }
        if !new_titles.is_empty() {
            for (id, title) in new_titles {
                self.pane_titles.insert(id, title);
            }
            self.update_window_title();
        }
        if !new_entries.is_empty() {
            self.create_block_tiles(&new_entries);
        }
        if !patched_tiles.is_empty() {
            self.update_live_tiles(&patched_tiles);
        }
        any
    }

    fn resize_all_panes(&mut self, (full_cols, full_rows): (usize, usize)) {
        let (cw, ch) = if let Some(renderer) = &self.renderer {
            renderer.cell_size()
        } else {
            (9.0, 20.0)
        };

        let layout_vp = Rect::new(0.0, 0.0, full_cols as f32 * cw, content_rows(full_rows) as f32 * ch);
        let rects = self.tab.rects(layout_vp);

        for (id, rect) in rects {
            let cols = (rect.width / cw).floor() as usize;
            let rows = (rect.height / ch).floor() as usize;
            if let Some(pane) = self.panes.get_mut(&id) {
                pane.resize(cols.max(1), rows.max(1));
            }
        }
    }

    /// Place the traversal cursor at the focused pane's terminal cursor, where
    /// the prompt sits, when entering Normal mode. The shell cursor rests one
    /// cell past the last typed character; clamp onto it so Normal mode never
    /// starts beyond the typed text (Vim steps left off the end on `Esc`).
    fn init_nav_cursor(&mut self, focused: PaneId) {
        if let Some(pane) = self.panes.get(&focused) {
            let grid = pane.grid();
            let (cursor_row, cursor_col) = grid.cursor();
            let row = cursor_row.min(grid.rows().saturating_sub(1));
            let col = cursor_col.min(grid.visible_line_end(row));
            self.nav_cursor = Some((row, col));
        }
    }

    /// Move the traversal cursor within the focused pane. Moves past a viewport
    /// edge scroll the grid's history instead, so the cursor reaches the whole
    /// buffer; page and jump moves scroll directly.
    fn move_nav_cursor(&mut self, mv: input::CursorMove, focused: PaneId) {
        use input::CursorMove;

        let Some(pane) = self.panes.get_mut(&focused) else {
            return;
        };
        let grid = pane.grid_mut();
        let rows = grid.rows();
        let cols = grid.cols();
        let (mut row, mut col) = self.nav_cursor.unwrap_or_else(|| grid.cursor());
        row = row.min(rows.saturating_sub(1));
        col = col.min(cols.saturating_sub(1));

        match mv {
            CursorMove::Left => col = col.saturating_sub(1),
            CursorMove::Right => col += 1,
            CursorMove::Up => {
                if row > 0 {
                    row -= 1;
                } else {
                    grid.scroll_up_history(1);
                }
            }
            CursorMove::Down => {
                if row + 1 < rows {
                    row += 1;
                } else {
                    grid.scroll_down_history(1);
                }
            }
            CursorMove::LineStart => col = 0,
            CursorMove::LineEnd => col = cols,
            CursorMove::FirstNonBlank => col = first_non_blank(&line_chars(grid, row)),
            CursorMove::WordForward => (row, col) = motion_word_forward(grid, rows, row, col, false),
            CursorMove::WordForwardBig => (row, col) = motion_word_forward(grid, rows, row, col, true),
            CursorMove::WordBack => (row, col) = motion_word_back(grid, row, col, false),
            CursorMove::WordBackBig => (row, col) = motion_word_back(grid, row, col, true),
            CursorMove::WordEnd => (row, col) = motion_word_end(grid, rows, row, col, false),
            CursorMove::WordEndBig => (row, col) = motion_word_end(grid, rows, row, col, true),
            CursorMove::Top => {
                grid.set_scroll_offset(grid.scrollback_len());
                row = 0;
                col = 0;
            }
            CursorMove::Bottom => {
                grid.set_scroll_offset(0);
                row = rows.saturating_sub(1);
            }
            CursorMove::PageUp => grid.scroll_up_history(rows),
            CursorMove::PageDown => grid.scroll_down_history(rows),
            CursorMove::HalfPageUp => grid.scroll_up_history(rows / 2),
            CursorMove::HalfPageDown => grid.scroll_down_history(rows / 2),
        }

        // Respect each line's real end: never sit on the blank padding past the
        // last printed character (snapping to a shorter line on vertical moves).
        col = col.min(grid.visible_line_end(row));
        self.nav_cursor = Some((row, col));
        self.dirty = true;
    }

    fn handle_palette_input(&mut self, palette: &mut Palette, key: &Key, focused: PaneId) {
        match key {
            Key::Named(NamedKey::Escape) => {
                palette.close();
                self.palette = None;
            }
            Key::Named(NamedKey::Enter) => {
                if let Some(action) = palette.selected_action() {
                    self.execute_palette_action(action, focused);
                }
                self.palette = None;
            }
            Key::Named(NamedKey::Backspace) => {
                palette.pop_char();
            }
            Key::Named(NamedKey::ArrowUp) => {
                palette.move_up();
            }
            Key::Named(NamedKey::ArrowDown) => {
                palette.move_down();
            }
            Key::Character(c) => {
                for ch in c.chars() {
                    palette.push_char(ch);
                }
            }
            _ => {}
        }
    }

    fn execute_palette_action(&mut self, action: &str, focused: PaneId) {
        match action {
            "toggle_mode" => {
                let mode = self.modes.get(&focused).copied().unwrap_or_default();
                let new_mode = match mode {
                    Mode::Insert => Mode::Normal,
                    Mode::Normal | Mode::BlockFocus => Mode::Insert,
                };
                self.modes.insert(focused, new_mode);
            }
            "split_horizontal" => {
                self.split_pane(Direction::Horizontal);
            }
            "split_vertical" => {
                self.split_pane(Direction::Vertical);
            }
            "close_pane" => {
                self.close_pane(focused);
            }
            "focus_down" | "focus_up" | "focus_left" | "focus_right" => {
                let dir = match action {
                    "focus_down" => FocusDir::Down,
                    "focus_up" => FocusDir::Up,
                    "focus_left" => FocusDir::Left,
                    _ => FocusDir::Right,
                };
                let viewport = self.viewport_rect();
                let layout_vp =
                    Rect::new(viewport.x, viewport.y, viewport.width, viewport.height);
                self.tab.focus_in_direction(dir, layout_vp);
            }
            "search" => {
                self.search_query = Some(String::new());
            }
            "next_block" => {
                self.focus_block(input::BlockNav::Next, focused);
            }
            "prev_block" => {
                self.focus_block(input::BlockNav::Previous, focused);
            }
            "quick_select" => {
                self.enter_quick_select(focused);
            }
            "yank_block" => {
                self.yank_block_source(focused);
            }
            "toggle_fold" => {
                let folded = self
                    .folded_blocks
                    .entry(focused)
                    .or_default();
                if folded.is_empty() {
                    folded.insert(0);
                } else {
                    folded.clear();
                }
                self.dirty = true;
            }
            "theme_dark" => {
                self.config.theme = crate::config::ThemeSetting::Dark;
                if let Some(renderer) = &mut self.renderer {
                    let mut theme = Theme::dark();
                    self.config.colors.apply(&mut theme);
                    renderer.set_theme(theme);
                }
                self.dirty = true;
            }
            "theme_light" => {
                self.config.theme = crate::config::ThemeSetting::Light;
                if let Some(renderer) = &mut self.renderer {
                    let mut theme = Theme::light();
                    self.config.colors.apply(&mut theme);
                    renderer.set_theme(theme);
                }
                self.dirty = true;
            }
            "theme_auto" => {
                self.config.theme = crate::config::ThemeSetting::Auto;
                if let (Some(renderer), Some(window)) = (&mut self.renderer, &self.window) {
                    let mut colors = window
                        .theme()
                        .map(|t| match t {
                            winit::window::Theme::Dark => Theme::dark(),
                            winit::window::Theme::Light => Theme::light(),
                        })
                        .unwrap_or_default();
                    self.config.colors.apply(&mut colors);
                    renderer.set_theme(colors);
                }
                self.dirty = true;
            }
            _ => {}
        }
    }

    fn render_frame(&mut self) {
        let bell_active = self.is_bell_active();
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        let (full_cols, full_rows) = renderer.grid_size();
        let (cw, ch) = renderer.cell_size();

        let content_rows = content_rows(full_rows);
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

    fn create_block_tiles(&mut self, entries: &[(PaneId, crate::block_queue::BlockEntry)]) {
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

            let html = crate::webview::render_block_html(&entry.emit);
            let block_h = crate::webview::WebViewManager::block_pixel_height(ch);
            let params = crate::webview::TileParams {
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

    fn update_live_tiles(&mut self, patched: &[(PaneId, usize)]) {
        for (pane_id, entry_idx) in patched {
            let entry = match self.panes.get(pane_id) {
                Some(p) => p.block_queue().entries().get(*entry_idx).cloned(),
                None => None,
            };
            if let Some(entry) = entry {
                let html = crate::webview::render_block_html(&entry.emit);
                if let Err(e) = self.webview_mgr.update_tile_html(*pane_id, &entry, &html) {
                    eprintln!("spaceterm: live-block update error: {e}");
                }
            }
        }
    }
}

// ========================================================================
// Status bar
// ========================================================================

/// Rows available to panes after reserving the status bar row, never below one.
fn content_rows(full_rows: usize) -> usize {
    full_rows.saturating_sub(STATUS_BAR_ROWS).max(1)
}

/// The status bar for the focused pane's mode: a padded label and a mode-coded
/// accent color drawn from the theme's ANSI palette.
fn status_bar(mode: Mode, theme: &Theme) -> StatusBar {
    let (label, accent) = match mode {
        Mode::Insert => ("INSERT", theme.ansi[2]),
        Mode::Normal => ("NORMAL", theme.ansi[4]),
        Mode::BlockFocus => ("BLOCK", theme.ansi[5]),
    };
    StatusBar {
        accent,
        label: format!(" {label} "),
    }
}

// ========================================================================
// Vim word motions
// ========================================================================

/// A character's word class, à la Vim. Blanks (class 0) separate words. With
/// `big` false (`w`/`b`/`e`): keyword runs (alphanumerics and `_`, class 1) are
/// distinct from punctuation runs (class 2). With `big` true (`W`/`B`/`E`): any
/// non-blank is class 1, so only whitespace breaks a WORD.
fn char_class(c: char, big: bool) -> u8 {
    if c == '\0' || c.is_whitespace() {
        0
    } else if big || c.is_alphanumeric() || c == '_' {
        1
    } else {
        2
    }
}

/// The start column of the next word at or after `col` (Vim `w`/`W`), or `None`
/// when the rest of the line holds no further word.
fn next_word_start(line: &[char], col: usize, big: bool) -> Option<usize> {
    let mut i = col;
    let here = line.get(i).map(|c| char_class(*c, big)).unwrap_or(0);
    if here != 0 {
        while i < line.len() && char_class(line[i], big) == here {
            i += 1;
        }
    }
    while i < line.len() && char_class(line[i], big) == 0 {
        i += 1;
    }
    (i < line.len()).then_some(i)
}

/// The start column of the previous word before `col` (Vim `b`/`B`), or `None`
/// when nothing precedes it on the line.
fn prev_word_start(line: &[char], col: usize, big: bool) -> Option<usize> {
    if col == 0 {
        return None;
    }
    let mut i = col - 1;
    while i > 0 && char_class(line[i], big) == 0 {
        i -= 1;
    }
    if char_class(line[i], big) == 0 {
        return None;
    }
    let class = char_class(line[i], big);
    while i > 0 && char_class(line[i - 1], big) == class {
        i -= 1;
    }
    Some(i)
}

/// The end column of the next word after `col` (Vim `e`/`E`), or `None` when the
/// rest of the line holds no further word.
fn word_end(line: &[char], col: usize, big: bool) -> Option<usize> {
    let mut i = col + 1;
    while i < line.len() && char_class(line[i], big) == 0 {
        i += 1;
    }
    if i >= line.len() {
        return None;
    }
    let class = char_class(line[i], big);
    while i + 1 < line.len() && char_class(line[i + 1], big) == class {
        i += 1;
    }
    Some(i)
}

/// The column of the first non-blank character (Vim `^`), or 0 for a blank line.
fn first_non_blank(line: &[char]) -> usize {
    line.iter().position(|c| char_class(*c, false) != 0).unwrap_or(0)
}

/// `w`/`W`: the next word start, wrapping to the next line (scrolling at the
/// bottom edge) when the current line has no further word.
fn motion_word_forward(
    grid: &mut spaceterm_render::Grid,
    rows: usize,
    row: usize,
    col: usize,
    big: bool,
) -> (usize, usize) {
    match next_word_start(&line_chars(grid, row), col, big) {
        Some(c) => (row, c),
        None => {
            let row = next_row(grid, rows, row);
            (row, first_non_blank(&line_chars(grid, row)))
        }
    }
}

/// `b`/`B`: the previous word start, wrapping to the prior line (scrolling at the
/// top edge) when nothing precedes the cursor on the current line.
fn motion_word_back(grid: &mut spaceterm_render::Grid, row: usize, col: usize, big: bool) -> (usize, usize) {
    match prev_word_start(&line_chars(grid, row), col, big) {
        Some(c) => (row, c),
        None => {
            let row = prev_row(grid, row);
            let prev = line_chars(grid, row);
            (row, prev_word_start(&prev, prev.len(), big).unwrap_or(0))
        }
    }
}

/// `e`/`E`: the next word end, wrapping to the next line (scrolling at the bottom
/// edge) when the current line has no further word.
fn motion_word_end(
    grid: &mut spaceterm_render::Grid,
    rows: usize,
    row: usize,
    col: usize,
    big: bool,
) -> (usize, usize) {
    match word_end(&line_chars(grid, row), col, big) {
        Some(c) => (row, c),
        None => {
            let row = next_row(grid, rows, row);
            (row, word_end(&line_chars(grid, row), 0, big).unwrap_or(0))
        }
    }
}

/// Step one visible row down, scrolling history at the bottom edge.
fn next_row(grid: &mut spaceterm_render::Grid, rows: usize, row: usize) -> usize {
    if row + 1 < rows {
        row + 1
    } else {
        grid.scroll_down_history(1);
        row
    }
}

/// Step one visible row up, scrolling history at the top edge.
fn prev_row(grid: &mut spaceterm_render::Grid, row: usize) -> usize {
    if row > 0 {
        row - 1
    } else {
        grid.scroll_up_history(1);
        row
    }
}

/// The printed characters of a visible row, trimmed of trailing blank padding so
/// motions see real line ends. A fully blank row yields an empty slice.
fn line_chars(grid: &spaceterm_render::Grid, row: usize) -> Vec<char> {
    let end = grid.visible_line_end(row);
    let mut chars: Vec<char> = (0..=end)
        .map(|col| grid.visible_cell(row, col).map(|c| c.ch).unwrap_or(' '))
        .map(|c| if c == '\0' { ' ' } else { c })
        .collect();
    if chars.len() == 1 && char_class(chars[0], false) == 0 {
        chars.clear();
    }
    chars
}

// ========================================================================
// Input translation
// ========================================================================

fn winit_key_to_code(key: &winit::keyboard::Key) -> KeyCode {
    match key.as_ref() {
        Key::Character(c) => KeyCode::Char(c.chars().next().unwrap_or('\0')),
        Key::Named(NamedKey::Enter) => KeyCode::Enter,
        Key::Named(NamedKey::Backspace) => KeyCode::Backspace,
        Key::Named(NamedKey::Tab) => KeyCode::Tab,
        Key::Named(NamedKey::Escape) => KeyCode::Escape,
        Key::Named(NamedKey::ArrowUp) => KeyCode::Up,
        Key::Named(NamedKey::ArrowDown) => KeyCode::Down,
        Key::Named(NamedKey::ArrowLeft) => KeyCode::Left,
        Key::Named(NamedKey::ArrowRight) => KeyCode::Right,
        Key::Named(NamedKey::Space) => KeyCode::Space,
        Key::Named(NamedKey::Home) => KeyCode::Home,
        Key::Named(NamedKey::End) => KeyCode::End,
        Key::Named(NamedKey::PageUp) => KeyCode::PageUp,
        Key::Named(NamedKey::PageDown) => KeyCode::PageDown,
        Key::Named(NamedKey::Insert) => KeyCode::Insert,
        Key::Named(NamedKey::Delete) => KeyCode::Delete,
        Key::Named(NamedKey::F1) => KeyCode::F(1),
        Key::Named(NamedKey::F2) => KeyCode::F(2),
        Key::Named(NamedKey::F3) => KeyCode::F(3),
        Key::Named(NamedKey::F4) => KeyCode::F(4),
        Key::Named(NamedKey::F5) => KeyCode::F(5),
        Key::Named(NamedKey::F6) => KeyCode::F(6),
        Key::Named(NamedKey::F7) => KeyCode::F(7),
        Key::Named(NamedKey::F8) => KeyCode::F(8),
        Key::Named(NamedKey::F9) => KeyCode::F(9),
        Key::Named(NamedKey::F10) => KeyCode::F(10),
        Key::Named(NamedKey::F11) => KeyCode::F(11),
        Key::Named(NamedKey::F12) => KeyCode::F(12),
        _ => KeyCode::Char('\0'),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_default_has_one_pane() {
        let app = App::new();
        assert_eq!(app.tab.panes(), vec![PaneId(0)]);
        assert!(app.panes.is_empty());
    }

    #[test]
    fn test_content_rows_reserves_status_bar_and_floors_at_one() {
        assert_eq!(content_rows(24), 24 - STATUS_BAR_ROWS);
        // Never underflows or collapses to zero on a tiny surface.
        assert_eq!(content_rows(1), 1);
        assert_eq!(content_rows(0), 1);
    }

    #[test]
    fn test_vim_word_motions_on_a_line() {
        // f o o , _ b a r _ b a z _ q u x   (_ = space)
        let line: Vec<char> = "foo, bar_baz qux".chars().collect();

        // `w`: word starts, treating punctuation as its own word.
        assert_eq!(next_word_start(&line, 0, false), Some(3)); // foo -> ','
        assert_eq!(next_word_start(&line, 3, false), Some(5)); // ',' -> 'bar_baz'
        assert_eq!(next_word_start(&line, 5, false), Some(13)); // 'bar_baz' -> 'qux'
        assert_eq!(next_word_start(&line, 13, false), None); // nothing after 'qux'

        // `b`: previous word starts.
        assert_eq!(prev_word_start(&line, 13, false), Some(5));
        assert_eq!(prev_word_start(&line, 5, false), Some(3));
        assert_eq!(prev_word_start(&line, 0, false), None);

        // `e`: word ends.
        assert_eq!(word_end(&line, 0, false), Some(2)); // end of 'foo'
        assert_eq!(word_end(&line, 2, false), Some(3)); // the ',' is a 1-char word
        assert_eq!(word_end(&line, 5, false), Some(11)); // end of 'bar_baz'

        // `^`: first non-blank.
        assert_eq!(first_non_blank(&"   hi".chars().collect::<Vec<_>>()), 3);
        assert_eq!(first_non_blank(&"".chars().collect::<Vec<_>>()), 0);
    }

    #[test]
    fn test_vim_big_word_motions_ignore_punctuation() {
        // WORD motions span punctuation: only whitespace separates WORDs.
        let line: Vec<char> = "foo, bar_baz qux".chars().collect();

        // `W`: "foo," is one WORD, so the next WORD is 'bar_baz' at 5, then 'qux'.
        assert_eq!(next_word_start(&line, 0, true), Some(5));
        assert_eq!(next_word_start(&line, 5, true), Some(13));

        // `B`: from 'qux' back to 'bar_baz' (5), then to "foo," (0).
        assert_eq!(prev_word_start(&line, 13, true), Some(5));
        assert_eq!(prev_word_start(&line, 5, true), Some(0));

        // `E`: end of "foo," is the comma at 3 (vs `e` which stops at 'foo').
        assert_eq!(word_end(&line, 0, true), Some(3));
    }

    #[test]
    fn test_status_bar_labels_each_mode() {
        let theme = Theme::dark();
        assert_eq!(status_bar(Mode::Insert, &theme).label, " INSERT ");
        assert_eq!(status_bar(Mode::Normal, &theme).label, " NORMAL ");
        assert_eq!(status_bar(Mode::BlockFocus, &theme).label, " BLOCK ");
        // The accent is mode-coded, so Insert and Normal differ.
        assert_ne!(
            status_bar(Mode::Insert, &theme).accent,
            status_bar(Mode::Normal, &theme).accent
        );
    }

    #[test]
    fn test_is_boundary_ding_key() {
        // Backspace, arrows, Home/End, Delete are recognized.
        assert!(is_boundary_ding_key(&[0x7f]));
        assert!(is_boundary_ding_key(b"\x1b[A")); // Up
        assert!(is_boundary_ding_key(b"\x1b[B")); // Down
        assert!(is_boundary_ding_key(b"\x1b[C")); // Right
        assert!(is_boundary_ding_key(b"\x1b[D")); // Left
        assert!(is_boundary_ding_key(b"\x1b[H")); // Home
        assert!(is_boundary_ding_key(b"\x1b[F")); // End
        assert!(is_boundary_ding_key(b"\x1bOP")); // Delete
        // Ordinary input is not.
        assert!(!is_boundary_ding_key(b"a"));
        assert!(!is_boundary_ding_key(b"\r"));
        assert!(!is_boundary_ding_key(b"\x1b[5~")); // PageUp
        assert!(!is_boundary_ding_key(&[]));
    }

    #[test]
    fn test_winit_key_to_code_chars() {
        assert_eq!(
            winit_key_to_code(&Key::Character("a".into())),
            KeyCode::Char('a')
        );
        assert_eq!(
            winit_key_to_code(&Key::Named(NamedKey::Enter)),
            KeyCode::Enter
        );
        assert_eq!(
            winit_key_to_code(&Key::Named(NamedKey::Space)),
            KeyCode::Space
        );
    }

    #[test]
    fn test_layout_rect_conversion() {
        let rect = Rect::new(10.0, 20.0, 400.0, 300.0);
        let pane_rect = App::layout_rect_to_pane(rect);
        assert_eq!(pane_rect.x, 10.0);
        assert_eq!(pane_rect.y, 20.0);
    }

    #[test]
    fn test_bell_flash_duration() {
        let _ = BELL_FLASH_DURATION;
    }

    #[test]
    fn test_word_chars_contains_alphanumeric() {
        assert!(WORD_CHARS.contains('a'));
        assert!(WORD_CHARS.contains('Z'));
        assert!(WORD_CHARS.contains('0'));
        assert!(WORD_CHARS.contains('_'));
        assert!(!WORD_CHARS.contains(' '));
    }

    #[test]
    fn test_quick_select_labels_constants() {
        assert!(!QUICK_SELECT_LABELS.is_empty());
        assert_eq!(QUICK_SELECT_LABELS[0], 'a');
    }

    #[test]
    fn test_app_new_has_no_quick_select() {
        let app = App::new();
        assert!(app.quick_select.is_none());
    }
}
