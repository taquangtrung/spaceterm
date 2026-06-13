//! The native application: winit event loop, GPU renderer, and PTY panes wired
//! together. This is the `SpaceTerm` binary's core runtime — keyboard input drives
//! the PTY, PTY output drives the cell grid, and the grid is rendered to the
//! GPU surface every frame.
//!
//! Submodules split the `App`'s responsibilities by concern:
//! - [`init`] — GPU/window bootstrap on `resumed`.
//! - [`actions`] — keyboard action dispatch.
//! - [`render`] — frame composition and WebView tile management.
//! - [`blocks`] — block fold / yank / focus operations.
//! - [`navigation`] — vim-style cursor motions, search, quick-select.
//! - [`pointer`] — mouse hit-testing, selection, clipboard, PTY mouse forwarding.

pub mod actions;
mod blocks;
mod init;
mod navigation;
mod pointer;
mod prompt_edit;
mod render;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::config::{Config, StatusBarIconsConfig};
use crate::model::input::{self, KeyCode, PendingPrefix};
use crate::model::layout::{Direction, FocusDir, PaneId, Rect, Tab};
use crate::model::mode::Mode;
use crate::model::palette::Palette;
use crate::session::Session;
use crate::terminal::pane::Pane;
use crate::terminal::webview::WebViewManager;
use spaceterm_render::renderer::{GpuRenderer, PaneRect};
use spaceterm_render::{StatusBar, Theme};

// ========================================================================
// Constants
// ========================================================================

const PTY_POLL_INTERVAL: Duration = Duration::from_millis(16);
const SPLIT_RATIO: f32 = 0.5;
/// Cell rows reserved at the bottom of the surface for the status bar.
pub(crate) const STATUS_BAR_ROWS: usize = 1;
const BELL_FLASH_DURATION: Duration = Duration::from_millis(150);
/// A bell arriving within this window after a cursor-navigation or edit key is
/// treated as the shell's boundary ding (e.g. Backspace with nothing to delete,
/// or Left at the start of the prompt), and its flash is suppressed.
const EDIT_KEY_BELL_WINDOW: Duration = Duration::from_millis(150);

const DEFAULT_COLS: u32 = 80;
const DEFAULT_ROWS: u32 = 24;
const APPROX_CELL_WIDTH: u32 = 9;
const APPROX_CELL_HEIGHT: u32 = 20;

// ========================================================================
// Free functions
// ========================================================================

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

/// Rows available to panes after reserving the status bar row, never below one.
pub(crate) fn content_rows(full_rows: usize) -> usize {
    full_rows.saturating_sub(STATUS_BAR_ROWS).max(1)
}

fn status_bar(
    mode: Mode,
    theme: &Theme,
    pane_title: Option<String>,
    icons: &StatusBarIconsConfig,
) -> StatusBar {
    let (mode_name, accent) = match mode {
        Mode::Insert => ("Insert", theme.ansi[2]),
        Mode::Normal => ("Normal", theme.ansi[4]),
        Mode::Visual => ("Visual", theme.ansi[6]),
        Mode::BlockFocus => ("Block", theme.ansi[5]),
    };
    // Visual shares the Normal icon: it is a navigation sub-mode, not a separate
    // configurable glyph.
    let mode_icon = match mode {
        Mode::Insert => &icons.insert,
        Mode::Normal | Mode::Visual => &icons.normal,
        Mode::BlockFocus => &icons.block,
    };
    let mode_label = if mode_icon.is_empty() {
        mode_name.to_string()
    } else {
        format!("{} {}", mode_icon, mode_name)
    };
    let right_label = if icons.branding.is_empty() {
        None
    } else {
        Some(format!("{} spaceterm", icons.branding))
    };
    StatusBar {
        accent,
        mode: mode_label,
        pane_title,
        right_label,
    }
}

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
// Data Structures
// ========================================================================

/// The application state, driven by winit's `ApplicationHandler` trait.
pub struct App {
    pub(crate) bell_until: Option<Instant>,
    pub(crate) config: Config,
    pub(crate) cursor_pos: (f32, f32),
    pub(crate) dirty: bool,
    pub(crate) folded_blocks: HashMap<PaneId, HashSet<usize>>,
    /// Raster-image blocks rendered natively on the GPU (instead of a WebView).
    pub(crate) image_blocks: Vec<ImageBlock>,
    pub(crate) last_click: Option<(Instant, f32, f32)>,
    pub(crate) last_edit_key: Option<Instant>,
    pub(crate) last_tile_layout: Option<(usize, usize, u32, u32)>,
    pub(crate) modifiers: winit::event::Modifiers,
    pub(crate) mouse_down: bool,
    /// The Normal-mode traversal cursor for the focused pane, in viewport
    /// `(row, col)`. `Some` only while that pane is in Normal mode.
    pub(crate) nav_cursor: Option<(usize, usize)>,
    /// Set after a Vim prompt edit so the nav cursor is re-seeded to the shell
    /// cursor once the resulting PTY echo is drained.
    pub(crate) nav_resync_pending: bool,
    pub(crate) next_image_id: u64,
    pub(crate) panes: HashMap<PaneId, Pane>,
    pub(crate) palette: Option<Palette>,
    pub(crate) pane_titles: HashMap<PaneId, String>,
    pub(crate) pending: PendingPrefix,
    pub(crate) quick_select: Option<Vec<QuickLabel>>,
    pub(crate) renderer: Option<GpuRenderer>,
    pub(crate) search_query: Option<String>,
    pub(crate) selection: Option<Selection>,
    pub(crate) tab: Tab,
    pub(crate) modes: HashMap<PaneId, Mode>,
    /// Visual-mode anchor (viewport `(row, col)`) where the selection began.
    /// `Some` only while the focused pane is in Visual mode.
    pub(crate) visual_anchor: Option<(usize, usize)>,
    /// Whether the active Visual selection is linewise (`V`) rather than
    /// charwise (`v`).
    pub(crate) visual_line: bool,
    pub(crate) webview_mgr: WebViewManager,
    pub(crate) window: Option<Arc<Window>>,
    pub(crate) window_title: String,
}

/// A block drawn natively via the GPU. `id` keys the renderer's texture cache;
/// `nat_w`/`nat_h` are the rendered pixel dimensions, used to preserve aspect
/// ratio when placing it at `grid_row`. Width-wrapped blocks carry their source
/// in `reflow` so they can be re-rasterized at `rastered_width` on resize.
pub(crate) struct ImageBlock {
    /// True for images/SVG (scaled down to fit the reserved band); false for
    /// text/markdown (shown at native size and clipped to the band).
    pub fit_to_band: bool,
    pub grid_row: usize,
    pub id: u64,
    /// Band height in rows the block is drawn into — matches the rows reserved
    /// for it in the grid, so the following prompt sits flush below.
    pub max_rows: usize,
    pub nat_h: u32,
    pub nat_w: u32,
    pub pane_id: PaneId,
    pub rastered_width: u32,
    pub reflow: Option<ReflowSource>,
}

/// Source content for a width-wrapped native block, retained so the block can
/// be re-rasterized when the pane width changes.
pub(crate) enum ReflowSource {
    Markdown(String),
    Text(String),
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct QuickLabel {
    pub col: usize,
    pub label: char,
    pub row: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct Selection {
    pub end_col: usize,
    pub end_row: usize,
    pub pane: PaneId,
    pub start_col: usize,
    pub start_row: usize,
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
            image_blocks: Vec::new(),
            last_tile_layout: None,
            modifiers: winit::event::Modifiers::default(),
            mouse_down: false,
            nav_cursor: None,
            nav_resync_pending: false,
            next_image_id: 0,
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
            visual_anchor: None,
            visual_line: false,
            webview_mgr: WebViewManager::new(),
            window: None,
            window_title: String::new(),
        }
    }

    pub(crate) fn viewport_rect(&self) -> PaneRect {
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

    pub(crate) fn layout_rect_to_pane(rect: Rect) -> PaneRect {
        PaneRect {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        }
    }

    pub(crate) fn update_window_title(&mut self) {
        let Some(window) = self.window.clone() else {
            return;
        };

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

    pub(crate) fn flash_bell(&mut self) {
        self.bell_until = Some(Instant::now() + BELL_FLASH_DURATION);
        self.dirty = true;
    }

    pub(crate) fn is_bell_active(&self) -> bool {
        self.bell_until.is_some_and(|t| Instant::now() < t)
    }

    pub(crate) fn split_pane(&mut self, direction: Direction) {
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

    pub(crate) fn close_pane(&mut self, pane_id: PaneId) {
        if self.tab.panes().len() <= 1 {
            return;
        }
        self.panes.remove(&pane_id);
        self.modes.remove(&pane_id);
        self.pane_titles.remove(&pane_id);
        self.webview_mgr.remove_tiles_for_pane(pane_id);
        self.image_blocks.retain(|img| img.pane_id != pane_id);
        self.last_tile_layout = None;
        self.tab.close(pane_id);
        if let Some(renderer) = &self.renderer {
            let grid_size = renderer.grid_size();
            self.resize_all_panes(grid_size);
        }
        self.dirty = true;
    }

    pub(crate) fn reap_dead_panes(&mut self) {
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

    pub(crate) fn drain_all_panes(&mut self) -> bool {
        let mut any = false;
        let mut any_bell = false;
        let mut new_entries: Vec<(PaneId, crate::terminal::block_queue::BlockEntry)> = Vec::new();
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

    pub(crate) fn resize_all_panes(&mut self, (full_cols, full_rows): (usize, usize)) {
        let (cw, ch) = if let Some(renderer) = &self.renderer {
            renderer.cell_size()
        } else {
            (9.0, 20.0)
        };

        let layout_vp = Rect::new(
            0.0,
            0.0,
            full_cols as f32 * cw,
            content_rows(full_rows) as f32 * ch,
        );
        let rects = self.tab.rects(layout_vp);

        for (id, rect) in rects {
            let cols = (rect.width / cw).floor() as usize;
            let rows = (rect.height / ch).floor() as usize;
            if let Some(pane) = self.panes.get_mut(&id) {
                pane.resize(cols.max(1), rows.max(1));
            }
        }
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
                    Mode::Normal | Mode::Visual | Mode::BlockFocus => Mode::Insert,
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
                let layout_vp = Rect::new(viewport.x, viewport.y, viewport.width, viewport.height);
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
                let folded = self.folded_blocks.entry(focused).or_default();
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
}

// ========================================================================
// ApplicationHandler
// ========================================================================

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        self.init_window(event_loop);
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
                let mouse_active = self.panes.get(&focused).is_some_and(|p| p.mouse_tracking());

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
                if self.panes.get(&focused).is_some_and(|p| p.mouse_tracking()) {
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
            // A Vim prompt edit just echoed back: re-seed the nav cursor onto the
            // shell's new cursor position so it tracks the edited line.
            if self.nav_resync_pending {
                let focused = self.tab.focused();
                if self.modes.get(&focused) == Some(&Mode::Normal) {
                    self.init_nav_cursor(focused);
                }
                self.nav_resync_pending = false;
            }
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
        assert_eq!(content_rows(1), 1);
        assert_eq!(content_rows(0), 1);
    }

    #[test]
    fn test_status_bar_labels_each_mode() {
        let theme = Theme::dark();
        let icons = StatusBarIconsConfig::default();
        assert_eq!(
            status_bar(Mode::Insert, &theme, None, &icons).mode,
            "\u{f03eb} Insert"
        );
        assert_eq!(
            status_bar(Mode::Normal, &theme, None, &icons).mode,
            "\u{e795} Normal"
        );
        assert_eq!(
            status_bar(Mode::BlockFocus, &theme, None, &icons).mode,
            "\u{f0485} Block"
        );
        assert_ne!(
            status_bar(Mode::Insert, &theme, None, &icons).accent,
            status_bar(Mode::Normal, &theme, None, &icons).accent
        );
    }

    #[test]
    fn test_is_boundary_ding_key() {
        assert!(is_boundary_ding_key(&[0x7f]));
        assert!(is_boundary_ding_key(b"\x1b[A"));
        assert!(is_boundary_ding_key(b"\x1b[B"));
        assert!(is_boundary_ding_key(b"\x1b[C"));
        assert!(is_boundary_ding_key(b"\x1b[D"));
        assert!(is_boundary_ding_key(b"\x1b[H"));
        assert!(is_boundary_ding_key(b"\x1b[F"));
        assert!(is_boundary_ding_key(b"\x1bOP"));
        assert!(!is_boundary_ding_key(b"a"));
        assert!(!is_boundary_ding_key(b"\r"));
        assert!(!is_boundary_ding_key(b"\x1b[5~"));
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
    fn test_app_new_has_no_quick_select() {
        let app = App::new();
        assert!(app.quick_select.is_none());
    }
}
