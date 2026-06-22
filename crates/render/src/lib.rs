//! The terminal screen model: a 2D cell grid driven by VT sequences.
//!
//! This is the CPU side of the native text grid. [`Grid`] holds styled cells
//! and a cursor; [`Screen`] drives it from a byte stream via `vte` (printing,
//! cursor motion, SGR colors, erase, scroll). [`renderer::GpuRenderer`] draws a
//! `Grid` to a wgpu surface using `cosmic-text` + `glyphon` for glyph rendering.

mod chrome;
mod grid;
mod image;
mod markdown;
pub mod renderer;
mod screen;
mod theme;

pub use chrome::{
    chrome_rows, hit_test, ChromeHit, ControlsSide, Menu, MenuItem, MenuStyle, TabLabel, TopChrome,
};
pub use grid::{Cell, Color, CursorShape, EraseMode, Grid, RgbColor, Style};
pub use image::ImagePlacement;
pub use renderer::{start_font_load, FontConfig, FontLoad, PaletteItem, PaletteView, PaneRect, PaneView, StatusBar};
pub use screen::Screen;
pub use theme::{Rgb as ThemeRgb, Theme};

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_screen_new_dimensions() {
        let screen = Screen::new(80, 24);
        assert_eq!(screen.grid().cols(), 80);
        assert_eq!(screen.grid().rows(), 24);
    }
}
