//! Translating Vim delete operators on the last prompt into the byte edits the
//! shell's line editor understands.
//!
//! Normal mode never forwards keys to the PTY, so a delete is realized by
//! sending the shell the equivalent readline keystrokes: first arrow keys to
//! move the line-editor cursor under the Vim cursor, then a kill. This assumes
//! the default emacs-mode readline bindings (`Ctrl-K`, `Ctrl-U`, ...).

// ========================================================================
// Constants
// ========================================================================

/// `Ctrl-A` — move to start of line (readline `beginning-of-line`).
const CTRL_A: u8 = 0x01;
/// `Ctrl-K` — kill from cursor to end of line.
const CTRL_K: u8 = 0x0b;
/// `Ctrl-U` — kill from cursor to start of line.
const CTRL_U: u8 = 0x15;
/// `Ctrl-W` — kill the word before the cursor.
const CTRL_W: u8 = 0x17;
/// `Alt-d` — kill the word after the cursor.
const ALT_D: &[u8] = b"\x1bd";
/// The Delete key (CSI 3~) — forward-delete one character.
const KEY_DELETE: &[u8] = b"\x1b[3~";
const ARROW_LEFT: &[u8] = b"\x1b[D";
const ARROW_RIGHT: &[u8] = b"\x1b[C";

// ========================================================================
// Data Structures
// ========================================================================

/// A Vim delete operator targeting the last prompt line.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PromptDelete {
    /// `x` — the character under the cursor.
    CharForward,
    /// `dd` — the whole line.
    Line,
    /// `D` / `d$` — from the cursor to the end of the line.
    ToLineEnd,
    /// `d0` — from the cursor to the start of the line.
    ToLineStart,
    /// `db` — the word before the cursor.
    WordBack,
    /// `dw` — the word after the cursor.
    WordForward,
}

// ========================================================================
// Translation
// ========================================================================

/// The bytes that perform `op` when the line-editor cursor sits at `pty_col`
/// and the Vim cursor sits at `nav_col` on the same prompt row. Column-relative
/// operators are prefixed with arrow keys that align the editor cursor first;
/// `Line` ignores the columns and clears the whole line.
pub(crate) fn prompt_delete_bytes(op: PromptDelete, pty_col: usize, nav_col: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    if op != PromptDelete::Line {
        align_cursor(&mut bytes, pty_col, nav_col);
    }
    match op {
        PromptDelete::CharForward => bytes.extend_from_slice(KEY_DELETE),
        PromptDelete::Line => {
            bytes.push(CTRL_A);
            bytes.push(CTRL_K);
        }
        PromptDelete::ToLineEnd => bytes.push(CTRL_K),
        PromptDelete::ToLineStart => bytes.push(CTRL_U),
        PromptDelete::WordBack => bytes.push(CTRL_W),
        PromptDelete::WordForward => bytes.extend_from_slice(ALT_D),
    }
    bytes
}

/// Append the arrow keys that walk the editor cursor from `from` to `to`.
fn align_cursor(bytes: &mut Vec<u8>, from: usize, to: usize) {
    let (seq, steps) = if to >= from {
        (ARROW_RIGHT, to - from)
    } else {
        (ARROW_LEFT, from - to)
    };
    for _ in 0..steps {
        bytes.extend_from_slice(seq);
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_line_clears_without_alignment() {
        assert_eq!(
            prompt_delete_bytes(PromptDelete::Line, 7, 3),
            vec![CTRL_A, CTRL_K]
        );
    }

    #[test]
    fn test_char_forward_aligns_left_then_deletes() {
        // Cursor at col 7, Vim cursor at col 5: two left arrows, then Delete.
        let bytes = prompt_delete_bytes(PromptDelete::CharForward, 7, 5);
        assert_eq!(bytes, [ARROW_LEFT, ARROW_LEFT, KEY_DELETE].concat());
    }

    #[test]
    fn test_to_line_end_aligns_right_then_kills() {
        // Cursor at col 2, Vim cursor at col 4: two right arrows, then Ctrl-K.
        let bytes = prompt_delete_bytes(PromptDelete::ToLineEnd, 2, 4);
        assert_eq!(bytes, [ARROW_RIGHT, ARROW_RIGHT, &[CTRL_K][..]].concat());
    }

    #[test]
    fn test_to_line_start_kills_with_ctrl_u() {
        let bytes = prompt_delete_bytes(PromptDelete::ToLineStart, 4, 4);
        assert_eq!(bytes, vec![CTRL_U]);
    }

    #[test]
    fn test_word_back_and_forward() {
        assert_eq!(
            prompt_delete_bytes(PromptDelete::WordBack, 3, 3),
            vec![CTRL_W]
        );
        assert_eq!(
            prompt_delete_bytes(PromptDelete::WordForward, 3, 3),
            ALT_D.to_vec()
        );
    }
}
