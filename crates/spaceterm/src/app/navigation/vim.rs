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

/// The landing column for a Vim char-search (`f`/`F`/`t`/`T`) on `line` from
/// `col`. `forward` searches right of the cursor, else left; `till` stops one
/// cell short of the match. Returns `None` when there is no match, or when a
/// `till` search would not move (target already adjacent).
pub(super) fn find_char(line: &[char], col: usize, find: super::input::FindChar) -> Option<usize> {
    let super::input::FindChar { ch, forward, till } = find;
    let target = if forward {
        (col + 1..line.len()).find(|&i| line[i] == ch)?
    } else {
        (0..col).rev().find(|&i| line[i] == ch)?
    };
    let landing = if !till {
        target
    } else if forward {
        target.checked_sub(1)?
    } else {
        target + 1
    };
    (landing != col).then_some(landing)
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

/// The rightmost column the Normal-mode cursor may occupy on visible `row`.
///
/// Usually the last printed character ([`Grid::visible_line_end`]), so the
/// cursor never wanders into the blank padding past a line. On the live prompt
/// row it extends to the shell cursor: a typed trailing space is indistinguishable
/// from blank padding in the cell grid (both are `' '`), and reaching the
/// insertion point itself keeps the cursor at the same column when the user
/// switches modes (Insert's shell cursor sits at that exact column).
pub(super) fn nav_line_end(grid: &Grid, row: usize) -> usize {
    let end = grid.visible_line_end(row);
    let (cursor_row, cursor_col) = grid.cursor();
    if grid.scroll_offset() == 0 && row == cursor_row {
        let cap = grid.cols().saturating_sub(1);
        end.max(cursor_col.min(cap))
    } else {
        end
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
    fn test_nav_line_end_extends_to_shell_cursor_for_trailing_space() {
        // A command typed with a trailing space: the cell grid stores the space
        // like blank padding, so the shell cursor (col 3) marks the real end.
        let mut grid = Grid::new(20, 3);
        for ch in "cd ".chars() {
            grid.print(ch);
        }
        assert_eq!(grid.visible_line_end(0), 1); // last printed glyph is 'd'
        assert_eq!(grid.cursor(), (0, 3));
        // nav_line_end reaches the shell cursor's column so the Normal-mode
        // cursor can sit at the same position the Insert-mode cursor did.
        assert_eq!(nav_line_end(&grid, 0), 3);
    }

    #[test]
    fn test_nav_line_end_reaches_shell_cursor_without_trailing_space() {
        let mut grid = Grid::new(20, 3);
        for ch in "cd".chars() {
            grid.print(ch);
        }
        // Even without trailing whitespace, nav_line_end reaches the shell
        // cursor (col 2) so Normal mode can start at the same column Insert's
        // shell cursor occupied.
        assert_eq!(grid.cursor(), (0, 2));
        assert_eq!(nav_line_end(&grid, 0), 2);
    }

    #[test]
    fn test_nav_line_end_does_not_extend_non_prompt_rows() {
        let mut grid = Grid::new(20, 3);
        for ch in "out ".chars() {
            grid.print(ch);
        }
        grid.line_feed(); // shell cursor moves to row 1
        grid.carriage_return();
        // Row 0 is no longer the cursor row, so its trailing space is padding.
        assert_eq!(nav_line_end(&grid, 0), 2); // last glyph 't'
    }

    #[test]
    fn test_find_char_forward_backward_and_till() {
        use crate::model::input::FindChar;
        let line: Vec<char> = "abcabc".chars().collect();
        let find = |forward, till| FindChar {
            ch: 'c',
            forward,
            till,
        };

        // `fc` from 0 lands on the first 'c' (index 2); repeating from there
        // (`;`) advances to the next 'c' at 5.
        assert_eq!(find_char(&line, 0, find(true, false)), Some(2));
        assert_eq!(find_char(&line, 2, find(true, false)), Some(5));
        // `tc` stops one cell short of the 'c'.
        assert_eq!(find_char(&line, 0, find(true, true)), Some(1));
        // `Fc` searches left; `Tc` stops one cell past it (to the right).
        assert_eq!(find_char(&line, 5, find(false, false)), Some(2));
        assert_eq!(find_char(&line, 5, find(false, true)), Some(3));
        // A miss leaves the caller to keep the cursor put.
        let miss = FindChar {
            ch: 'z',
            forward: true,
            till: false,
        };
        assert_eq!(find_char(&line, 0, miss), None);
        // A till search onto the adjacent cell would not move, so it reports None.
        assert_eq!(find_char(&line, 1, find(true, true)), None);
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
