//! Terminal color theme: all visual colors used by the GPU renderer. Built-in
//! presets (WezTerm-inspired); user overrides are applied by the app layer from
//! the KDL config.

// ========================================================================
// Data Structures
// ========================================================================

/// A 24-bit RGB color stored as three `u8` values.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Rgb {
    pub b: u8,
    pub g: u8,
    pub r: u8,
}

/// The complete set of colors used to render the terminal.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub ansi: [Rgb; 16],
    pub background: Rgb,
    pub bell: Rgb,
    pub cursor_bg: Rgb,
    pub cursor_fg: Rgb,
    pub divider: Rgb,
    pub foreground: Rgb,
    pub indexed: Vec<(u8, Rgb)>,
    /// Elevated surface of the dropdown menu panel, lighter than the terminal so
    /// the menu reads as a card floating above it.
    pub menu_bg: Rgb,
    /// Highlight behind a hovered dropdown menu item.
    pub menu_hover_bg: Rgb,
    pub selection_bg: Rgb,
    pub selection_fg: Rgb,
    pub status_bar_fg: Rgb,
    /// Fill behind the active tab in the tabbar.
    pub tab_active_bg: Rgb,
    /// Text color of the active tab's title.
    pub tab_active_fg: Rgb,
    /// Background of the tabbar / menubar bands.
    pub tabbar_bg: Rgb,
}

// ========================================================================
// Rgb
// ========================================================================

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { b, g, r }
    }

    pub fn as_linear(self) -> (f32, f32, f32) {
        (
            self.r as f32 / 255.0,
            self.g as f32 / 255.0,
            self.b as f32 / 255.0,
        )
    }

    pub fn to_glyphon(self) -> glyphon::Color {
        glyphon::Color::rgba(self.r, self.g, self.b, 255)
    }

    pub fn parse_hex(s: &str) -> Option<Self> {
        let s = s.strip_prefix('#')?;
        if s.len() != 6 {
            return None;
        }
        let r = u8::from_str_radix(&s[0..2], 16).ok()?;
        let g = u8::from_str_radix(&s[2..4], 16).ok()?;
        let b = u8::from_str_radix(&s[4..6], 16).ok()?;
        Some(Self { b, g, r })
    }

    /// Format as a `#rrggbb` hex string, the inverse of [`Self::parse_hex`].
    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

// ========================================================================
// Theme
// ========================================================================

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

impl Theme {
    /// The built-in dark preset (WezTerm-inspired).
    pub fn dark() -> Self {
        Self {
            background: Rgb::parse_hex("#2a2f31").unwrap(),
            foreground: Rgb::parse_hex("#d8d8d8").unwrap(),
            cursor_bg: Rgb::parse_hex("#52ad70").unwrap(),
            cursor_fg: Rgb::parse_hex("#000000").unwrap(),
            selection_bg: Rgb::parse_hex("#fffacd").unwrap(),
            selection_fg: Rgb::parse_hex("#000000").unwrap(),
            divider: Rgb::parse_hex("#51554f").unwrap(),
            bell: Rgb::parse_hex("#202020").unwrap(),
            menu_bg: Rgb::parse_hex("#313841").unwrap(),
            menu_hover_bg: Rgb::parse_hex("#3b4a63").unwrap(),
            status_bar_fg: Rgb::parse_hex("#15181a").unwrap(),
            tab_active_bg: Rgb::parse_hex("#2a2f31").unwrap(),
            tab_active_fg: Rgb::parse_hex("#d8d8d8").unwrap(),
            tabbar_bg: Rgb::parse_hex("#1b1f20").unwrap(),
            ansi: [
                Rgb::parse_hex("#000000").unwrap(), // black
                Rgb::parse_hex("#c22727").unwrap(), // red
                Rgb::parse_hex("#71b312").unwrap(), // green
                Rgb::parse_hex("#faa213").unwrap(), // yellow
                Rgb::parse_hex("#4fa2fa").unwrap(), // blue
                Rgb::parse_hex("#bb67b2").unwrap(), // magenta
                Rgb::parse_hex("#21afbf").unwrap(), // cyan
                Rgb::parse_hex("#c0c0c0").unwrap(), // white
                Rgb::parse_hex("#7a7a7a").unwrap(), // bright black
                Rgb::parse_hex("#d43f30").unwrap(), // bright red
                Rgb::parse_hex("#71b312").unwrap(), // bright green
                Rgb::parse_hex("#ebb909").unwrap(), // bright yellow
                Rgb::parse_hex("#5da2eb").unwrap(), // bright blue
                Rgb::parse_hex("#c97df5").unwrap(), // bright magenta
                Rgb::parse_hex("#04cfe1").unwrap(), // bright cyan
                Rgb::parse_hex("#e1ebfa").unwrap(), // bright white
            ],
            indexed: vec![(136, Rgb::parse_hex("#af8700").unwrap())],
        }
    }

    /// The built-in light preset. Reuses the dark palette but swaps the surface
    /// colors (background, foreground, cursor, selection, divider, bell).
    pub fn light() -> Self {
        Self {
            background: Rgb::parse_hex("#f2f2f2").unwrap(),
            foreground: Rgb::parse_hex("#1e1e1e").unwrap(),
            cursor_bg: Rgb::parse_hex("#262626").unwrap(),
            cursor_fg: Rgb::parse_hex("#ffffff").unwrap(),
            selection_bg: Rgb::parse_hex("#b3b3d9").unwrap(),
            selection_fg: Rgb::parse_hex("#000000").unwrap(),
            divider: Rgb::parse_hex("#cccccc").unwrap(),
            bell: Rgb::parse_hex("#dddddd").unwrap(),
            menu_bg: Rgb::parse_hex("#ffffff").unwrap(),
            menu_hover_bg: Rgb::parse_hex("#e6eefb").unwrap(),
            tab_active_bg: Rgb::parse_hex("#f2f2f2").unwrap(),
            tab_active_fg: Rgb::parse_hex("#1e1e1e").unwrap(),
            tabbar_bg: Rgb::parse_hex("#d0d0d0").unwrap(),
            ..Self::dark()
        }
    }

    pub fn ansi_color(&self, index: u8) -> (u8, u8, u8) {
        if (index as usize) < self.ansi.len() {
            let c = self.ansi[index as usize];
            (c.r, c.g, c.b)
        } else {
            (0, 0, 0)
        }
    }

    pub fn indexed_color(&self, index: u8) -> Option<(u8, u8, u8)> {
        for &(idx, c) in &self.indexed {
            if idx == index {
                return Some((c.r, c.g, c.b));
            }
        }
        None
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_theme_is_valid() {
        let theme = Theme::default();
        assert_ne!(theme.background, theme.foreground);
        assert_eq!(theme.ansi.len(), 16);
        let (r, g, b) = theme.ansi_color(0);
        assert_eq!((r, g, b), (0, 0, 0));
    }

    #[test]
    fn test_light_differs_from_dark() {
        assert_ne!(Theme::light().background, Theme::dark().background);
        assert_eq!(Theme::light().ansi, Theme::dark().ansi);
    }

    #[test]
    fn test_parse_hex_rgb() {
        assert_eq!(Rgb::parse_hex("#ff0000"), Some(Rgb::new(255, 0, 0)));
        assert_eq!(Rgb::parse_hex("#00ff00"), Some(Rgb::new(0, 255, 0)));
        assert_eq!(Rgb::parse_hex("#0000ff"), Some(Rgb::new(0, 0, 255)));
        assert_eq!(Rgb::parse_hex("ff0000"), None);
        assert_eq!(Rgb::parse_hex("#fff"), None);
        assert_eq!(Rgb::parse_hex("#gggggg"), None);
    }

    #[test]
    fn test_to_hex_roundtrips_parse_hex() {
        for hex in ["#ff0000", "#00ff00", "#0000ff", "#2a2f31", "#000000"] {
            assert_eq!(Rgb::parse_hex(hex).unwrap().to_hex(), hex);
        }
    }

    #[test]
    fn test_rgb_as_linear() {
        let c = Rgb::new(255, 128, 0);
        let (r, g, b) = c.as_linear();
        assert!((r - 1.0).abs() < f32::EPSILON);
        assert!((g - 128.0 / 255.0).abs() < 1e-6);
        assert!((b - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_indexed_color_not_found() {
        let theme = Theme::default();
        assert_eq!(theme.indexed_color(0), None);
        assert!(theme.indexed_color(136).is_some());
    }
}
