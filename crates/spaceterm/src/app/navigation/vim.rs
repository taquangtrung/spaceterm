//! Vim-style word and line motion helpers for Normal-mode cursor navigation.
//!
//! These are pure functions over a [`Grid`](spaceterm_render::Grid) reference;
//! they do not touch `App` state directly.

use spaceterm_render::Grid;

// ========================================================================
// Word classification
// ========================================================================

/// A character's word class, à la Vim. Blanks (class 0) separate words. With
/// `big` false (`w`/`b`/`e`): keyword runs (alphanumerics and `_`, class 1) are
/// distinct from punctuation runs (class 2). With `big` true (`W`/`B`/`E`): any
/// non-blank is class 1, so only whitespace breaks a WORD.
pub(super) fn char_class(c: char, big: bool) -> u8 {
    if c == '\0' || c.is_whitespace() {
        0
    } else if big || c.is_alphanumeric() || c == '_' {
        1
    } else {
        2
    }
}

// ========================================================================
// Single-line word motion primitives
// ========================================================================

/// The start column of the next word at or after `col` (Vim `w`/`W`), or `None`
/// when the rest of the line holds no further word.
pub(super) fn next_word_start(line: &[char], col: usize, big: bool) -> Option<usize> {
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
pub(super) fn prev_word_start(line: &[char], col: usize, big: bool) -> Option<usize> {
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
pub(super) fn word_end(line: &[char], col: usize, big: bool) -> Option<usize> {
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
pub(super) fn first_non_blank(line: &[char]) -> usize {
    line.iter()
        .position(|c| char_class(*c, false) != 0)
        .unwrap_or(0)
}

// ========================================================================
// Multi-line motion wrappers
// ========================================================================

/// `w`/`W`: the next word start, wrapping to the next line (scrolling at the
/// bottom edge) when the current line has no further word.
pub(super) fn motion_word_forward(
    grid: &mut Grid,
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
pub(super) fn motion_word_back(
    grid: &mut Grid,
    row: usize,
    col: usize,
    big: bool,
) -> (usize, usize) {
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
pub(super) fn motion_word_end(
    grid: &mut Grid,
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

// ========================================================================
// Row stepping
// ========================================================================

/// Step one visible row down, scrolling history at the bottom edge.
fn next_row(grid: &mut Grid, rows: usize, row: usize) -> usize {
    if row + 1 < rows {
        row + 1
    } else {
        grid.scroll_down_history(1);
        row
    }
}

/// Step one visible row up, scrolling history at the top edge.
fn prev_row(grid: &mut Grid, row: usize) -> usize {
    if row > 0 {
        row - 1
    } else {
        grid.scroll_up_history(1);
        row
    }
}

/// The printed characters of a visible row, trimmed of trailing blank padding so
/// motions see real line ends. A fully blank row yields an empty slice.
pub(super) fn line_chars(grid: &Grid, row: usize) -> Vec<char> {
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
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

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
}
