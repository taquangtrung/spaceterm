//! KDL configuration parser: theme, fonts, keybindings, and appearance.
//!
//! Configuration lives in `~/.config/spaceterm/` (or `$XDG_CONFIG_HOME/spaceterm`)
//! split across two files:
//! - `settings.kdl` — appearance and behavior (theme, fonts, colors, menu style,
//!   status bar).
//! - `keys.kdl` — keybindings, as top-level mode blocks.
//!
//! A legacy single-file `spaceterm.kdl` (settings + a `keybindings` block) is
//! still read when neither split file is present.
//!
//! `settings.kdl`:
//! ```kdl
//! theme "auto"
//! font "FiraCode Nerd Font"
//! font-size "15"
//! opacity "1.0"
//! menu-style "modern"
//!
//! colors {
//!     background "#2a2f31"
//!     foreground "#d8d8d8"
//!     cursor-bg "#52ad70"
//!     selection-bg "#fffacd"
//!     split "#51554f"
//!     visual-bell "#202020"
//!     ansi "#000000" "#c22727" "#71b312" "#faa213" "#4fa2fa" "#bb67b2" "#21afbf" "#c0c0c0"
//!     brights "#7a7a7a" "#d43f30" "#71b312" "#ebb909" "#5da2eb" "#c97df5" "#04cfe1" "#e1ebfa"
//!     indexed 136 "#af8700"
//! }
//! ```
//!
//! `keys.kdl` (mode blocks at the top level):
//! ```kdl
//! normal {
//!     binding "Ctrl-Space" "toggle_mode"
//!     binding "j" "focus_down"
//! }
//! insert {
//!     binding "Ctrl-Space" "toggle_mode"
//! }
//! // Window management (split / close / focus). The key is one or two chords;
//! // a two-chord binding sets the leader (default `Ctrl-w`). Actions:
//! // split_vertical, split_horizontal, close_pane, close_other_panes,
//! // focus_left, focus_down, focus_up, focus_right.
//! window {
//!     binding "Ctrl-w v" "split_vertical"
//!     binding "Ctrl-w c" "close_pane"
//!     binding "Ctrl-h" "focus_left"
//! }
//! ```

use std::collections::HashMap;
use std::path::PathBuf;

use spaceterm_render::{MenuStyle, Theme, ThemeRgb};

// ========================================================================
// Data Structures
// ========================================================================

#[derive(Clone, Debug)]
pub struct StatusBarIconsConfig {
    pub normal: String,
    pub insert: String,
    pub block: String,
    pub branding: String,
}

impl Default for StatusBarIconsConfig {
    fn default() -> Self {
        Self {
            normal: "\u{e795}".to_string(),    // 
            insert: "\u{f03eb}".to_string(),   // 󰏫
            block: "\u{f0485}".to_string(),    // 󰒅
            branding: "\u{f0697}".to_string(), // 󰚗
        }
    }
}

/// Status bar visibility and content. `enabled` toggles the whole bar (which
/// also frees its reserved row); the `show_*` flags toggle individual elements.
#[derive(Clone, Debug)]
pub struct StatusBarConfig {
    pub enabled: bool,
    pub icons: StatusBarIconsConfig,
    pub show_branding: bool,
    pub show_mode: bool,
    pub show_title: bool,
}

impl Default for StatusBarConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            icons: StatusBarIconsConfig::default(),
            show_branding: true,
            show_mode: true,
            show_title: true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub colors: ColorOverrides,
    pub font_family: Option<String>,
    pub font_size: f32,
    pub keybindings: HashMap<String, HashMap<String, String>>,
    /// Tabbar menu presentation: a modern hamburger dropdown or a classic menubar.
    pub menu_style: MenuStyle,
    pub opacity: f32,
    pub status_bar: StatusBarConfig,
    pub theme: ThemeSetting,
}

/// Per-color overrides parsed from the KDL `colors` block. Applied on top of the
/// active preset ([`Theme::dark`]/[`Theme::light`]); unset colors keep the preset.
#[derive(Clone, Debug, Default)]
pub struct ColorOverrides {
    pub ansi: Vec<ThemeRgb>,
    pub background: Option<ThemeRgb>,
    pub bell: Option<ThemeRgb>,
    pub brights: Vec<ThemeRgb>,
    pub cursor_bg: Option<ThemeRgb>,
    pub cursor_fg: Option<ThemeRgb>,
    pub divider: Option<ThemeRgb>,
    pub foreground: Option<ThemeRgb>,
    pub indexed: Vec<(u8, ThemeRgb)>,
    pub selection_bg: Option<ThemeRgb>,
    pub selection_fg: Option<ThemeRgb>,
}

impl ColorOverrides {
    /// Overwrite the matching fields of `theme` with any colors set here.
    pub fn apply(&self, theme: &mut Theme) {
        if let Some(c) = self.background {
            theme.background = c;
        }
        if let Some(c) = self.foreground {
            theme.foreground = c;
        }
        if let Some(c) = self.cursor_bg {
            theme.cursor_bg = c;
        }
        if let Some(c) = self.cursor_fg {
            theme.cursor_fg = c;
        }
        if let Some(c) = self.selection_bg {
            theme.selection_bg = c;
        }
        if let Some(c) = self.selection_fg {
            theme.selection_fg = c;
        }
        if let Some(c) = self.divider {
            theme.divider = c;
        }
        if let Some(c) = self.bell {
            theme.bell = c;
        }
        for (i, c) in self.ansi.iter().enumerate().take(8) {
            theme.ansi[i] = *c;
        }
        for (i, c) in self.brights.iter().enumerate().take(8) {
            theme.ansi[8 + i] = *c;
        }
        for (idx, c) in &self.indexed {
            if let Some(slot) = theme.indexed.iter_mut().find(|(i, _)| i == idx) {
                slot.1 = *c;
            } else {
                theme.indexed.push((*idx, *c));
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ThemeSetting {
    Auto,
    #[default]
    Dark,
    Light,
}

// ========================================================================
// KDL schema
// ========================================================================

#[derive(knuffel::Decode)]
struct KdlConfig {
    #[knuffel(child)]
    colors: Option<KdlColors>,
    #[knuffel(child)]
    font: Option<KdlFont>,
    #[knuffel(child)]
    font_size: Option<KdlFontSize>,
    #[knuffel(child)]
    keybindings: Option<KdlKeybindings>,
    #[knuffel(child, unwrap(argument))]
    menu_style: Option<String>,
    #[knuffel(child)]
    opacity: Option<KdlOpacity>,
    #[knuffel(child)]
    theme: Option<KdlTheme>,
    #[knuffel(child)]
    status_bar: Option<KdlStatusBar>,
}

#[derive(knuffel::Decode)]
struct KdlStatusBar {
    #[knuffel(child, unwrap(argument))]
    normal_icon: Option<String>,
    #[knuffel(child, unwrap(argument))]
    insert_icon: Option<String>,
    #[knuffel(child, unwrap(argument))]
    block_icon: Option<String>,
    #[knuffel(child, unwrap(argument))]
    branding_icon: Option<String>,
    #[knuffel(child, unwrap(argument))]
    show: Option<String>,
    #[knuffel(child, unwrap(argument))]
    show_mode: Option<String>,
    #[knuffel(child, unwrap(argument))]
    show_title: Option<String>,
    #[knuffel(child, unwrap(argument))]
    show_branding: Option<String>,
}

#[derive(knuffel::Decode)]
struct KdlColors {
    #[knuffel(child, unwrap(argument))]
    background: Option<String>,
    #[knuffel(child, unwrap(argument))]
    foreground: Option<String>,
    #[knuffel(child, unwrap(argument))]
    cursor_bg: Option<String>,
    #[knuffel(child, unwrap(argument))]
    cursor_fg: Option<String>,
    #[knuffel(child, unwrap(argument))]
    selection_bg: Option<String>,
    #[knuffel(child, unwrap(argument))]
    selection_fg: Option<String>,
    #[knuffel(child, unwrap(argument))]
    split: Option<String>,
    #[knuffel(child, unwrap(argument))]
    visual_bell: Option<String>,
    #[knuffel(child)]
    ansi: Option<KdlColorList>,
    #[knuffel(child)]
    brights: Option<KdlColorList>,
    #[knuffel(children(name = "indexed"))]
    indexed: Vec<KdlIndexed>,
}

#[derive(knuffel::Decode)]
struct KdlColorList {
    #[knuffel(arguments)]
    values: Vec<String>,
}

#[derive(knuffel::Decode)]
struct KdlIndexed {
    #[knuffel(argument)]
    index: u8,
    #[knuffel(argument)]
    color: String,
}

#[derive(knuffel::Decode)]
struct KdlFont {
    #[knuffel(argument)]
    value: String,
}

#[derive(knuffel::Decode)]
struct KdlFontSize {
    #[knuffel(argument)]
    value: String,
}

#[derive(knuffel::Decode)]
struct KdlOpacity {
    #[knuffel(argument)]
    value: String,
}

#[derive(knuffel::Decode)]
struct KdlTheme {
    #[knuffel(argument)]
    value: String,
}

#[derive(knuffel::Decode)]
struct KdlKeybindings {
    #[knuffel(children)]
    modes: Vec<KdlModeBindings>,
}

/// The standalone `keys.kdl` schema: mode blocks at the top level (no wrapping
/// `keybindings` node), e.g. `normal { binding "j" "focus_down" }`.
#[derive(knuffel::Decode)]
struct KdlKeys {
    #[knuffel(children)]
    modes: Vec<KdlModeBindings>,
}

#[derive(knuffel::Decode)]
struct KdlModeBindings {
    #[knuffel(node_name)]
    name: String,
    #[knuffel(children)]
    bindings: Vec<KdlBinding>,
}

#[derive(knuffel::Decode)]
struct KdlBinding {
    #[knuffel(argument)]
    key: String,
    #[knuffel(argument)]
    action: String,
}

// ========================================================================
// Implementation
// ========================================================================

impl Config {
    /// Load configuration from `~/.config/spaceterm/`. Settings come from
    /// `settings.kdl` and keybindings from `keys.kdl`. If neither exists, fall
    /// back to the legacy single-file `spaceterm.kdl`; failing that, defaults.
    pub fn load() -> Self {
        let dir = config_dir();
        let settings_path = dir.join("settings.kdl");
        let keys_path = dir.join("keys.kdl");

        if settings_path.exists() || keys_path.exists() {
            let settings = std::fs::read_to_string(&settings_path).unwrap_or_default();
            let keys = std::fs::read_to_string(&keys_path).unwrap_or_default();
            return Self::parse_with_keys(&settings, &keys);
        }

        let legacy = dir.join("spaceterm.kdl");
        if legacy.exists() {
            Self::load_from(&legacy)
        } else {
            Self::default()
        }
    }

    pub fn load_from(path: &PathBuf) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => return Self::default(),
        };
        Self::parse(&text)
    }

    /// Parse settings (`settings.kdl`) and keybindings (`keys.kdl`) from separate
    /// sources. Keybindings in `keys` replace any present in `settings`.
    pub fn parse_with_keys(settings: &str, keys: &str) -> Self {
        let mut config = Self::parse(settings);
        let parsed = parse_keys(keys);
        if !parsed.is_empty() {
            config.keybindings = parsed;
        }
        config
    }

    pub fn parse(text: &str) -> Self {
        let kdl: KdlConfig = match knuffel::parse("spaceterm.kdl", text) {
            Ok(c) => c,
            Err(_) => return Self::default(),
        };

        let theme = kdl
            .theme
            .as_ref()
            .map(|t| match t.value.as_str() {
                "auto" => ThemeSetting::Auto,
                "light" => ThemeSetting::Light,
                _ => ThemeSetting::Dark,
            })
            .unwrap_or_default();

        let keybindings = kdl
            .keybindings
            .map(|kb| {
                kb.modes
                    .into_iter()
                    .map(|m| {
                        let bindings = m.bindings.into_iter().map(|b| (b.key, b.action)).collect();
                        (m.name, bindings)
                    })
                    .collect()
            })
            .unwrap_or_default();

        let status_bar = kdl
            .status_bar
            .map(|sb| {
                let defaults = StatusBarIconsConfig::default();
                StatusBarConfig {
                    enabled: parse_bool(sb.show.as_deref(), true),
                    icons: StatusBarIconsConfig {
                        normal: sb.normal_icon.unwrap_or(defaults.normal),
                        insert: sb.insert_icon.unwrap_or(defaults.insert),
                        block: sb.block_icon.unwrap_or(defaults.block),
                        branding: sb.branding_icon.unwrap_or(defaults.branding),
                    },
                    show_branding: parse_bool(sb.show_branding.as_deref(), true),
                    show_mode: parse_bool(sb.show_mode.as_deref(), true),
                    show_title: parse_bool(sb.show_title.as_deref(), true),
                }
            })
            .unwrap_or_default();

        let menu_style = kdl
            .menu_style
            .as_deref()
            .map(|s| match s {
                "classic" => MenuStyle::Classic,
                _ => MenuStyle::Modern,
            })
            .unwrap_or_default();

        Config {
            colors: kdl.colors.map(color_overrides_from_kdl).unwrap_or_default(),
            font_family: kdl.font.map(|f| f.value).filter(|s| !s.trim().is_empty()),
            font_size: kdl
                .font_size
                .as_ref()
                .and_then(|f| f.value.parse().ok())
                .unwrap_or(15.0),
            keybindings,
            menu_style,
            opacity: kdl
                .opacity
                .as_ref()
                .and_then(|o| o.value.parse().ok())
                .unwrap_or(1.0f32)
                .clamp(0.1, 1.0),
            status_bar,
            theme,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            colors: ColorOverrides::default(),
            font_family: None,
            font_size: 15.0,
            keybindings: HashMap::new(),
            menu_style: MenuStyle::default(),
            opacity: 1.0,
            status_bar: StatusBarConfig::default(),
            theme: ThemeSetting::default(),
        }
    }
}

/// Parse a KDL boolean-ish string (`"true"`/`"false"`), falling back to `default`.
fn parse_bool(value: Option<&str>, default: bool) -> bool {
    match value {
        Some(v) => matches!(v.trim().to_ascii_lowercase().as_str(), "true" | "1" | "yes" | "on"),
        None => default,
    }
}

fn color_overrides_from_kdl(kdl: KdlColors) -> ColorOverrides {
    fn hex(s: Option<String>) -> Option<ThemeRgb> {
        s.and_then(|v| ThemeRgb::parse_hex(&v))
    }
    fn hex_list(list: Option<KdlColorList>) -> Vec<ThemeRgb> {
        list.map(|l| {
            l.values
                .iter()
                .filter_map(|s| ThemeRgb::parse_hex(s))
                .collect()
        })
        .unwrap_or_default()
    }

    ColorOverrides {
        background: hex(kdl.background),
        foreground: hex(kdl.foreground),
        cursor_bg: hex(kdl.cursor_bg),
        cursor_fg: hex(kdl.cursor_fg),
        selection_bg: hex(kdl.selection_bg),
        selection_fg: hex(kdl.selection_fg),
        divider: hex(kdl.split),
        bell: hex(kdl.visual_bell),
        ansi: hex_list(kdl.ansi),
        brights: hex_list(kdl.brights),
        indexed: kdl
            .indexed
            .into_iter()
            .filter_map(|e| ThemeRgb::parse_hex(&e.color).map(|c| (e.index, c)))
            .collect(),
    }
}

/// Parse a standalone `keys.kdl` into the mode -> (key -> action) map. An empty
/// or unparseable file yields an empty map (callers keep their defaults).
fn parse_keys(text: &str) -> HashMap<String, HashMap<String, String>> {
    if text.trim().is_empty() {
        return HashMap::new();
    }
    match knuffel::parse::<KdlKeys>("keys.kdl", text) {
        Ok(keys) => keys
            .modes
            .into_iter()
            .map(|m| {
                let bindings = m.bindings.into_iter().map(|b| (b.key, b.action)).collect();
                (m.name, bindings)
            })
            .collect(),
        Err(_) => HashMap::new(),
    }
}

/// The configuration directory: `$XDG_CONFIG_HOME/spaceterm` or
/// `~/.config/spaceterm`.
fn config_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        PathBuf::from(xdg).join("spaceterm")
    } else if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".config/spaceterm")
    } else {
        PathBuf::from(".")
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.font_size, 15.0);
        assert_eq!(config.opacity, 1.0);
        assert_eq!(config.theme, ThemeSetting::Dark);
        assert!(config.keybindings.is_empty());
    }

    #[test]
    fn test_parse_minimal_config() {
        let config = Config::parse("font-size \"18\"\nopacity \"0.9\"");
        assert_eq!(config.font_size, 18.0);
        assert_eq!(config.opacity, 0.9);
    }

    #[test]
    fn test_parse_font_family() {
        let config = Config::parse("font \"FiraCode Nerd Font\"");
        assert_eq!(config.font_family.as_deref(), Some("FiraCode Nerd Font"));
    }

    #[test]
    fn test_default_font_family_is_none() {
        assert_eq!(Config::default().font_family, None);
        assert_eq!(Config::parse("font-size \"12\"").font_family, None);
    }

    #[test]
    fn test_parse_colors_block() {
        let config = Config::parse(
            r##"
colors {
    background "#1a1a2e"
    foreground "#e0e0e0"
    split "#333333"
    ansi "#000000" "#c22727" "#71b312" "#faa213" "#4fa2fa" "#bb67b2" "#21afbf" "#c0c0c0"
    indexed 136 "#af8700"
}
"##,
        );
        let mut theme = Theme::dark();
        config.colors.apply(&mut theme);
        assert_eq!(theme.background, ThemeRgb::parse_hex("#1a1a2e").unwrap());
        assert_eq!(theme.foreground, ThemeRgb::parse_hex("#e0e0e0").unwrap());
        assert_eq!(theme.divider, ThemeRgb::parse_hex("#333333").unwrap());
        assert_eq!(theme.ansi[1], ThemeRgb::parse_hex("#c22727").unwrap());
        assert_eq!(theme.indexed_color(136), Some((175, 135, 0)));
        // cursor was not overridden, keeps the preset.
        assert_eq!(theme.cursor_bg, Theme::dark().cursor_bg);
    }

    #[test]
    fn test_no_colors_block_leaves_preset_untouched() {
        let config = Config::parse("theme \"dark\"");
        let mut theme = Theme::dark();
        config.colors.apply(&mut theme);
        assert_eq!(theme, Theme::dark());
    }

    #[test]
    fn test_parse_theme_auto() {
        let config = Config::parse("theme \"auto\"");
        assert_eq!(config.theme, ThemeSetting::Auto);
    }

    #[test]
    fn test_parse_theme_light() {
        let config = Config::parse("theme \"light\"");
        assert_eq!(config.theme, ThemeSetting::Light);
    }

    #[test]
    fn test_parse_keybindings() {
        let config = Config::parse(
            r#"
keybindings {
    normal {
        binding "j" "focus_down"
        binding "k" "focus_up"
    }
    insert {
        binding "Ctrl-Space" "toggle_mode"
    }
}
"#,
        );
        assert_eq!(config.keybindings.len(), 2);
        let normal = config.keybindings.get("normal").unwrap();
        assert_eq!(normal.get("j"), Some(&"focus_down".to_string()));
        assert_eq!(normal.get("k"), Some(&"focus_up".to_string()));
        let insert = config.keybindings.get("insert").unwrap();
        assert_eq!(insert.get("Ctrl-Space"), Some(&"toggle_mode".to_string()));
    }

    #[test]
    fn test_opacity_clamped() {
        let config = Config::parse("opacity \"0.05\"");
        assert_eq!(config.opacity, 0.1);
    }

    #[test]
    fn test_parse_invalid_returns_default() {
        let config = Config::parse("this is not valid kdl {{{{");
        assert_eq!(config.font_size, 15.0);
    }

    #[test]
    fn test_parse_with_keys_merges_separate_files() {
        let settings = "theme \"light\"\nmenu-style \"classic\"";
        let keys = r#"
normal {
    binding "j" "focus_down"
}
window {
    binding "Ctrl-w v" "split_vertical"
}
"#;
        let config = Config::parse_with_keys(settings, keys);
        // Settings come from settings.kdl.
        assert_eq!(config.theme, ThemeSetting::Light);
        assert_eq!(config.menu_style, MenuStyle::Classic);
        // Keybindings come from keys.kdl (top-level mode blocks).
        assert_eq!(
            config.keybindings.get("normal").and_then(|m| m.get("j")),
            Some(&"focus_down".to_string())
        );
        assert_eq!(
            config
                .keybindings
                .get("window")
                .and_then(|m| m.get("Ctrl-w v")),
            Some(&"split_vertical".to_string())
        );
    }

    #[test]
    fn test_parse_with_empty_keys_keeps_no_bindings() {
        let config = Config::parse_with_keys("font-size \"12\"", "");
        assert!(config.keybindings.is_empty());
    }

    #[test]
    fn test_menu_style_defaults_to_modern_and_parses_classic() {
        assert_eq!(Config::default().menu_style, MenuStyle::Modern);
        assert_eq!(
            Config::parse("font-size \"12\"").menu_style,
            MenuStyle::Modern
        );
        assert_eq!(
            Config::parse("menu-style \"classic\"").menu_style,
            MenuStyle::Classic
        );
        // An unrecognized value falls back to the modern default.
        assert_eq!(
            Config::parse("menu-style \"fancy\"").menu_style,
            MenuStyle::Modern
        );
    }

    #[test]
    fn test_parse_status_bar_icons() {
        let config = Config::parse(
            r#"
status-bar {
    normal-icon "N"
    insert-icon "I"
    block-icon "B"
    branding-icon "S"
}
"#,
        );
        assert_eq!(config.status_bar.icons.normal, "N");
        assert_eq!(config.status_bar.icons.insert, "I");
        assert_eq!(config.status_bar.icons.block, "B");
        assert_eq!(config.status_bar.icons.branding, "S");
    }

    #[test]
    fn test_status_bar_visibility_defaults_on_and_parses_off() {
        let default = Config::default().status_bar;
        assert!(default.enabled && default.show_mode && default.show_title && default.show_branding);

        let config = Config::parse(
            r#"
status-bar {
    show "false"
    show-branding "false"
}
"#,
        );
        assert!(!config.status_bar.enabled);
        assert!(!config.status_bar.show_branding);
        // Unspecified element flags keep their default (shown).
        assert!(config.status_bar.show_mode);
        assert!(config.status_bar.show_title);
    }
}
