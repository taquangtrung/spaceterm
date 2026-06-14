//! Full-window settings page: a `wry` child WebView showing an HTML form, with
//! changes posted back over `window.ipc` and drained by the app each tick.
//!
//! The page is our own trusted HTML (no CSP, JavaScript enabled). The app builds
//! it from the live [`Config`] and effective [`Theme`] so the controls open
//! pre-filled and styled to match the terminal. Each edit becomes a
//! [`SettingsMsg`] the app applies and persists to `settings.kdl`.
//!
//! The page is a child WebView stacked over the GPU surface and any existing
//! block tiles. Block tiles emitted *while* it is open (from streaming output)
//! stack above it until it closes; this is acceptable for a brief modal edit.

use std::sync::{Arc, Mutex};

use serde_json::Value;
use winit::window::Window;
use wry::dpi::{PhysicalPosition, PhysicalSize};
use wry::{Rect, WebView, WebViewBuilder};

use spaceterm_render::{MenuStyle, Theme};

use crate::config::Config;

// ========================================================================
// Constants
// ========================================================================

const SETTINGS_HTML: &str = include_str!("settings.html");

// ========================================================================
// Data Structures
// ========================================================================

/// A queue of pending edits, shared between the WebView's IPC handler (which
/// pushes) and the app (which drains it every poll tick).
type MsgQueue = Arc<Mutex<Vec<SettingsMsg>>>;

/// A single field edit from the settings form: a `key` (e.g. `theme`,
/// `status.show_mode`) and its new `value` as text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettingsSet {
    pub key: String,
    pub value: String,
}

/// A message posted by the settings page.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SettingsMsg {
    /// The user dismissed the page (close button or Escape).
    Close,
    /// A control changed.
    Set(SettingsSet),
}

/// The open settings page: its child WebView and the edit queue its IPC handler
/// feeds. Dropping it tears the WebView down, closing the page.
pub struct SettingsView {
    queue: MsgQueue,
    webview: WebView,
}

// ========================================================================
// SettingsMsg
// ========================================================================

impl SettingsMsg {
    /// Parse one `window.ipc` payload. Returns `None` for anything that is not a
    /// recognized `{type: "set" | "close", ...}` object.
    pub fn parse(raw: &str) -> Option<Self> {
        let value: Value = serde_json::from_str(raw).ok()?;
        match value.get("type")?.as_str()? {
            "close" => Some(Self::Close),
            "set" => Some(Self::Set(SettingsSet {
                key: value.get("key")?.as_str()?.to_string(),
                value: value.get("value")?.as_str()?.to_string(),
            })),
            _ => None,
        }
    }
}

// ========================================================================
// SettingsView
// ========================================================================

impl SettingsView {
    /// Build the page as a full-window child WebView over `window`, pre-filled
    /// from `config` and styled with the effective `theme`.
    pub fn open(window: &Window, config: &Config, theme: &Theme) -> Result<Self, wry::Error> {
        let queue: MsgQueue = Arc::new(Mutex::new(Vec::new()));
        let ipc_queue = Arc::clone(&queue);
        let size = window.inner_size();

        let webview = WebViewBuilder::new()
            .with_html(render_html(config, theme))
            .with_bounds(full_window_bounds(size.width, size.height))
            .with_visible(true)
            .with_navigation_handler(|_url| false)
            .with_ipc_handler(move |req| {
                if let Some(msg) = SettingsMsg::parse(req.body()) {
                    if let Ok(mut queue) = ipc_queue.lock() {
                        queue.push(msg);
                    }
                }
            })
            .build_as_child(window)?;

        Ok(Self { queue, webview })
    }

    /// Stretch the page to fill a resized window.
    pub fn resize(&self, width: u32, height: u32) {
        let _ = self.webview.set_bounds(full_window_bounds(width, height));
    }

    /// Take all edits queued since the last call.
    pub fn drain(&self) -> Vec<SettingsMsg> {
        self.queue
            .lock()
            .map(|mut queue| std::mem::take(&mut *queue))
            .unwrap_or_default()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The WebView bounds covering the whole window, origin top-left.
fn full_window_bounds(width: u32, height: u32) -> Rect {
    Rect {
        position: PhysicalPosition::new(0, 0).into(),
        size: PhysicalSize::new(width, height).into(),
    }
}

/// Render the settings HTML, injecting the theme colors that style the page and
/// the initial control values as a JSON blob the page reads on load.
fn render_html(config: &Config, theme: &Theme) -> String {
    SETTINGS_HTML
        .replace("{{BG}}", &theme.background.to_hex())
        .replace("{{FG}}", &theme.foreground.to_hex())
        .replace("{{ACCENT}}", &theme.cursor_bg.to_hex())
        .replace("{{PANEL}}", &theme.menu_bg.to_hex())
        .replace("{{BORDER}}", &theme.divider.to_hex())
        .replace("{{STATE_JSON}}", &form_state(config).to_string())
}

/// The initial form state, keyed by the HTML control ids. `theme_options` lists
/// the built-in presets followed by every user theme file the page can pick.
fn form_state(config: &Config) -> Value {
    let menu_style = match config.menu_style {
        MenuStyle::Classic => "classic",
        MenuStyle::Modern => "modern",
    };
    let status = &config.status_bar;
    serde_json::json!({
        "theme": config.theme.as_value(),
        "theme_options": theme_options(),
        "menu_style": menu_style,
        "font_family": config.font_family.clone().unwrap_or_default(),
        "font_size": config.font_size.to_string(),
        "opacity": config.opacity.to_string(),
        "status_enabled": status.enabled,
        "status_show_mode": status.show_mode,
        "status_show_title": status.show_title,
        "status_show_branding": status.show_branding,
    })
}

/// The theme dropdown choices: the three built-ins, then each `themes/*.kdl`
/// file by name. Each entry is a `{value, label}` the page renders as an option.
fn theme_options() -> Vec<Value> {
    let mut options = vec![
        serde_json::json!({"value": "dark", "label": "Dark"}),
        serde_json::json!({"value": "light", "label": "Light"}),
        serde_json::json!({"value": "auto", "label": "Auto (follow system)"}),
    ];
    for name in crate::config::available_themes() {
        options.push(serde_json::json!({"value": name, "label": name}));
    }
    options
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_set_message() {
        let msg = SettingsMsg::parse(r#"{"type":"set","key":"theme","value":"light"}"#);
        assert_eq!(
            msg,
            Some(SettingsMsg::Set(SettingsSet {
                key: "theme".to_string(),
                value: "light".to_string(),
            }))
        );
    }

    #[test]
    fn test_parse_close_message() {
        assert_eq!(
            SettingsMsg::parse(r#"{"type":"close"}"#),
            Some(SettingsMsg::Close)
        );
    }

    #[test]
    fn test_parse_rejects_unknown_and_malformed() {
        assert_eq!(SettingsMsg::parse(r#"{"type":"frob"}"#), None);
        assert_eq!(SettingsMsg::parse("not json"), None);
        // A set message missing its value is not a valid edit.
        assert_eq!(SettingsMsg::parse(r#"{"type":"set","key":"theme"}"#), None);
    }

    #[test]
    fn test_form_state_carries_config_values() {
        let mut config = Config::default();
        config.theme = crate::config::ThemeSetting::Named("dracula".to_string());
        config.menu_style = MenuStyle::Classic;
        config.status_bar.enabled = false;

        let state = form_state(&config);
        assert_eq!(state["theme"], "dracula");
        assert_eq!(state["menu_style"], "classic");
        assert_eq!(state["status_enabled"], false);
    }

    #[test]
    fn test_form_state_lists_builtin_theme_options() {
        let state = form_state(&Config::default());
        let options = state["theme_options"].as_array().expect("theme_options");
        // The three built-ins always lead the list.
        let values: Vec<&str> = options.iter().filter_map(|o| o["value"].as_str()).collect();
        assert_eq!(&values[..3], &["dark", "light", "auto"]);
    }

    #[test]
    fn test_render_html_fills_every_placeholder() {
        let html = render_html(&Config::default(), &Theme::dark());
        assert!(!html.contains("{{"), "unfilled placeholder remains");
        assert!(html.contains(&Theme::dark().background.to_hex()));
        assert!(html.contains("\"theme\""));
    }
}
