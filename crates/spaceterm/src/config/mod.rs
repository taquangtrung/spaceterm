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
//! User themes live in `themes/<name>.kdl`; set `theme "<name>"` to select one
//! (the reserved words `dark`/`light`/`auto` stay built-in). A theme file is an
//! optional `base "dark"|"light"` plus a `colors` block layered over it:
//! ```kdl
//! base "dark"
//! colors {
//!     background "#282a36"
//!     foreground "#f8f8f2"
//! }
//! ```
//!
//! `settings.kdl`:
//! ```kdl
//! theme "auto"
//! font "FiraCode Nerd Font"
//! font-size "15"
//! opacity "1.0"
//! menu-style "modern"
//! window-controls-side "right"
//! cursor {
//!     insert "bar"
//!     normal "block"
//!     visual "block"
//!     block-focus "bar"
//! }
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

use spaceterm_render::{ControlsSide, CursorShape, MenuStyle, Theme, ThemeRgb};

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
    pub cursor: CursorConfig,
    pub font_family: Option<String>,
    pub font_size: f32,
    pub font_weight: Option<String>,
    pub font_weight_bold: Option<String>,
    pub keybindings: HashMap<String, HashMap<String, String>>,
    /// Enable OpenType ligatures in the font renderer. Defaults to `true`.
    pub ligatures: bool,
    /// Tabbar menu presentation: a modern hamburger dropdown or a classic menubar.
    pub menu_style: MenuStyle,
    pub opacity: f32,
    /// Draw an underline under fuzzy-matched characters in the command palette.
    pub palette_match_underline: bool,
    /// Right-click pastes from the clipboard instead of opening the context menu.
    pub paste_on_right_click: bool,
    /// Maximum scrollback rows per pane. `None` uses the compiled-in default (10 000).
    pub scrollback_lines: Option<usize>,
    /// Path or name of the shell to launch in new panes. Overrides `SHELL` and
    /// `SPACETERM_SHELL`. `None` falls back to the environment-variable chain.
    pub shell: Option<String>,
    pub status_bar: StatusBarConfig,
    pub theme: ThemeSetting,
    pub title_bar_style: TitleBarStyle,
    /// Which edge carries the minimize/maximize/close buttons (the hamburger
    /// button sits on the opposite edge).
    pub window_controls_side: ControlsSide,
}

/// Per-mode cursor shapes, parsed from the `cursor` block. Each mode renders
/// its cursor with the configured shape; defaults follow the common terminal
/// convention (a bar for insert-like modes, a block for navigation).
#[derive(Clone, Copy, Debug)]
pub struct CursorConfig {
    /// Whether the cursor blinks. Applies only to the focused pane; non-focused
    /// panes always show a static cursor.
    pub blink: bool,
    pub block_focus: CursorShape,
    pub insert: CursorShape,
    pub normal: CursorShape,
    pub visual: CursorShape,
}

impl Default for CursorConfig {
    fn default() -> Self {
        Self {
            blink: true,
            block_focus: CursorShape::Bar,
            insert: CursorShape::Bar,
            normal: CursorShape::Block,
            visual: CursorShape::Block,
        }
    }
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
    pub status_bar_border: Option<ThemeRgb>,
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
        if let Some(c) = self.status_bar_border {
            theme.status_bar_border = c;
        }
        if let Some(c) = self.bell {
            theme.bell = Some(c);
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

/// Window title-bar presentation: the OS-drawn decorations, or a borderless
/// window whose tab strip doubles as a VS Code-style title bar carrying its own
/// minimize/maximize/close controls.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TitleBarStyle {
    /// Native OS window decorations; the tab strip sits inside, below them.
    System,
    /// Borderless: the tab strip is the title bar, with custom window controls.
    #[default]
    Modern,
}

impl TitleBarStyle {
    /// Interpret a `title-bar-style` config value; unknown values fall back to the
    /// modern default. `native` is accepted as an alias for `system`.
    pub fn from_value(value: &str) -> Self {
        match value {
            "system" | "native" => Self::System,
            _ => Self::Modern,
        }
    }

    /// The `title-bar-style` config value this selection serializes to.
    pub fn as_value(&self) -> &str {
        match self {
            Self::System => "system",
            Self::Modern => "modern",
        }
    }
}

/// The selected theme: a built-in preset, the system-following `Auto`, or a
/// user theme file `themes/<name>.kdl` referenced by name.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ThemeSetting {
    Auto,
    #[default]
    Dark,
    Light,
    Named(String),
}

impl ThemeSetting {
    /// Interpret a `theme` config value: the reserved words `auto`/`dark`/`light`
    /// are built-ins, anything else names a theme file in `themes/`.
    pub fn from_value(value: &str) -> Self {
        match value {
            "auto" => Self::Auto,
            "light" => Self::Light,
            "dark" => Self::Dark,
            other => Self::Named(other.to_string()),
        }
    }

    /// The `theme` config value this selection serializes to.
    pub fn as_value(&self) -> &str {
        match self {
            Self::Auto => "auto",
            Self::Dark => "dark",
            Self::Light => "light",
            Self::Named(name) => name,
        }
    }
}

// ========================================================================
// KDL schema
// ========================================================================

#[derive(knuffel::Decode)]
struct KdlConfig {
    #[knuffel(child)]
    colors: Option<KdlColors>,
    #[knuffel(child)]
    cursor: Option<KdlCursor>,
    #[knuffel(child)]
    font: Option<KdlFont>,
    #[knuffel(child)]
    font_size: Option<KdlFontSize>,
    #[knuffel(child)]
    font_weight: Option<KdlFontWeight>,
    #[knuffel(child)]
    font_weight_bold: Option<KdlFontWeightBold>,
    #[knuffel(child)]
    keybindings: Option<KdlKeybindings>,
    #[knuffel(child, unwrap(argument))]
    menu_style: Option<String>,
    #[knuffel(child, unwrap(argument))]
    scrollback_lines: Option<String>,
    #[knuffel(child, unwrap(argument))]
    shell: Option<String>,
    #[knuffel(child, unwrap(argument))]
    title_bar_style: Option<String>,
    #[knuffel(child, unwrap(argument))]
    ligatures: Option<String>,
    #[knuffel(child, unwrap(argument))]
    palette_match_underline: Option<String>,
    #[knuffel(child, unwrap(argument))]
    paste_on_right_click: Option<String>,
    #[knuffel(child, unwrap(argument))]
    window_controls_side: Option<String>,
    #[knuffel(child)]
    opacity: Option<KdlOpacity>,
    #[knuffel(child)]
    theme: Option<KdlTheme>,
    #[knuffel(child)]
    status_bar: Option<KdlStatusBar>,
}

#[derive(knuffel::Decode)]
struct KdlCursor {
    #[knuffel(child, unwrap(argument))]
    blink: Option<String>,
    #[knuffel(child, unwrap(argument))]
    block_focus: Option<String>,
    #[knuffel(child, unwrap(argument))]
    insert: Option<String>,
    #[knuffel(child, unwrap(argument))]
    normal: Option<String>,
    #[knuffel(child, unwrap(argument))]
    visual: Option<String>,
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
    status_bar_border: Option<String>,
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
struct KdlFontWeight {
    #[knuffel(argument)]
    value: String,
}

#[derive(knuffel::Decode)]
struct KdlFontWeightBold {
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

/// A `themes/<name>.kdl` file: an optional `base` preset (`dark`/`light`) plus a
/// `colors` block layered over it, reusing the same color schema as the main
/// config.
#[derive(knuffel::Decode)]
struct KdlThemeFile {
    #[knuffel(child, unwrap(argument))]
    base: Option<String>,
    #[knuffel(child)]
    colors: Option<KdlColors>,
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
        let kdl: KdlConfig = match knuffel::parse("settings.kdl", text) {
            Ok(c) => c,
            Err(e) => {
                let problems = config_problems(&e, text);
                eprintln!(
                    "spaceterm: ignoring settings.kdl ({} problem(s)); using built-in defaults until fixed:",
                    problems.len()
                );
                for problem in &problems {
                    eprintln!("  line {}: {}", problem.line, problem.message);
                }
                return Self::default();
            }
        };

        let theme = kdl
            .theme
            .as_ref()
            .map(|t| ThemeSetting::from_value(&t.value))
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

        let title_bar_style = kdl
            .title_bar_style
            .as_deref()
            .map(TitleBarStyle::from_value)
            .unwrap_or_default();

        let window_controls_side = kdl
            .window_controls_side
            .as_deref()
            .map(controls_side_from_value)
            .unwrap_or_default();

        Config {
            colors: kdl.colors.map(color_overrides_from_kdl).unwrap_or_default(),
            cursor: kdl.cursor.map(cursor_config_from_kdl).unwrap_or_default(),
            font_family: kdl.font.map(|f| f.value).filter(|s| !s.trim().is_empty()),
            font_size: kdl
                .font_size
                .as_ref()
                .and_then(|f| f.value.parse().ok())
                .unwrap_or(15.0),
            font_weight: kdl
                .font_weight
                .map(|w| w.value)
                .filter(|s| !s.trim().is_empty()),
            font_weight_bold: kdl
                .font_weight_bold
                .map(|w| w.value)
                .filter(|s| !s.trim().is_empty()),
            keybindings,
            menu_style,
            opacity: kdl
                .opacity
                .as_ref()
                .and_then(|o| o.value.parse().ok())
                .unwrap_or(1.0f32)
                .clamp(0.1, 1.0),
            ligatures: parse_bool(kdl.ligatures.as_deref(), true),
            palette_match_underline: parse_bool(kdl.palette_match_underline.as_deref(), false),
            paste_on_right_click: parse_bool(kdl.paste_on_right_click.as_deref(), false),
            scrollback_lines: kdl.scrollback_lines.as_deref().and_then(|s| s.parse::<usize>().ok()).filter(|&n| n > 0),
            shell: kdl.shell.filter(|s| !s.trim().is_empty()),
            status_bar,
            theme,
            title_bar_style,
            window_controls_side,
        }
    }

    /// Serialize the appearance/behavior settings as `settings.kdl` text. The
    /// output round-trips through [`Self::parse`]. Keybindings live in a separate
    /// `keys.kdl` and are intentionally not written here.
    pub fn to_kdl(&self) -> String {
        let menu_style = match self.menu_style {
            MenuStyle::Classic => "classic",
            MenuStyle::Modern => "modern",
        };

        let mut out = String::new();
        out.push_str(&format!("theme {}\n", kdl_string(self.theme.as_value())));
        if let Some(n) = self.scrollback_lines {
            out.push_str(&format!("scrollback-lines \"{n}\"\n"));
        }
        if let Some(shell) = &self.shell {
            out.push_str(&format!("shell {}\n", kdl_string(shell)));
        }
        if let Some(font) = &self.font_family {
            out.push_str(&format!("font {}\n", kdl_string(font)));
        }
        out.push_str(&format!(
            "font-size {}\n",
            kdl_string(&self.font_size.to_string())
        ));
        if let Some(font_weight) = &self.font_weight {
            out.push_str(&format!("font-weight {}\n", kdl_string(font_weight)));
        }
        if let Some(bold_weight) = &self.font_weight_bold {
            out.push_str(&format!("font-weight-bold {}\n", kdl_string(bold_weight)));
        }
        out.push_str(&format!(
            "opacity {}\n",
            kdl_string(&self.opacity.to_string())
        ));
        out.push_str(&format!("menu-style {}\n", kdl_string(menu_style)));
        out.push_str(&format!(
            "palette-match-underline {}\n",
            kdl_bool(self.palette_match_underline)
        ));
        out.push_str(&format!(
            "ligatures {}\n",
            kdl_bool(self.ligatures)
        ));
        out.push_str(&format!(
            "paste-on-right-click {}\n",
            kdl_bool(self.paste_on_right_click)
        ));
        out.push_str(&format!(
            "title-bar-style {}\n",
            kdl_string(self.title_bar_style.as_value())
        ));
        out.push_str(&format!(
            "window-controls-side {}\n",
            kdl_string(controls_side_as_value(self.window_controls_side))
        ));
        out.push_str(&self.cursor_kdl());
        out.push_str(&self.status_bar_kdl());
        if let Some(colors) = self.colors_kdl() {
            out.push_str(&colors);
        }
        out
    }

    /// Write [`Self::to_kdl`] to `settings.kdl`, creating the config directory if
    /// needed. Keybindings are left untouched.
    pub fn save(&self) -> std::io::Result<()> {
        let dir = config_dir();
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("settings.kdl"), self.to_kdl())
    }

    /// The `cursor` block, always emitted so every per-mode shape round-trips.
    fn cursor_kdl(&self) -> String {
        let c = &self.cursor;
        let mut out = String::from("cursor {\n");
        out.push_str(&format!("    blink {}\n", kdl_bool(c.blink)));
        out.push_str(&format!(
            "    insert {}\n",
            kdl_string(c.insert.as_value())
        ));
        out.push_str(&format!(
            "    normal {}\n",
            kdl_string(c.normal.as_value())
        ));
        out.push_str(&format!(
            "    visual {}\n",
            kdl_string(c.visual.as_value())
        ));
        out.push_str(&format!(
            "    block-focus {}\n",
            kdl_string(c.block_focus.as_value())
        ));
        out.push_str("}\n");
        out
    }

    /// The `status-bar` block, always emitted so every flag and icon round-trips.
    fn status_bar_kdl(&self) -> String {
        let sb = &self.status_bar;
        let mut out = String::from("status-bar {\n");
        out.push_str(&format!("    show {}\n", kdl_bool(sb.enabled)));
        out.push_str(&format!("    show-mode {}\n", kdl_bool(sb.show_mode)));
        out.push_str(&format!("    show-title {}\n", kdl_bool(sb.show_title)));
        out.push_str(&format!(
            "    show-branding {}\n",
            kdl_bool(sb.show_branding)
        ));
        out.push_str(&format!(
            "    normal-icon {}\n",
            kdl_string(&sb.icons.normal)
        ));
        out.push_str(&format!(
            "    insert-icon {}\n",
            kdl_string(&sb.icons.insert)
        ));
        out.push_str(&format!("    block-icon {}\n", kdl_string(&sb.icons.block)));
        out.push_str(&format!(
            "    branding-icon {}\n",
            kdl_string(&sb.icons.branding)
        ));
        out.push_str("}\n");
        out
    }

    /// The `colors` block, or `None` when no color override is set (so an
    /// untouched config keeps the preset and writes no block).
    fn colors_kdl(&self) -> Option<String> {
        let c = &self.colors;
        let scalars: [(&str, Option<ThemeRgb>); 9] = [
            ("background", c.background),
            ("foreground", c.foreground),
            ("cursor-bg", c.cursor_bg),
            ("cursor-fg", c.cursor_fg),
            ("selection-bg", c.selection_bg),
            ("selection-fg", c.selection_fg),
            ("split", c.divider),
            ("status-bar-border", c.status_bar_border),
            ("visual-bell", c.bell),
        ];
        let any = scalars.iter().any(|(_, v)| v.is_some())
            || !c.ansi.is_empty()
            || !c.brights.is_empty()
            || !c.indexed.is_empty();
        if !any {
            return None;
        }

        let mut out = String::from("colors {\n");
        for (name, value) in scalars {
            if let Some(rgb) = value {
                out.push_str(&format!("    {name} {}\n", kdl_string(&rgb.to_hex())));
            }
        }
        if let Some(line) = color_list_kdl("ansi", &c.ansi) {
            out.push_str(&line);
        }
        if let Some(line) = color_list_kdl("brights", &c.brights) {
            out.push_str(&line);
        }
        for (index, rgb) in &c.indexed {
            out.push_str(&format!(
                "    indexed {index} {}\n",
                kdl_string(&rgb.to_hex())
            ));
        }
        out.push_str("}\n");
        Some(out)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            colors: ColorOverrides::default(),
            cursor: CursorConfig::default(),
            font_family: None,
            font_size: 15.0,
            font_weight: None,
            font_weight_bold: None,
            keybindings: HashMap::new(),
            ligatures: true,
            menu_style: MenuStyle::default(),
            opacity: 1.0,
            palette_match_underline: false,
            paste_on_right_click: false,
            scrollback_lines: None,
            shell: None,
            status_bar: StatusBarConfig::default(),
            theme: ThemeSetting::default(),
            title_bar_style: TitleBarStyle::default(),
            window_controls_side: ControlsSide::default(),
        }
    }
}

/// Names (filename without `.kdl`) of the theme files in `themes/`, sorted. An
/// absent or unreadable directory yields an empty list.
pub fn available_themes() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(themes_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension()?.to_str()? != "kdl" {
                return None;
            }
            path.file_stem()?.to_str().map(str::to_string)
        })
        .collect();
    names.sort();
    names
}

/// Load and resolve `themes/<name>.kdl` into a full [`Theme`] (its `base` preset
/// with its `colors` layered on top), or `None` if the file is missing or
/// unparseable.
pub fn load_named_theme(name: &str) -> Option<Theme> {
    let path = themes_dir().join(format!("{name}.kdl"));
    let text = std::fs::read_to_string(path).ok()?;
    parse_theme_file(&text)
}

/// Resolve a theme file's text into a [`Theme`]: start from the named `base`
/// preset (defaulting to dark) and apply its color overrides.
fn parse_theme_file(text: &str) -> Option<Theme> {
    let kdl: KdlThemeFile = knuffel::parse("theme.kdl", text).ok()?;
    let mut theme = match kdl.base.as_deref() {
        Some("light") => Theme::light(),
        _ => Theme::dark(),
    };
    if let Some(colors) = kdl.colors {
        color_overrides_from_kdl(colors).apply(&mut theme);
    }
    Some(theme)
}

/// The user theme directory: `<config_dir>/themes`.
fn themes_dir() -> PathBuf {
    config_dir().join("themes")
}

/// Paths of all config files that trigger a reload when modified. Callers can
/// poll their modification times to implement hot-reload.
pub fn config_file_paths() -> Vec<PathBuf> {
    let dir = config_dir();
    vec![
        dir.join("settings.kdl"),
        dir.join("keys.kdl"),
        dir.join("spaceterm.kdl"),
    ]
}

/// Load the last saved window dimensions from `<config_dir>/window-size`.
/// Returns `None` if the file is missing or cannot be parsed.
pub fn load_window_size() -> Option<(u32, u32)> {
    let text = std::fs::read_to_string(config_dir().join("window-size")).ok()?;
    let (w, h) = text.trim().split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

/// Persist window dimensions to `<config_dir>/window-size`.
pub fn save_window_size(width: u32, height: u32) {
    let dir = config_dir();
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::write(dir.join("window-size"), format!("{width}x{height}"));
    }
}

/// Parse a KDL boolean-ish string (`"true"`/`"false"`), falling back to `default`.
fn parse_bool(value: Option<&str>, default: bool) -> bool {
    match value {
        Some(v) => matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "true" | "1" | "yes" | "on"
        ),
        None => default,
    }
}

/// Quote `s` as a KDL string argument, escaping backslashes and double quotes.
fn kdl_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A boolean as the `"true"`/`"false"` KDL string read back by [`parse_bool`].
fn kdl_bool(value: bool) -> String {
    kdl_string(if value { "true" } else { "false" })
}

/// A `colors` list node (`ansi`/`brights`) of hex string arguments, or `None`
/// when the list is empty.
fn color_list_kdl(name: &str, colors: &[ThemeRgb]) -> Option<String> {
    if colors.is_empty() {
        return None;
    }
    let args: Vec<String> = colors.iter().map(|c| kdl_string(&c.to_hex())).collect();
    Some(format!("    {name} {}\n", args.join(" ")))
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
        status_bar_border: hex(kdl.status_bar_border),
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

/// Interpret a `window-controls-side` config value: `"left"` (the default)
/// or `"right"`. Unknown values fall back to the default left side.
fn controls_side_from_value(value: &str) -> ControlsSide {
    match value.trim().to_ascii_lowercase().as_str() {
        "right" => ControlsSide::Right,
        _ => ControlsSide::Left,
    }
}

/// The canonical config value for a [`ControlsSide`] (round-trips through
/// [`controls_side_from_value`]).
fn controls_side_as_value(side: ControlsSide) -> &'static str {
    match side {
        ControlsSide::Left => "left",
        ControlsSide::Right => "right",
    }
}

/// Apply the `cursor` block on top of the default per-mode shapes; any unset
/// entry keeps its default. Unknown shape strings fall back to `Block` via
/// [`CursorShape::from_value`].
fn cursor_config_from_kdl(kdl: KdlCursor) -> CursorConfig {
    let defaults = CursorConfig::default();
    CursorConfig {
        blink: parse_bool(kdl.blink.as_deref(), defaults.blink),
        block_focus: kdl
            .block_focus
            .as_deref()
            .map(CursorShape::from_value)
            .unwrap_or(defaults.block_focus),
        insert: kdl
            .insert
            .as_deref()
            .map(CursorShape::from_value)
            .unwrap_or(defaults.insert),
        normal: kdl
            .normal
            .as_deref()
            .map(CursorShape::from_value)
            .unwrap_or(defaults.normal),
        visual: kdl
            .visual
            .as_deref()
            .map(CursorShape::from_value)
            .unwrap_or(defaults.visual),
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

/// One problem found in a config file: its 1-based `line` and a human `message`.
struct ConfigProblem {
    line: usize,
    message: String,
}

/// Pull the individual problems out of a knuffel parse error so each can be
/// reported on its own line. knuffel collects every issue (an unknown key, a
/// duplicate key, a bad value) as a "related" diagnostic, but its own top-level
/// message collapses to "error parsing KDL"; this surfaces the details, located
/// against `text` so we can show the line each one is on.
fn config_problems(err: &knuffel::Error, text: &str) -> Vec<ConfigProblem> {
    use miette::Diagnostic;

    let related: Vec<&dyn Diagnostic> = err
        .related()
        .map(|problems| problems.collect())
        .unwrap_or_default();

    // No related diagnostics means the file failed to tokenize at all; report the
    // whole error as a single problem rather than dropping it silently.
    if related.is_empty() {
        return vec![ConfigProblem {
            line: 1,
            message: err.to_string(),
        }];
    }

    related
        .into_iter()
        .map(|problem| {
            let offset = problem
                .labels()
                .and_then(|mut labels| labels.next())
                .map(|label| label.offset())
                .unwrap_or(0);
            ConfigProblem {
                line: line_number(text, offset),
                message: problem.to_string(),
            }
        })
        .collect()
}

/// The 1-based line number containing byte `offset` within `text`.
fn line_number(text: &str, offset: usize) -> usize {
    let end = offset.min(text.len());
    text.as_bytes()[..end]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
        + 1
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
    fn test_config_problems_report_unknown_and_duplicate_keys() {
        let text = "theme \"dark\"\nfont-weigth \"300\"\ntheme \"light\"\n";
        let err = knuffel::parse::<KdlConfig>("settings.kdl", text)
            .err()
            .expect("unknown and duplicate keys should fail to parse");
        let problems = config_problems(&err, text);

        let unknown = problems
            .iter()
            .find(|p| p.message.contains("font-weigth"))
            .expect("unknown key reported");
        assert_eq!(unknown.line, 2);

        let duplicate = problems
            .iter()
            .find(|p| p.message.contains("duplicate") && p.message.contains("theme"))
            .expect("duplicate key reported");
        assert_eq!(duplicate.line, 3);
    }

    #[test]
    fn test_valid_config_has_no_problems() {
        // A clean parse never reaches the problem reporter.
        assert!(knuffel::parse::<KdlConfig>("settings.kdl", "theme \"dark\"\n").is_ok());
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
    fn test_parse_scrollback_lines() {
        let config = Config::parse("scrollback-lines \"50000\"");
        assert_eq!(config.scrollback_lines, Some(50_000));
    }

    #[test]
    fn test_scrollback_lines_zero_is_ignored() {
        let config = Config::parse("scrollback-lines \"0\"");
        assert_eq!(config.scrollback_lines, None);
    }

    #[test]
    fn test_to_kdl_roundtrips_scrollback_lines() {
        let mut config = Config::default();
        config.scrollback_lines = Some(20_000);
        let parsed = Config::parse(&config.to_kdl());
        assert_eq!(parsed.scrollback_lines, Some(20_000));
    }

    #[test]
    fn test_parse_shell() {
        let config = Config::parse("shell \"/usr/bin/fish\"");
        assert_eq!(config.shell.as_deref(), Some("/usr/bin/fish"));
    }

    #[test]
    fn test_default_shell_is_none() {
        assert_eq!(Config::default().shell, None);
        assert_eq!(Config::parse("font-size \"12\"").shell, None);
    }

    #[test]
    fn test_shell_whitespace_ignored() {
        let config = Config::parse("shell \"   \"");
        assert_eq!(config.shell, None);
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
    fn test_title_bar_style_defaults_modern_and_roundtrips() {
        assert_eq!(Config::default().title_bar_style, TitleBarStyle::Modern);
        assert_eq!(
            Config::parse("title-bar-style \"system\"").title_bar_style,
            TitleBarStyle::System
        );
        // `native` is an alias; unknown values fall back to the modern default.
        assert_eq!(
            Config::parse("title-bar-style \"native\"").title_bar_style,
            TitleBarStyle::System
        );
        assert_eq!(
            Config::parse("title-bar-style \"bogus\"").title_bar_style,
            TitleBarStyle::Modern
        );
        // Serializes and parses back to the same selection.
        let mut config = Config::default();
        config.title_bar_style = TitleBarStyle::System;
        assert_eq!(
            Config::parse(&config.to_kdl()).title_bar_style,
            TitleBarStyle::System
        );
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
    fn test_parse_cursor_block_and_synonyms() {
        let config = Config::parse(
            r#"
cursor {
    insert "beam"
    normal "underline"
    visual "underscore"
    block-focus "bar"
}
"#,
        );
        // Synonyms resolve to the canonical variants; "beam"→Bar,
        // "underline"/"underscore"→Underline.
        assert_eq!(config.cursor.insert, CursorShape::Bar);
        assert_eq!(config.cursor.normal, CursorShape::Underline);
        assert_eq!(config.cursor.visual, CursorShape::Underline);
        assert_eq!(config.cursor.block_focus, CursorShape::Bar);
    }

    #[test]
    fn test_window_controls_side_defaults_left_parses_right() {
        assert_eq!(Config::default().window_controls_side, ControlsSide::Left);

        let config = Config::parse("window-controls-side \"right\"");
        assert_eq!(config.window_controls_side, ControlsSide::Right);

        // Unknown values fall back to the left-side default.
        let config = Config::parse("window-controls-side \"sideways\"");
        assert_eq!(config.window_controls_side, ControlsSide::Left);
    }

    #[test]
    fn test_window_controls_side_roundtrips() {
        let mut config = Config::default();
        config.window_controls_side = ControlsSide::Left;
        let parsed = Config::parse(&config.to_kdl());
        assert_eq!(parsed.window_controls_side, ControlsSide::Left);
    }

    #[test]
    fn test_cursor_defaults_to_block_for_nav_bar_for_insert() {
        let config = Config::default();
        assert_eq!(config.cursor.insert, CursorShape::Bar);
        assert_eq!(config.cursor.normal, CursorShape::Block);
        assert_eq!(config.cursor.visual, CursorShape::Block);
        assert_eq!(config.cursor.block_focus, CursorShape::Bar);
    }

    #[test]
    fn test_cursor_block_roundtrips_through_kdl() {
        let mut config = Config::default();
        config.cursor.normal = CursorShape::Underline;
        config.cursor.visual = CursorShape::Bar;
        let parsed = Config::parse(&config.to_kdl());
        assert_eq!(parsed.cursor.insert, CursorShape::Bar);
        assert_eq!(parsed.cursor.normal, CursorShape::Underline);
        assert_eq!(parsed.cursor.visual, CursorShape::Bar);
        assert_eq!(parsed.cursor.block_focus, CursorShape::Bar);
    }

    #[test]
    fn test_to_kdl_roundtrips_scalar_settings() {
        let mut config = Config::default();
        config.theme = ThemeSetting::Light;
        config.menu_style = MenuStyle::Classic;
        config.font_family = Some("Fira Code".to_string());
        config.font_size = 18.0;
        config.opacity = 0.8;
        config.status_bar.enabled = false;
        config.status_bar.show_branding = false;

        let parsed = Config::parse(&config.to_kdl());
        assert_eq!(parsed.theme, ThemeSetting::Light);
        assert_eq!(parsed.menu_style, MenuStyle::Classic);
        assert_eq!(parsed.font_family.as_deref(), Some("Fira Code"));
        assert_eq!(parsed.font_size, 18.0);
        assert_eq!(parsed.opacity, 0.8);
        assert!(!parsed.status_bar.enabled);
        assert!(!parsed.status_bar.show_branding);
        assert!(parsed.status_bar.show_mode);
    }

    #[test]
    fn test_to_kdl_roundtrips_shell() {
        let mut config = Config::default();
        config.shell = Some("/usr/bin/fish".to_string());
        let parsed = Config::parse(&config.to_kdl());
        assert_eq!(parsed.shell.as_deref(), Some("/usr/bin/fish"));
    }

    #[test]
    fn test_to_kdl_omits_shell_when_none() {
        let kdl = Config::default().to_kdl();
        assert!(!kdl.contains("shell"), "shell should not appear when None");
    }

    #[test]
    fn test_theme_setting_from_and_to_value() {
        assert_eq!(ThemeSetting::from_value("auto"), ThemeSetting::Auto);
        assert_eq!(ThemeSetting::from_value("light"), ThemeSetting::Light);
        assert_eq!(ThemeSetting::from_value("dark"), ThemeSetting::Dark);
        assert_eq!(
            ThemeSetting::from_value("dracula"),
            ThemeSetting::Named("dracula".to_string())
        );
        assert_eq!(ThemeSetting::Named("nord".to_string()).as_value(), "nord");
        assert_eq!(ThemeSetting::Auto.as_value(), "auto");
    }

    #[test]
    fn test_parse_and_roundtrip_named_theme() {
        let config = Config::parse("theme \"dracula\"");
        assert_eq!(config.theme, ThemeSetting::Named("dracula".to_string()));
        let parsed = Config::parse(&config.to_kdl());
        assert_eq!(parsed.theme, ThemeSetting::Named("dracula".to_string()));
    }

    #[test]
    fn test_parse_theme_file_applies_base_and_colors() {
        let theme = parse_theme_file(
            r##"
base "light"
colors {
    background "#282a36"
    foreground "#f8f8f2"
}
"##,
        )
        .expect("theme file parses");
        // Base preset is light, then the two colors override it.
        assert_eq!(theme.background, ThemeRgb::parse_hex("#282a36").unwrap());
        assert_eq!(theme.foreground, ThemeRgb::parse_hex("#f8f8f2").unwrap());
        // An unspecified color keeps the light base.
        assert_eq!(theme.cursor_bg, Theme::light().cursor_bg);
    }

    #[test]
    fn test_parse_theme_file_defaults_base_to_dark() {
        let theme = parse_theme_file("colors {\n    background \"#000000\"\n}").unwrap();
        assert_eq!(theme.cursor_bg, Theme::dark().cursor_bg);
    }

    #[test]
    fn test_to_kdl_roundtrips_color_overrides() {
        let mut config = Config::default();
        config.colors.background = ThemeRgb::parse_hex("#1a1a2e");
        config.colors.cursor_bg = ThemeRgb::parse_hex("#52ad70");
        config.colors.ansi = vec![ThemeRgb::parse_hex("#000000").unwrap()];
        config.colors.indexed = vec![(136, ThemeRgb::parse_hex("#af8700").unwrap())];

        let parsed = Config::parse(&config.to_kdl());
        assert_eq!(parsed.colors.background, ThemeRgb::parse_hex("#1a1a2e"));
        assert_eq!(parsed.colors.cursor_bg, ThemeRgb::parse_hex("#52ad70"));
        assert_eq!(
            parsed.colors.ansi,
            vec![ThemeRgb::parse_hex("#000000").unwrap()]
        );
        assert_eq!(
            parsed.colors.indexed,
            vec![(136, ThemeRgb::parse_hex("#af8700").unwrap())]
        );
    }

    #[test]
    fn test_to_kdl_without_overrides_writes_no_colors_block() {
        let config = Config::default();
        let kdl = config.to_kdl();
        assert!(!kdl.contains("colors {"), "{kdl}");
        // A pristine config still round-trips to its defaults.
        let parsed = Config::parse(&kdl);
        assert_eq!(parsed.theme, ThemeSetting::Dark);
        assert_eq!(parsed.font_size, 15.0);
    }

    #[test]
    fn test_status_bar_visibility_defaults_on_and_parses_off() {
        let default = Config::default().status_bar;
        assert!(
            default.enabled && default.show_mode && default.show_title && default.show_branding
        );

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

    #[test]
    fn test_cursor_blink_parses_and_round_trips() {
        let c = Config::default();
        assert!(c.cursor.blink, "blink defaults to true");

        let no_blink = Config::parse("cursor {\n    blink \"false\"\n}");
        assert!(!no_blink.cursor.blink);

        // Round-trip via to_kdl / parse.
        let kdl = no_blink.to_kdl();
        let restored = Config::parse(&kdl);
        assert!(!restored.cursor.blink);
    }

    #[test]
    fn test_ligatures_parses_and_round_trips() {
        let c = Config::default();
        assert!(c.ligatures, "ligatures defaults to true");

        let no_lig = Config::parse(r#"ligatures "false""#);
        assert!(!no_lig.ligatures);

        let kdl = no_lig.to_kdl();
        let restored = Config::parse(&kdl);
        assert!(!restored.ligatures);
    }
}
