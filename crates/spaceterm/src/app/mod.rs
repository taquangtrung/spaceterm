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
mod chrome;
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

use crate::config::{Config, StatusBarConfig};
use crate::model::input::{self, Action, KeyCode, PendingPrefix, WindowKeymap};
use crate::model::layout::{Direction, FocusDir, PaneId, Rect, Tab};
use crate::model::mode::Mode;
use crate::model::palette::Palette;
use crate::session::Session;
use crate::terminal::pane::Pane;
use crate::terminal::settings_view::{SettingsMsg, SettingsView};
use crate::terminal::webview::WebViewManager;
use spaceterm_render::renderer::{GpuRenderer, PaneRect};
use spaceterm_render::{MenuStyle, StatusBar, Theme};

// ========================================================================
// Constants
// ========================================================================

const PTY_POLL_INTERVAL: Duration = Duration::from_millis(16);
const SPLIT_RATIO: f32 = 0.5;
/// Cell rows reserved at the bottom of the surface for the status bar.
pub(crate) const STATUS_BAR_ROWS: usize = 1;
const BELL_FLASH_DURATION: Duration = Duration::from_millis(150);
/// How long a transient status-bar error notice stays on screen before it
/// expires and the bar returns to showing the pane title.
const ERROR_NOTICE_DURATION: Duration = Duration::from_secs(3);
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
    notice: Option<String>,
    config: &StatusBarConfig,
) -> StatusBar {
    let icons = &config.icons;
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
    let mode_label = if !config.show_mode {
        String::new()
    } else if mode_icon.is_empty() {
        mode_name.to_string()
    } else {
        format!("{} {}", mode_icon, mode_name)
    };
    let pane_title = if config.show_title { pane_title } else { None };
    // An empty right label suppresses the branding entirely; `None` would let the
    // renderer fall back to its default branding text.
    let right_label = if !config.show_branding {
        Some(String::new())
    } else if icons.branding.is_empty() {
        None
    } else {
        Some(format!("{} spaceterm", icons.branding))
    };
    StatusBar {
        accent,
        mode: mode_label,
        notice,
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
    /// A transient error notice and the instant it expires, shown in the status
    /// bar (e.g. a Vim edit aimed at the non-editable scrollback area).
    pub(crate) error_notice: Option<(String, Instant)>,
    pub(crate) folded_blocks: HashMap<PaneId, HashSet<usize>>,
    /// Raster-image blocks rendered natively on the GPU (instead of a WebView).
    pub(crate) image_blocks: Vec<ImageBlock>,
    pub(crate) last_click: Option<(Instant, f32, f32)>,
    pub(crate) last_edit_key: Option<Instant>,
    /// The last char-search (`f`/`F`/`t`/`T`), repeated by `;` and `,`.
    pub(crate) last_find: Option<input::FindChar>,
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
    /// The open full-window settings page (a child WebView), or `None`.
    pub(crate) settings_view: Option<SettingsView>,
    /// Which tab is shown; index into [`Self::tabs`].
    pub(crate) active_tab: usize,
    /// Tab indices in most-recently-used order (front = the current tab after a
    /// deliberate switch). Drives the recency tab commands.
    pub(crate) tab_mru: Vec<usize>,
    /// Cursor into [`Self::tab_mru`] while a recency walk is in progress, so
    /// repeated recency commands step through usage order without reshuffling it.
    /// `None` once a deliberate switch ends the walk.
    pub(crate) mru_walk: Option<usize>,
    /// The open menu/dropdown (index into the chrome's menu list), or `None`.
    pub(crate) open_menu: Option<usize>,
    /// Index of the open submenu's parent within the open menu's items, or `None`.
    pub(crate) open_submenu: Option<usize>,
    /// The hovered dropdown item while a menu is open.
    pub(crate) selected_item: Option<usize>,
    /// The hovered submenu child while a submenu is open.
    pub(crate) selected_subitem: Option<usize>,
    /// Next free globally-unique pane id; allocated by [`Self::alloc_pane_id`].
    pub(crate) next_pane_id: u64,
    /// All open tabs, each its own split-tree of panes.
    pub(crate) tabs: Vec<Tab>,
    pub(crate) modes: HashMap<PaneId, Mode>,
    /// Visual-mode anchor (viewport `(row, col)`) where the selection began.
    /// `Some` only while the focused pane is in Visual mode.
    pub(crate) visual_anchor: Option<(usize, usize)>,
    /// Whether the active Visual selection is linewise (`V`) rather than
    /// charwise (`v`).
    pub(crate) visual_line: bool,
    pub(crate) webview_mgr: WebViewManager,
    pub(crate) window: Option<Arc<Window>>,
    /// Configurable split/close/focus key bindings (the `window` keybindings
    /// block), resolved against in Normal mode.
    pub(crate) window_keymap: WindowKeymap,
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

        let config = Config::load();
        let window_keymap = WindowKeymap::from_config(config.keybindings.get("window"));

        Self {
            bell_until: None,
            config,
            cursor_pos: (0.0, 0.0),
            dirty: true,
            error_notice: None,
            folded_blocks: HashMap::new(),
            last_click: None,
            last_edit_key: None,
            last_find: None,
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
            settings_view: None,
            active_tab: 0,
            tab_mru: vec![0],
            mru_walk: None,
            open_menu: None,
            open_submenu: None,
            selected_item: None,
            selected_subitem: None,
            next_pane_id: 1,
            tabs: vec![Tab::new()],
            modes,
            visual_anchor: None,
            visual_line: false,
            webview_mgr: WebViewManager::new(),
            window: None,
            window_keymap,
            window_title: String::new(),
        }
    }

    /// The currently visible tab.
    pub(crate) fn tab(&self) -> &Tab {
        &self.tabs[self.active_tab]
    }

    /// The currently visible tab, mutably.
    pub(crate) fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active_tab]
    }

    /// Allocate the next globally-unique pane id.
    pub(crate) fn alloc_pane_id(&mut self) -> PaneId {
        let id = PaneId(self.next_pane_id);
        self.next_pane_id += 1;
        id
    }

    /// Number of top cell rows reserved for the tabbar/menubar chrome.
    pub(crate) fn top_chrome_rows(&self) -> usize {
        spaceterm_render::chrome_rows(self.config.menu_style)
    }

    /// Bottom cell rows reserved for the status bar: one when shown, zero when
    /// the user has hidden it.
    pub(crate) fn status_bar_rows(&self) -> usize {
        if self.config.status_bar.enabled {
            STATUS_BAR_ROWS
        } else {
            0
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
        // Reserve the status bar row at the bottom and the tabbar/menubar rows at
        // the top, so pane hit-testing and focus geometry match the area actually
        // drawn to panes.
        let ch = self
            .renderer
            .as_ref()
            .map(|r| r.cell_size().1)
            .unwrap_or(0.0);
        let status_h = ch * self.status_bar_rows() as f32;
        let top_h = ch * self.top_chrome_rows() as f32;
        PaneRect {
            x: 0.0,
            y: top_h,
            width: w,
            height: (h - status_h - top_h).max(1.0),
        }
    }

    /// The pane area as a layout `Rect` (same coordinates as [`PaneRect`]).
    pub(crate) fn content_viewport(&self) -> Rect {
        let vp = self.viewport_rect();
        Rect::new(vp.x, vp.y, vp.width, vp.height)
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
            match self.pane_titles.get(&self.tab().focused()) {
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

    /// Show `message` as a transient error notice in the status bar.
    pub(crate) fn set_error(&mut self, message: impl Into<String>) {
        self.error_notice = Some((message.into(), Instant::now() + ERROR_NOTICE_DURATION));
        self.dirty = true;
    }

    /// The current error notice text, if one is set and has not yet expired.
    pub(crate) fn active_error_notice(&self) -> Option<&str> {
        self.error_notice
            .as_ref()
            .filter(|(_, expiry)| Instant::now() < *expiry)
            .map(|(text, _)| text.as_str())
    }

    pub(crate) fn split_pane(&mut self, direction: Direction) {
        let new_id = self.alloc_pane_id();
        self.tab_mut().split(direction, SPLIT_RATIO, new_id);

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

        if self.renderer.is_some() {
            self.resize_all_panes();
        }
        self.dirty = true;
    }

    pub(crate) fn close_pane(&mut self, pane_id: PaneId) {
        self.close_pane_in_any_tab(pane_id);
    }

    /// Close `pane_id` in whichever tab holds it, collapsing its split into the
    /// sibling. The last pane of a tab is not closed here (tabs are closed via
    /// [`Self::close_tab`]). Drops all per-pane state and re-lays-out if the
    /// affected tab is the active one.
    fn close_pane_in_any_tab(&mut self, pane_id: PaneId) {
        let Some(tab_idx) = self.tabs.iter().position(|t| t.panes().contains(&pane_id)) else {
            return;
        };
        if self.tabs[tab_idx].panes().len() <= 1 {
            return;
        }
        self.panes.remove(&pane_id);
        self.modes.remove(&pane_id);
        self.pane_titles.remove(&pane_id);
        self.webview_mgr.remove_tiles_for_pane(pane_id);
        self.image_blocks.retain(|img| img.pane_id != pane_id);
        self.last_tile_layout = None;
        self.tabs[tab_idx].close(pane_id);
        if tab_idx == self.active_tab && self.renderer.is_some() {
            self.resize_all_panes();
        }
        self.dirty = true;
    }

    /// Close every pane in the tab except `focused` (Vim `Ctrl-w o`).
    pub(crate) fn close_other_panes(&mut self, focused: PaneId) {
        let others: Vec<PaneId> = self
            .tab()
            .panes()
            .into_iter()
            .filter(|&id| id != focused)
            .collect();
        for id in others {
            self.close_pane(id);
        }
    }

    /// Open a new tab with a fresh shell pane and switch to it.
    pub(crate) fn new_tab(&mut self) {
        let id = self.alloc_pane_id();
        let (cols, rows) = self
            .renderer
            .as_ref()
            .map(|r| r.grid_size())
            .unwrap_or((DEFAULT_COLS as usize, DEFAULT_ROWS as usize));
        // Sized roughly now; resize_all_panes fixes the exact grid once placed.
        let pane = Pane::new(cols.max(1), rows.max(1));
        self.panes.insert(id, pane);
        self.modes.insert(id, Mode::default());
        self.tabs.push(Tab::with_root(id));
        self.active_tab = self.tabs.len() - 1;
        self.touch_mru(self.active_tab);
        self.close_menu();
        self.last_tile_layout = None;
        if self.renderer.is_some() {
            self.resize_all_panes();
        }
        self.dirty = true;
        self.update_window_title();
    }

    /// Switch the visible tab to `index` as a deliberate selection: record it as
    /// most-recently-used (ending any recency walk) and show it.
    pub(crate) fn switch_tab(&mut self, index: usize) {
        if index >= self.tabs.len() || index == self.active_tab {
            return;
        }
        self.touch_mru(index);
        self.activate_tab(index);
    }

    /// Make `index` the visible tab without touching the MRU order. Shared by
    /// deliberate switches ([`Self::switch_tab`]) and recency walks
    /// ([`Self::recent_tab`]).
    fn activate_tab(&mut self, index: usize) {
        self.active_tab = index;
        self.selection = None;
        // Force a tile reposition so background-tab WebViews are hidden and the
        // new tab's are shown (the layout key alone may not have changed).
        self.last_tile_layout = None;
        if self.renderer.is_some() {
            self.resize_all_panes();
        }
        self.dirty = true;
        self.update_window_title();
    }

    /// Move `index` to the front of the most-recently-used order (inserting it if
    /// new) and end any in-progress recency walk.
    fn touch_mru(&mut self, index: usize) {
        self.tab_mru.retain(|&i| i != index);
        self.tab_mru.insert(0, index);
        self.mru_walk = None;
    }

    /// Cycle to the next (`forward`) or previous tab by position, wrapping around.
    pub(crate) fn cycle_tab(&mut self, forward: bool) {
        let count = self.tabs.len();
        if count <= 1 {
            return;
        }
        let next = if forward {
            (self.active_tab + 1) % count
        } else {
            (self.active_tab + count - 1) % count
        };
        self.switch_tab(next);
    }

    /// Switch tabs in most-recently-used order: `forward` steps toward more
    /// recently used, otherwise toward less recently used, wrapping around. The
    /// MRU order is held still across consecutive calls (a "walk") so the user can
    /// step back and forth through usage history; the next deliberate switch ends
    /// the walk and re-seeds the order from the chosen tab.
    pub(crate) fn recent_tab(&mut self, forward: bool) {
        let count = self.tabs.len();
        if count <= 1 {
            return;
        }
        // Guard against any drift from tab open/close bookkeeping: a malformed
        // order is rebuilt with the current tab most-recent.
        if self.tab_mru.len() != count {
            self.tab_mru = (0..count).collect();
            self.touch_mru(self.active_tab);
        }
        let cursor = self.mru_walk.unwrap_or(0);
        let next = if forward {
            (cursor + count - 1) % count
        } else {
            (cursor + 1) % count
        };
        self.mru_walk = Some(next);
        self.activate_tab(self.tab_mru[next]);
    }

    /// Close tab `index`, dropping all its panes. The last tab is never closed.
    pub(crate) fn close_tab(&mut self, index: usize) {
        if self.tabs.len() <= 1 || index >= self.tabs.len() {
            return;
        }
        for id in self.tabs[index].panes() {
            self.panes.remove(&id);
            self.modes.remove(&id);
            self.pane_titles.remove(&id);
            self.webview_mgr.remove_tiles_for_pane(id);
            self.image_blocks.retain(|img| img.pane_id != id);
        }
        self.tabs.remove(index);
        if index < self.active_tab {
            self.active_tab -= 1;
        }
        self.active_tab = self.active_tab.min(self.tabs.len() - 1);
        // Drop the closed tab from the MRU order and shift the indices above it
        // down, then re-seed the current tab as most-recent.
        self.tab_mru.retain(|&i| i != index);
        for i in self.tab_mru.iter_mut() {
            if *i > index {
                *i -= 1;
            }
        }
        self.touch_mru(self.active_tab);
        self.close_menu();
        self.last_tile_layout = None;
        if self.renderer.is_some() {
            self.resize_all_panes();
        }
        self.dirty = true;
        self.update_window_title();
    }

    pub(crate) fn reap_dead_panes(&mut self) {
        let dead: Vec<PaneId> = self
            .panes
            .iter_mut()
            .filter_map(|(id, pane)| if pane.is_alive() { None } else { Some(*id) })
            .collect();
        for id in dead {
            // Only reap when its tab still has another pane; the last pane of a
            // tab is reaped by [`Self::close_tab`] logic instead.
            if self
                .tabs
                .iter()
                .any(|t| t.panes().contains(&id) && t.panes().len() > 1)
            {
                self.close_pane_in_any_tab(id);
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

    pub(crate) fn resize_all_panes(&mut self) {
        let (cw, ch) = if let Some(renderer) = &self.renderer {
            renderer.cell_size()
        } else {
            (9.0, 20.0)
        };

        let layout_vp = self.content_viewport();
        let rects = self.tab().rects(layout_vp);

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
                    self.run_command(action, focused);
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

    /// Run a named command, shared by the command palette and the menus.
    pub(crate) fn run_command(&mut self, action: &str, focused: PaneId) {
        match action {
            "new_tab" => {
                self.new_tab();
            }
            "close_tab" => {
                self.close_tab(self.active_tab);
            }
            "next_tab" => {
                self.cycle_tab(true);
            }
            "prev_tab" => {
                self.cycle_tab(false);
            }
            "recent_tab_back" => {
                self.recent_tab(false);
            }
            "recent_tab_forward" => {
                self.recent_tab(true);
            }
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
                self.tab_mut().focus_in_direction(dir, layout_vp);
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
                self.rebuild_theme();
            }
            "theme_light" => {
                self.config.theme = crate::config::ThemeSetting::Light;
                self.rebuild_theme();
            }
            "theme_auto" => {
                self.config.theme = crate::config::ThemeSetting::Auto;
                self.rebuild_theme();
            }
            "open_settings" => {
                self.open_settings();
            }
            _ => {}
        }
    }

    /// Rebuild the renderer theme from the current `config.theme` selection plus
    /// any color overrides, and request a redraw. Shared by the theme menu
    /// commands and the live settings page.
    pub(crate) fn rebuild_theme(&mut self) {
        use crate::config::ThemeSetting;
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        let mut theme = match &self.config.theme {
            ThemeSetting::Dark => Theme::dark(),
            ThemeSetting::Light => Theme::light(),
            ThemeSetting::Auto => self
                .window
                .as_ref()
                .and_then(|w| w.theme())
                .map(|t| match t {
                    winit::window::Theme::Dark => Theme::dark(),
                    winit::window::Theme::Light => Theme::light(),
                })
                .unwrap_or_default(),
            // A user theme file; fall back to the dark preset if it is missing.
            ThemeSetting::Named(name) => {
                crate::config::load_named_theme(name).unwrap_or_else(Theme::dark)
            }
        };
        self.config.colors.apply(&mut theme);
        renderer.set_theme(theme);
        self.dirty = true;
    }

    /// Open the full-window settings page, dismissing any open menu or palette
    /// first. A no-op if it is already open or the window/renderer are not ready.
    pub(crate) fn open_settings(&mut self) {
        if self.settings_view.is_some() {
            return;
        }
        self.close_menu();
        self.palette = None;
        let opened = match (&self.window, &self.renderer) {
            (Some(window), Some(renderer)) => {
                Some(SettingsView::open(window, &self.config, renderer.theme()))
            }
            _ => None,
        };
        match opened {
            Some(Ok(view)) => {
                self.settings_view = Some(view);
                self.dirty = true;
            }
            Some(Err(e)) => {
                eprintln!("spaceterm: settings page error: {e}");
                self.set_error("settings unavailable");
            }
            None => {}
        }
    }

    /// Close the settings page (dropping its WebView) and restore the panes.
    pub(crate) fn close_settings(&mut self) {
        if self.settings_view.take().is_some() {
            // The overlay is gone; force block tiles to re-show and re-position.
            self.last_tile_layout = None;
            self.dirty = true;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }

    /// Drain and apply edits queued by the settings page, persisting the config
    /// when anything changed and closing the page on a close request.
    pub(crate) fn apply_settings(&mut self) {
        let Some(view) = &self.settings_view else {
            return;
        };
        let messages = view.drain();
        if messages.is_empty() {
            return;
        }

        let mut should_close = false;
        let mut changed = false;
        for message in messages {
            match message {
                SettingsMsg::Close => should_close = true,
                SettingsMsg::Set(set) => changed |= self.apply_setting(&set.key, &set.value),
            }
        }

        if changed {
            if let Err(e) = self.config.save() {
                eprintln!("spaceterm: could not save settings: {e}");
            }
            self.dirty = true;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        if should_close {
            self.close_settings();
        }
    }

    /// Apply one settings edit to the live config and perform any renderer or
    /// layout refresh it implies. Returns whether the config changed (and so
    /// should be persisted); an unparseable value leaves the config untouched.
    fn apply_setting(&mut self, key: &str, value: &str) -> bool {
        use crate::config::ThemeSetting;
        match key {
            "theme" => {
                self.config.theme = ThemeSetting::from_value(value);
                self.rebuild_theme();
            }
            "menu_style" => {
                self.config.menu_style = match value {
                    "classic" => MenuStyle::Classic,
                    _ => MenuStyle::Modern,
                };
                self.relayout_chrome();
            }
            "font_family" => {
                let trimmed = value.trim();
                self.config.font_family = (!trimmed.is_empty()).then(|| trimmed.to_string());
            }
            "font_size" => match value.parse::<f32>() {
                Ok(size) => self.config.font_size = size,
                Err(_) => return false,
            },
            "opacity" => match value.parse::<f32>() {
                Ok(opacity) => self.config.opacity = opacity.clamp(0.1, 1.0),
                Err(_) => return false,
            },
            "status.enabled" => {
                self.config.status_bar.enabled = value == "true";
                self.relayout_chrome();
            }
            "status.show_mode" => {
                self.config.status_bar.show_mode = value == "true";
                self.dirty = true;
            }
            "status.show_title" => {
                self.config.status_bar.show_title = value == "true";
                self.dirty = true;
            }
            "status.show_branding" => {
                self.config.status_bar.show_branding = value == "true";
                self.dirty = true;
            }
            _ => return false,
        }
        true
    }

    /// Re-lay-out panes after a change to the reserved top-chrome or status-bar
    /// rows (menu style, status-bar visibility), and request a redraw.
    fn relayout_chrome(&mut self) {
        self.last_tile_layout = None;
        if self.renderer.is_some() {
            self.resize_all_panes();
        }
        self.dirty = true;
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
                    }
                    if self.renderer.is_some() {
                        self.resize_all_panes();
                    }
                    if let Some(view) = &self.settings_view {
                        view.resize(size.width, size.height);
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

                let focused = self.tab().focused();
                let mode = self.modes.get(&focused).copied().unwrap_or_default();

                let mods_state = self.modifiers.state();
                let key = input::Key {
                    alt: mods_state.alt_key(),
                    code: winit_key_to_code(&event.logical_key),
                    ctrl: mods_state.control_key(),
                    shift: mods_state.shift_key(),
                };

                // While the settings page is up it owns input via its WebView; the
                // page closes itself on Esc, but if focus is elsewhere honor Esc
                // here too and swallow other keys so they don't reach the PTY.
                if self.settings_view.is_some() {
                    if key.code == KeyCode::Escape {
                        self.close_settings();
                    }
                    return;
                }

                // Esc dismisses an open menu before anything else acts on it.
                if self.open_menu.is_some() && key.code == KeyCode::Escape {
                    self.close_menu();
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }

                // Ctrl-, opens the settings page. Some keyboard layouts report the
                // shifted comma as `<`, so accept either glyph.
                if mods_state.control_key() && !mods_state.alt_key() {
                    if let Key::Character(c) = event.logical_key.as_ref() {
                        if c == "," || c == "<" {
                            self.open_settings();
                            if let Some(window) = &self.window {
                                window.request_redraw();
                            }
                            return;
                        }
                    }
                }

                // Ctrl-Tab / Ctrl-Shift-Tab cycle tabs.
                if mods_state.control_key() && key.code == KeyCode::Tab {
                    let action = if mods_state.shift_key() {
                        Action::PrevTab
                    } else {
                        Action::NextTab
                    };
                    self.handle_action(action, focused);
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return;
                }

                // Alt-1..9 jumps straight to that tab, browser style.
                if mods_state.alt_key() && !mods_state.control_key() {
                    if let KeyCode::Char(c) = key.code {
                        if let Some(n) = c.to_digit(10).filter(|d| (1..=9).contains(d)) {
                            self.handle_action(Action::GotoTab(n as usize), focused);
                            if let Some(window) = &self.window {
                                window.request_redraw();
                            }
                            return;
                        }
                    }
                }

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
                        } else if lower == "t" {
                            self.handle_action(Action::NewTab, focused);
                            if let Some(window) = &self.window {
                                window.request_redraw();
                            }
                            return;
                        } else if lower == "w" {
                            self.handle_action(Action::CloseTab(None), focused);
                            if let Some(window) = &self.window {
                                window.request_redraw();
                            }
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
                let action = input::resolve_with(
                    mode,
                    &key,
                    &mut self.pending,
                    fullscreen,
                    &self.window_keymap,
                );
                self.handle_action(action, focused);
                self.update_window_title();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }

            WindowEvent::MouseInput { state, button, .. } => {
                let focused = self.tab().focused();

                // Tabbar/menubar clicks take precedence over the panes (including
                // mouse-tracking apps), and any click resolves an open menu.
                if let (ElementState::Pressed, MouseButton::Left) = (state, button) {
                    let (x, y) = self.cursor_pos;
                    if (self.open_menu.is_some() || y < self.top_chrome_height())
                        && self.handle_chrome_click(x, y)
                    {
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                        return;
                    }
                }

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
                            self.tab_mut().focus(pane_id);
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

                if self.open_menu.is_some() {
                    self.update_menu_hover(x, y);
                }

                let focused = self.tab().focused();
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

                let focused = self.tab().focused();
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
                Session::save(self.tab(), &self.panes);
                self.panes.clear();
                event_loop.exit();
            }

            WindowEvent::Focused(focused) => {
                let focused_pane = self.tab().focused();
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

        // The settings page posts edits during the GTK pump above; apply them now.
        if self.settings_view.is_some() {
            self.apply_settings();
        }

        self.reap_dead_panes();
        let any_output = self.drain_all_panes();
        if any_output {
            // A Vim prompt edit just echoed back: re-seed the nav cursor onto the
            // shell's new cursor position so it tracks the edited line.
            if self.nav_resync_pending {
                let focused = self.tab().focused();
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

        // Once an error notice expires, redraw once to clear it from the bar.
        if self.error_notice.is_some() && self.active_error_notice().is_none() {
            self.error_notice = None;
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
        assert_eq!(app.tab().panes(), vec![PaneId(0)]);
        assert!(app.panes.is_empty());
    }

    #[test]
    fn test_alloc_pane_id_is_monotonic() {
        let mut app = App::new();
        assert_eq!(app.alloc_pane_id(), PaneId(1));
        assert_eq!(app.alloc_pane_id(), PaneId(2));
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
        let cfg = StatusBarConfig::default();
        assert_eq!(
            status_bar(Mode::Insert, &theme, None, None, &cfg).mode,
            "\u{f03eb} Insert"
        );
        assert_eq!(
            status_bar(Mode::Normal, &theme, None, None, &cfg).mode,
            "\u{e795} Normal"
        );
        assert_eq!(
            status_bar(Mode::BlockFocus, &theme, None, None, &cfg).mode,
            "\u{f0485} Block"
        );
        assert_ne!(
            status_bar(Mode::Insert, &theme, None, None, &cfg).accent,
            status_bar(Mode::Normal, &theme, None, None, &cfg).accent
        );
    }

    #[test]
    fn test_status_bar_element_toggles_hide_content() {
        let theme = Theme::dark();
        let cfg = StatusBarConfig {
            show_mode: false,
            show_title: false,
            show_branding: false,
            ..StatusBarConfig::default()
        };
        let bar = status_bar(Mode::Normal, &theme, Some("title".to_string()), None, &cfg);
        assert_eq!(bar.mode, "");
        assert_eq!(bar.pane_title, None);
        // An empty (Some) right label suppresses the default branding fallback.
        assert_eq!(bar.right_label.as_deref(), Some(""));
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

    #[test]
    fn test_set_error_surfaces_a_live_notice() {
        let mut app = App::new();
        assert!(app.active_error_notice().is_none());
        app.set_error("boom");
        assert_eq!(app.active_error_notice(), Some("boom"));
    }

    #[test]
    fn test_expired_error_notice_is_not_active() {
        let mut app = App::new();
        // An already-elapsed expiry reads as inactive.
        app.error_notice = Some(("stale".to_string(), Instant::now() - Duration::from_secs(1)));
        assert!(app.active_error_notice().is_none());
    }

    #[test]
    fn test_status_bar_notice_overrides_pane_title() {
        let theme = Theme::dark();
        let cfg = StatusBarConfig::default();
        let bar = status_bar(
            Mode::Normal,
            &theme,
            Some("title".to_string()),
            Some("oops".to_string()),
            &cfg,
        );
        assert_eq!(bar.notice.as_deref(), Some("oops"));
    }

    /// An app with `n` empty tabs (no panes/PTYs), active tab 0, MRU `[0, 1, ..]`.
    fn app_with_tabs(n: usize) -> App {
        let mut app = App::new();
        for i in 1..n {
            app.tabs.push(Tab::with_root(PaneId(i as u64)));
        }
        app.tab_mru = (0..n).collect();
        app
    }

    #[test]
    fn test_switch_tab_moves_to_front_of_mru() {
        let mut app = app_with_tabs(3);
        app.switch_tab(2);
        assert_eq!(app.active_tab, 2);
        assert_eq!(app.tab_mru, vec![2, 0, 1]);
        app.switch_tab(1);
        assert_eq!(app.tab_mru, vec![1, 2, 0]);
        assert_eq!(app.mru_walk, None);
    }

    #[test]
    fn test_recent_tab_walks_backward_and_forward_without_reshuffling() {
        let mut app = app_with_tabs(3);
        app.switch_tab(2);
        app.switch_tab(1); // MRU now [1, 2, 0], current tab 1.

        // Backward steps toward less-recently-used, holding the order still.
        app.recent_tab(false);
        assert_eq!(app.active_tab, 2);
        assert_eq!(app.mru_walk, Some(1));
        assert_eq!(app.tab_mru, vec![1, 2, 0]);
        app.recent_tab(false);
        assert_eq!(app.active_tab, 0);
        assert_eq!(app.mru_walk, Some(2));

        // Forward steps back toward more-recently-used.
        app.recent_tab(true);
        assert_eq!(app.active_tab, 2);
        assert_eq!(app.tab_mru, vec![1, 2, 0]);
    }

    #[test]
    fn test_deliberate_switch_ends_walk_and_reseeds_mru() {
        let mut app = app_with_tabs(3);
        app.switch_tab(1); // MRU [1, 0, 2], current tab 1.
        app.recent_tab(false); // Walk to tab 0 (a less-recent tab).
        assert_eq!(app.active_tab, 0);
        assert!(app.mru_walk.is_some());
        // A deliberate switch to a different tab ends the walk and re-seeds.
        app.switch_tab(2);
        assert_eq!(app.mru_walk, None);
        assert_eq!(app.tab_mru[0], 2);
    }

    #[test]
    fn test_recent_tab_is_noop_with_one_tab() {
        let mut app = App::new();
        app.recent_tab(false);
        app.recent_tab(true);
        assert_eq!(app.active_tab, 0);
        assert_eq!(app.mru_walk, None);
    }

    #[test]
    fn test_close_tab_compacts_mru_indices() {
        let mut app = app_with_tabs(3);
        app.switch_tab(2); // MRU [2, 0, 1], current tab 2.
        app.close_tab(0); // Tabs above the closed index shift down by one.
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.active_tab, 1);
        // The closed index is gone and 1->0, 2->1; current tab re-seeded to front.
        assert_eq!(app.tab_mru, vec![1, 0]);
        assert_eq!(app.mru_walk, None);
    }
}
