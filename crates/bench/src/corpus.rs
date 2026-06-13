//! Deterministic VT byte corpus shared by both backends.
//!
//! The same bytes drive the `glyphon` and `webgl` runs (the webgl host base64-
//! encodes them and hands them to the page), so the comparison is apples to
//! apples. Every line carries an SGR foreground color plus a fixed-width text
//! payload, exercising glyph shaping and color resolution.

// ========================================================================
// Constants
// ========================================================================

/// First index of the xterm 256-color cube (skips the 16 ANSI slots so the
/// stream paints the wide cube range rather than just the base palette).
const COLOR_CUBE_BASE: usize = 16;
const COLOR_CUBE_SPAN: usize = 216;
const FILLER: &str = "the quick brown fox jumps over the lazy dog 0123456789 ";
const LINE_COLS: usize = 100;

// ========================================================================
// Functions
// ========================================================================

/// Build `lines` rows of colored, fixed-width text terminated by `\r\n`.
pub fn generate(lines: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(lines * (LINE_COLS + 16));
    for i in 0..lines {
        let color = COLOR_CUBE_BASE + (i % COLOR_CUBE_SPAN);
        let mut visible = format!("{i:>7} ");
        while visible.len() < LINE_COLS {
            visible.push_str(FILLER);
        }
        visible.truncate(LINE_COLS);
        out.extend_from_slice(format!("\x1b[38;5;{color}m{visible}\x1b[0m\r\n").as_bytes());
    }
    out
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_is_deterministic_with_one_newline_per_line() {
        let a = generate(10);
        let b = generate(10);
        assert_eq!(a, b);
        assert_eq!(a.iter().filter(|&&c| c == b'\n').count(), 10);
    }
}
