//! The VT driver: parses a byte stream with `vte` and applies it to a [`Grid`]
//! (printing, cursor motion, erase, and SGR styling).

use vte::{Params, Parser, Perform};

use crate::grid::{Color, CursorShape, EraseMode, Grid, RgbColor, Style};

// ========================================================================
// Constants
// ========================================================================

const SGR_EXT_RGB: u16 = 2;
const SGR_EXT_INDEXED: u16 = 5;
const BRIGHT_OFFSET: u8 = 8;
const BELL: u8 = 0x07;

// ========================================================================
// Data Structures
// ========================================================================

/// A terminal screen: a `vte` parser feeding a [`Grid`].
pub struct Screen {
    grid: Grid,
    parser: Parser,
    bell: bool,
}

// ========================================================================
// Screen
// ========================================================================

impl Screen {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            grid: Grid::new(cols, rows),
            parser: Parser::new(),
            bell: false,
        }
    }

    /// Consume a chunk of terminal output, updating the grid.
    pub fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.parser.advance(&mut self.grid, byte);
            if byte == BELL {
                self.bell = true;
            }
        }
    }

    pub fn grid(&self) -> &Grid {
        &self.grid
    }

    pub fn grid_mut(&mut self) -> &mut Grid {
        &mut self.grid
    }

    /// Whether a bell character was received since the last check.
    pub fn take_bell(&mut self) -> bool {
        std::mem::take(&mut self.bell)
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.grid.resize(cols, rows);
    }
}

// ========================================================================
// vte driver
// ========================================================================

impl Perform for Grid {
    fn print(&mut self, c: char) {
        Grid::print(self, c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            LINE_FEED => self.line_feed(),
            CARRIAGE_RETURN => self.carriage_return(),
            BACKSPACE => self.backspace(),
            HORIZONTAL_TAB => self.tab(),
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        if intermediates == *b"?" {
            let set = action == 'h';
            for sub in params.iter() {
                if let Some(&mode) = sub.first() {
                    self.set_private_mode(mode, set);
                }
            }
            return;
        }
        match action {
            '@' => self.insert_chars(first_param(params, 1)),
            'A' => self.move_up(first_param(params, 1)),
            'B' => self.move_down(first_param(params, 1)),
            'C' => self.move_right(first_param(params, 1)),
            'D' => self.move_left(first_param(params, 1)),
            'H' | 'f' => {
                let row = first_param(params, 1).saturating_sub(1);
                let col = nth_param(params, 1, 1).saturating_sub(1);
                self.move_to(row, col);
            }
            'J' => self.erase_in_display(erase_mode(first_param(params, 0))),
            'K' => self.erase_in_line(erase_mode(first_param(params, 0))),
            'L' => self.insert_lines(first_param(params, 1)),
            'M' => self.delete_lines(first_param(params, 1)),
            'P' => self.delete_chars(first_param(params, 1)),
            'S' => self.scroll_up(first_param(params, 1)),
            'T' => self.scroll_down(first_param(params, 1)),
            'r' => {
                let top = first_param(params, 1).saturating_sub(1);
                let bottom = match params.iter().nth(1).and_then(|p| p.first()) {
                    Some(&0) | None => self.rows(),
                    Some(&n) => n as usize,
                };
                if top == 0 && bottom == self.rows() {
                    self.reset_scroll_region();
                } else {
                    self.set_scroll_region(top, bottom.saturating_sub(1));
                }
            }
            's' => self.save_cursor(),
            'u' => self.restore_cursor(),
            'q' => {
                if intermediates == *b" " {
                    let shape = match first_param(params, 0) {
                        1 | 2 => CursorShape::Block,
                        3 | 4 => CursorShape::Underline,
                        5 | 6 => CursorShape::Bar,
                        _ => CursorShape::Block,
                    };
                    self.set_cursor_shape(shape);
                }
            }
            'm' => {
                let mut style = self.style();
                apply_sgr(params, &mut style);
                self.set_style(style);
            }
            _ => {}
        }
    }

    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _action: char) {}

    fn put(&mut self, _byte: u8) {}

    fn unhook(&mut self) {}

    fn osc_dispatch(&mut self, _params: &[&[u8]], _bell_terminated: bool) {}

    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, byte: u8) {
        match byte {
            b'7' => self.save_cursor(),
            b'8' => self.restore_cursor(),
            _ => {}
        }
    }
}

// ========================================================================
// CSI / SGR helpers
// ========================================================================

const LINE_FEED: u8 = b'\n';
const CARRIAGE_RETURN: u8 = b'\r';
const BACKSPACE: u8 = 0x08;
const HORIZONTAL_TAB: u8 = b'\t';

fn first_param(params: &Params, default: usize) -> usize {
    nth_param(params, 0, default)
}

fn nth_param(params: &Params, index: usize, default: usize) -> usize {
    match params.iter().nth(index).and_then(|p| p.first()) {
        Some(&0) | None => default,
        Some(&value) => value as usize,
    }
}

fn erase_mode(value: usize) -> EraseMode {
    match value {
        1 => EraseMode::ToStart,
        2 => EraseMode::Whole,
        _ => EraseMode::ToEnd,
    }
}

/// Apply an SGR sequence to `style`. Subparameters are flattened so the colon and
/// semicolon forms of extended colors (`38:5:n` and `38;5;n`) are handled alike.
fn apply_sgr(params: &Params, style: &mut Style) {
    let flat: Vec<u16> = params.iter().flatten().copied().collect();
    if flat.is_empty() {
        *style = Style::default();
        return;
    }

    let mut index = 0;
    while index < flat.len() {
        index += apply_sgr_code(&flat[index..], style);
    }
}

/// Apply one SGR code at the start of `codes`, returning how many codes it
/// consumed (extended colors consume several).
fn apply_sgr_code(codes: &[u16], style: &mut Style) -> usize {
    let code = codes[0];
    match code {
        0 => *style = Style::default(),
        1 => style.bold = true,
        3 => style.italic = true,
        4 => style.underline = true,
        22 => style.bold = false,
        23 => style.italic = false,
        24 => style.underline = false,
        30..=37 => style.foreground = Color::Indexed((code - 30) as u8),
        90..=97 => style.foreground = Color::Indexed((code - 90) as u8 + BRIGHT_OFFSET),
        39 => style.foreground = Color::Default,
        40..=47 => style.background = Color::Indexed((code - 40) as u8),
        100..=107 => style.background = Color::Indexed((code - 100) as u8 + BRIGHT_OFFSET),
        49 => style.background = Color::Default,
        38 => return extended_color(codes, |color| style.foreground = color),
        48 => return extended_color(codes, |color| style.background = color),
        _ => {}
    }
    1
}

/// Parse `38`/`48` extended color (`;5;n` indexed or `;2;r;g;b` true color),
/// returning the number of codes consumed including the leading `38`/`48`.
fn extended_color(codes: &[u16], mut set: impl FnMut(Color)) -> usize {
    match codes.get(1) {
        Some(&SGR_EXT_INDEXED) => match codes.get(2) {
            Some(&value) => {
                set(Color::Indexed(value as u8));
                3
            }
            None => 2,
        },
        Some(&SGR_EXT_RGB) => match (codes.get(2), codes.get(3), codes.get(4)) {
            (Some(&r), Some(&g), Some(&b)) => {
                set(Color::Rgb(RgbColor {
                    r: r as u8,
                    g: g as u8,
                    b: b as u8,
                }));
                5
            }
            _ => 2,
        },
        _ => 1,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(cols: usize, rows: usize, bytes: &[u8]) -> Screen {
        let mut screen = Screen::new(cols, rows);
        screen.feed(bytes);
        screen
    }

    #[test]
    fn test_plain_text_and_newline() {
        let screen = feed(10, 2, b"hi\r\nthere");
        assert_eq!(screen.grid().to_text(), "hi\nthere");
    }

    #[test]
    fn test_cursor_position_then_print_overwrites() {
        // CUP to row 1, col 1, then write — the 'X' lands at the origin.
        let screen = feed(5, 2, b"abc\x1b[1;1HX");
        assert_eq!(screen.grid().to_text(), "Xbc");
    }

    #[test]
    fn test_erase_line_from_cursor() {
        let screen = feed(8, 1, b"hello\x1b[3D\x1b[K");
        assert_eq!(screen.grid().to_text(), "he");
    }

    #[test]
    fn test_sgr_sets_bold_and_truecolor_foreground() {
        let screen = feed(4, 1, b"\x1b[1;38;2;10;20;30mZ");
        let cell = screen.grid().cell(0, 0).expect("cell exists");
        assert!(cell.style.bold);
        assert_eq!(
            cell.style.foreground,
            Color::Rgb(RgbColor {
                r: 10,
                g: 20,
                b: 30
            })
        );
    }

    #[test]
    fn test_sgr_reset_clears_style() {
        let screen = feed(4, 1, b"\x1b[1m\x1b[0mz");
        let cell = screen.grid().cell(0, 0).expect("cell exists");
        assert!(!cell.style.bold);
        assert_eq!(cell.style.foreground, Color::Default);
    }

    #[test]
    fn test_esc_7_8_save_restore_cursor() {
        let screen = feed(5, 3, b"\x1b[2;3H\x1b7\x1b[1;1HX\x1b8Y");
        assert_eq!(screen.grid().cursor(), (1, 3));
    }

    #[test]
    fn test_csi_s_u_save_restore_cursor() {
        let screen = feed(5, 3, b"\x1b[2;3H\x1b[s\x1b[1;1HX\x1b[uY");
        assert_eq!(screen.grid().cursor(), (1, 3));
    }

    #[test]
    fn test_alt_screen_via_csi_1049() {
        let screen = feed(3, 2, b"abc\x1b[?1049hXYZ");
        assert!(screen.grid().is_alt_screen());
        assert_eq!(screen.grid().cell(0, 0).map(|c| c.ch), Some('X'));
    }

    #[test]
    fn test_alt_screen_restore_via_csi_1049() {
        let screen = feed(3, 2, b"abc\x1b[?1049hXYZ\x1b[?1049l");
        assert!(!screen.grid().is_alt_screen());
        assert_eq!(screen.grid().cell(0, 0).map(|c| c.ch), Some('a'));
    }

    #[test]
    fn test_insert_lines_csi_l() {
        let screen = feed(3, 3, b"abcdef\x1b[2;1H\x1b[1L");
        assert_eq!(screen.grid().cell(0, 0).map(|c| c.ch), Some('a'));
        assert_eq!(screen.grid().cell(1, 0).map(|c| c.ch), Some(' '));
        assert_eq!(screen.grid().cell(2, 0).map(|c| c.ch), Some('d'));
    }

    #[test]
    fn test_delete_lines_csi_m() {
        let screen = feed(3, 3, b"abcdef\x1b[1;1H\x1b[1M");
        assert_eq!(screen.grid().cell(0, 0).map(|c| c.ch), Some('d'));
        assert_eq!(screen.grid().cell(1, 0).map(|c| c.ch), Some(' '));
        assert_eq!(screen.grid().cell(2, 0).map(|c| c.ch), Some(' '));
    }

    #[test]
    fn test_insert_chars_csi_at() {
        let screen = feed(5, 1, b"hello\x1b[1;2H\x1b[2@");
        assert_eq!(screen.grid().cell(0, 0).map(|c| c.ch), Some('h'));
        assert_eq!(screen.grid().cell(0, 1).map(|c| c.ch), Some(' '));
    }

    #[test]
    fn test_delete_chars_csi_p() {
        let screen = feed(5, 1, b"hello\x1b[1;2H\x1b[2P");
        assert_eq!(screen.grid().cell(0, 0).map(|c| c.ch), Some('h'));
        assert_eq!(screen.grid().cell(0, 1).map(|c| c.ch), Some('l'));
    }

    #[test]
    fn test_scroll_region_csi_r() {
        let screen = feed(3, 4, b"abcdefghijkl\x1b[2;3r\x1b[3;1H\n");
        assert_eq!(screen.grid().cell(0, 0).map(|c| c.ch), Some('a'));
    }

    #[test]
    fn test_decscusr_cursor_shape() {
        let mut screen = Screen::new(5, 1);
        assert_eq!(screen.grid().cursor_shape(), CursorShape::Block);

        screen.feed(b"\x1b[5 q");
        assert_eq!(screen.grid().cursor_shape(), CursorShape::Bar);

        screen.feed(b"\x1b[3 q");
        assert_eq!(screen.grid().cursor_shape(), CursorShape::Underline);

        screen.feed(b"\x1b[1 q");
        assert_eq!(screen.grid().cursor_shape(), CursorShape::Block);

        screen.feed(b"\x1b[0 q");
        assert_eq!(screen.grid().cursor_shape(), CursorShape::Block);
    }
}
