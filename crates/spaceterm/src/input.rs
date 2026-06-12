//! Keymap: resolve a key event into an [`Action`], given the pane's [`Mode`].
//!
//! In Insert, keys encode to terminal bytes for the PTY (the entry chord is the
//! one exception). In Normal, SpaceTerm intercepts keys as navigation/layout actions.
//! In Block-focus, keys forward to the block until `Esc`. The default bindings
//! live here; KDL-configured keymaps (§5.7) replace this map later.

use crate::layout::{Direction, FocusDir};
use crate::mode::{Mode, ModeEvent};

// ========================================================================
// Data Structures
// ========================================================================

/// A decoded key event from the windowing layer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Key {
    pub alt: bool,
    pub code: KeyCode,
    pub ctrl: bool,
    pub shift: bool,
}

/// A physical key, independent of modifiers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyCode {
    Backspace,
    Char(char),
    Delete,
    Down,
    End,
    Enter,
    Escape,
    F(u8),
    Home,
    Insert,
    Left,
    PageDown,
    PageUp,
    Right,
    Space,
    Tab,
    Up,
}

/// What a key resolves to in the current mode.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    ClosePane,
    FocusBlock(BlockNav),
    FocusPane(FocusDir),
    ForwardToBlock(Vec<u8>),
    Ignore,
    MoveCursor(CursorMove),
    QuickCancel,
    QuickJump(char),
    QuickSelect,
    SearchBackspace,
    SearchCancel,
    SearchChar(char),
    SearchExecute,
    SearchNext,
    SearchPrevious,
    SearchStart,
    SendBytes(Vec<u8>),
    SplitPane(Direction),
    SwitchMode(Mode),
    ToggleFold,
    YankBlock,
}

/// Direction of a block-selection move in Normal mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockNav {
    Next,
    Previous,
}

/// A Normal-mode cursor traversal over the pane's whole buffer, scrollback
/// included. Moves past a viewport edge scroll the view to follow the cursor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CursorMove {
    Bottom,
    Down,
    FirstNonBlank,
    HalfPageDown,
    HalfPageUp,
    Left,
    LineEnd,
    LineStart,
    PageDown,
    PageUp,
    Right,
    Top,
    Up,
    WordBack,
    WordBackBig,
    WordEnd,
    WordEndBig,
    WordForward,
    WordForwardBig,
}

/// Tracks a pending prefix awaiting the second key in a vim-style multi-key
/// sequence (e.g. `]b`, `[b`, `za`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PendingPrefix {
    BracketClose,
    BracketOpen,
    G,
    None,
    QuickSelect,
    SearchInput,
    Z,
}

// ========================================================================
// Constants
// ========================================================================

const CONTROL_MASK: u8 = 0x1f;
const DELETE: u8 = 0x7f;
const ESCAPE: u8 = 0x1b;
const CARRIAGE_RETURN: u8 = b'\r';

// ========================================================================
// Resolution
// ========================================================================

/// Resolve a key in the given mode using the default keymap.
/// `pending` tracks multi-key sequences (e.g. `]b`); it is updated in place.
/// `fullscreen` is true when a fullscreen app holds the pane (the alternate
/// screen); it keeps `Esc` bound to the PTY instead of entering Normal mode.
pub fn resolve(mode: Mode, key: &Key, pending: &mut PendingPrefix, fullscreen: bool) -> Action {
    match mode {
        Mode::Insert => resolve_insert(key, fullscreen),
        Mode::Normal => resolve_normal(key, pending),
        Mode::BlockFocus => resolve_block_focus(key),
    }
}

fn resolve_insert(key: &Key, fullscreen: bool) -> Action {
    if is_entry_chord(key) {
        return Action::SwitchMode(Mode::Insert.apply(ModeEvent::EnterNormal));
    }
    // Esc enters Normal mode, except while a fullscreen app owns the screen,
    // where Esc belongs to that app (e.g. vim, less, htop).
    if key.code == KeyCode::Escape && !key.ctrl && !key.alt && !fullscreen {
        return Action::SwitchMode(Mode::Insert.apply(ModeEvent::EnterNormal));
    }
    Action::SendBytes(encode(key))
}

fn resolve_normal(key: &Key, pending: &mut PendingPrefix) -> Action {
    if key.ctrl {
        *pending = PendingPrefix::None;
        return match key.code {
            KeyCode::Char('h') => Action::FocusPane(FocusDir::Left),
            KeyCode::Char('j') => Action::FocusPane(FocusDir::Down),
            KeyCode::Char('k') => Action::FocusPane(FocusDir::Up),
            KeyCode::Char('l') => Action::FocusPane(FocusDir::Right),
            KeyCode::Char('d') => Action::MoveCursor(CursorMove::HalfPageDown),
            KeyCode::Char('u') => Action::MoveCursor(CursorMove::HalfPageUp),
            _ => Action::Ignore,
        };
    }

    let prev = *pending;
    *pending = PendingPrefix::None;

    match prev {
        PendingPrefix::BracketClose => match key.code {
            KeyCode::Char('b') => Action::FocusBlock(BlockNav::Next),
            _ => Action::Ignore,
        },
        PendingPrefix::BracketOpen => match key.code {
            KeyCode::Char('b') => Action::FocusBlock(BlockNav::Previous),
            _ => Action::Ignore,
        },
        PendingPrefix::G => match key.code {
            KeyCode::Char('g') => Action::MoveCursor(CursorMove::Top),
            _ => Action::Ignore,
        },
        PendingPrefix::Z => match key.code {
            KeyCode::Char('a') => Action::ToggleFold,
            _ => Action::Ignore,
        },
        PendingPrefix::QuickSelect => match key.code {
            KeyCode::Escape => Action::QuickCancel,
            KeyCode::Char(c) if c.is_ascii_alphabetic() => Action::QuickJump(c),
            _ => Action::QuickCancel,
        },
        PendingPrefix::SearchInput => match key.code {
            KeyCode::Escape => Action::SearchCancel,
            KeyCode::Enter => Action::SearchExecute,
            KeyCode::Backspace => {
                *pending = PendingPrefix::SearchInput;
                Action::SearchBackspace
            }
            KeyCode::Char(c) => {
                *pending = PendingPrefix::SearchInput;
                Action::SearchChar(c)
            }
            _ => Action::Ignore,
        },
        PendingPrefix::None => match key.code {
            KeyCode::Char('i') => Action::SwitchMode(Mode::Normal.apply(ModeEvent::ToInsert)),
            KeyCode::Escape => Action::SwitchMode(Mode::Normal.apply(ModeEvent::Escape)),
            KeyCode::Enter => Action::SwitchMode(Mode::Normal.apply(ModeEvent::FocusBlock)),
            KeyCode::Char('h') | KeyCode::Left => Action::MoveCursor(CursorMove::Left),
            KeyCode::Char('j') | KeyCode::Down => Action::MoveCursor(CursorMove::Down),
            KeyCode::Char('k') | KeyCode::Up => Action::MoveCursor(CursorMove::Up),
            KeyCode::Char('l') | KeyCode::Right => Action::MoveCursor(CursorMove::Right),
            KeyCode::Char('0') => Action::MoveCursor(CursorMove::LineStart),
            KeyCode::Char('$') => Action::MoveCursor(CursorMove::LineEnd),
            KeyCode::Char('^') => Action::MoveCursor(CursorMove::FirstNonBlank),
            KeyCode::Char('w') => Action::MoveCursor(CursorMove::WordForward),
            KeyCode::Char('b') => Action::MoveCursor(CursorMove::WordBack),
            KeyCode::Char('e') => Action::MoveCursor(CursorMove::WordEnd),
            KeyCode::Char('W') => Action::MoveCursor(CursorMove::WordForwardBig),
            KeyCode::Char('B') => Action::MoveCursor(CursorMove::WordBackBig),
            KeyCode::Char('E') => Action::MoveCursor(CursorMove::WordEndBig),
            KeyCode::Char('G') => Action::MoveCursor(CursorMove::Bottom),
            KeyCode::PageDown => Action::MoveCursor(CursorMove::PageDown),
            KeyCode::PageUp => Action::MoveCursor(CursorMove::PageUp),
            KeyCode::Char('v') => Action::SplitPane(Direction::Vertical),
            KeyCode::Char('s') => Action::SplitPane(Direction::Horizontal),
            KeyCode::Char('x') => Action::ClosePane,
            KeyCode::Char('/') => {
                *pending = PendingPrefix::SearchInput;
                Action::SearchStart
            }
            KeyCode::Char('n') => Action::SearchNext,
            KeyCode::Char('N') => Action::SearchPrevious,
            KeyCode::Char('y') => Action::YankBlock,
            KeyCode::Char('g') => {
                *pending = PendingPrefix::G;
                Action::Ignore
            }
            KeyCode::Char(']') => {
                *pending = PendingPrefix::BracketClose;
                Action::Ignore
            }
            KeyCode::Char('[') => {
                *pending = PendingPrefix::BracketOpen;
                Action::Ignore
            }
            KeyCode::Char('z') => {
                *pending = PendingPrefix::Z;
                Action::Ignore
            }
            KeyCode::Char('q') => {
                *pending = PendingPrefix::QuickSelect;
                Action::QuickSelect
            }
            _ => Action::Ignore,
        },
    }
}

fn resolve_block_focus(key: &Key) -> Action {
    if key.code == KeyCode::Escape {
        return Action::SwitchMode(Mode::BlockFocus.apply(ModeEvent::Escape));
    }
    Action::ForwardToBlock(encode(key))
}

fn is_entry_chord(key: &Key) -> bool {
    key.ctrl && key.shift && key.code == KeyCode::Space
}

/// Encode a key as the bytes a terminal program expects on the PTY.
fn encode(key: &Key) -> Vec<u8> {
    match key.code {
        KeyCode::Backspace => vec![DELETE],
        KeyCode::Char('\0') => Vec::new(),
        KeyCode::Char(c) => encode_char(c, key.ctrl),
        KeyCode::Delete => ss3(b'P'),
        KeyCode::Down => csi(b'B'),
        KeyCode::End => csi(b'F'),
        KeyCode::Enter => vec![CARRIAGE_RETURN],
        KeyCode::Escape => vec![ESCAPE],
        KeyCode::F(n) => encode_f(n),
        KeyCode::Home => csi(b'H'),
        KeyCode::Insert => csi_param(b'2', b'~'),
        KeyCode::Left => csi(b'D'),
        KeyCode::PageDown => csi_param(b'6', b'~'),
        KeyCode::PageUp => csi_param(b'5', b'~'),
        KeyCode::Right => csi(b'C'),
        KeyCode::Space => vec![b' '],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::Up => csi(b'A'),
    }
}

fn encode_char(c: char, ctrl: bool) -> Vec<u8> {
    if ctrl && c.is_ascii_alphabetic() {
        return vec![(c.to_ascii_uppercase() as u8) & CONTROL_MASK];
    }
    c.to_string().into_bytes()
}

fn csi(final_byte: u8) -> Vec<u8> {
    vec![ESCAPE, b'[', final_byte]
}

fn csi_param(param: u8, final_byte: u8) -> Vec<u8> {
    vec![ESCAPE, b'[', param, final_byte]
}

fn csi_two_params(p1: u8, p2: u8, final_byte: u8) -> Vec<u8> {
    vec![ESCAPE, b'[', p1, b';', p2, final_byte]
}

fn ss3(final_byte: u8) -> Vec<u8> {
    vec![ESCAPE, b'O', final_byte]
}

fn encode_f(n: u8) -> Vec<u8> {
    match n {
        1 => ss3(b'P'),
        2 => ss3(b'Q'),
        3 => ss3(b'R'),
        4 => ss3(b'S'),
        5..=10 => {
            let code = 11 + (n - 5);
            format!("\x1b[{code}~").into_bytes()
        }
        11 => csi_two_params(b'2', b'3', b'~'),
        12 => csi_two_params(b'2', b'4', b'~'),
        _ => Vec::new(),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> Key {
        Key {
            alt: false,
            code,
            ctrl: false,
            shift: false,
        }
    }

    fn resolve_simple(mode: Mode, key: &Key) -> Action {
        let mut pending = PendingPrefix::None;
        resolve(mode, key, &mut pending, false)
    }

    #[test]
    fn test_insert_sends_printable_bytes() {
        assert_eq!(
            resolve_simple(Mode::Insert, &key(KeyCode::Char('a'))),
            Action::SendBytes(vec![b'a'])
        );
    }

    #[test]
    fn test_insert_encodes_control_chars() {
        let ctrl_c = Key {
            ctrl: true,
            ..key(KeyCode::Char('c'))
        };
        assert_eq!(
            resolve_simple(Mode::Insert, &ctrl_c),
            Action::SendBytes(vec![0x03])
        );
    }

    #[test]
    fn test_insert_encodes_arrow_keys() {
        assert_eq!(
            resolve_simple(Mode::Insert, &key(KeyCode::Up)),
            Action::SendBytes(vec![0x1b, b'[', b'A'])
        );
    }

    #[test]
    fn test_esc_enters_normal_unless_fullscreen() {
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve(Mode::Insert, &key(KeyCode::Escape), &mut pending, false),
            Action::SwitchMode(Mode::Normal)
        );
        // A fullscreen app keeps Esc bound to the PTY.
        assert_eq!(
            resolve(Mode::Insert, &key(KeyCode::Escape), &mut pending, true),
            Action::SendBytes(vec![0x1b])
        );
    }

    #[test]
    fn test_entry_chord_switches_to_normal() {
        let chord = Key {
            ctrl: true,
            shift: true,
            ..key(KeyCode::Space)
        };
        assert_eq!(
            resolve_simple(Mode::Insert, &chord),
            Action::SwitchMode(Mode::Normal)
        );
    }

    #[test]
    fn test_normal_navigation_and_mode_exits() {
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('i'))),
            Action::SwitchMode(Mode::Insert)
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Enter)),
            Action::SwitchMode(Mode::BlockFocus)
        );
    }

    #[test]
    fn test_normal_hjkl_moves_the_cursor() {
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('h'))),
            Action::MoveCursor(CursorMove::Left)
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('j'))),
            Action::MoveCursor(CursorMove::Down)
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('k'))),
            Action::MoveCursor(CursorMove::Up)
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('l'))),
            Action::MoveCursor(CursorMove::Right)
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('G'))),
            Action::MoveCursor(CursorMove::Bottom)
        );
    }

    #[test]
    fn test_gg_jumps_to_top_of_buffer() {
        let mut pending = PendingPrefix::None;
        let action = resolve(Mode::Normal, &key(KeyCode::Char('g')), &mut pending, false);
        assert_eq!(action, Action::Ignore);
        assert_eq!(pending, PendingPrefix::G);
        let action = resolve(Mode::Normal, &key(KeyCode::Char('g')), &mut pending, false);
        assert_eq!(action, Action::MoveCursor(CursorMove::Top));
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_ctrl_d_scrolls_half_page() {
        let ctrl_d = Key {
            ctrl: true,
            ..key(KeyCode::Char('d'))
        };
        assert_eq!(
            resolve_simple(Mode::Normal, &ctrl_d),
            Action::MoveCursor(CursorMove::HalfPageDown)
        );
    }

    #[test]
    fn test_normal_ctrl_hjkl_moves_pane_focus() {
        let ctrl_l = Key {
            ctrl: true,
            ..key(KeyCode::Char('l'))
        };
        assert_eq!(
            resolve_simple(Mode::Normal, &ctrl_l),
            Action::FocusPane(FocusDir::Right)
        );
    }

    #[test]
    fn test_block_focus_escape_returns_to_normal() {
        assert_eq!(
            resolve_simple(Mode::BlockFocus, &key(KeyCode::Escape)),
            Action::SwitchMode(Mode::Normal)
        );
    }

    #[test]
    fn test_unbound_normal_key_is_ignored() {
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('z'))),
            Action::Ignore
        );
    }

    #[test]
    fn test_bracket_close_b_navigates_next_block() {
        let mut pending = PendingPrefix::None;
        let action = resolve(Mode::Normal, &key(KeyCode::Char(']')), &mut pending, false);
        assert_eq!(action, Action::Ignore);
        assert_eq!(pending, PendingPrefix::BracketClose);
        let action = resolve(Mode::Normal, &key(KeyCode::Char('b')), &mut pending, false);
        assert_eq!(action, Action::FocusBlock(BlockNav::Next));
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_bracket_open_b_navigates_previous_block() {
        let mut pending = PendingPrefix::None;
        let action = resolve(Mode::Normal, &key(KeyCode::Char('[')), &mut pending, false);
        assert_eq!(action, Action::Ignore);
        assert_eq!(pending, PendingPrefix::BracketOpen);
        let action = resolve(Mode::Normal, &key(KeyCode::Char('b')), &mut pending, false);
        assert_eq!(action, Action::FocusBlock(BlockNav::Previous));
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_bracket_prefix_cancelled_by_ctrl() {
        let mut pending = PendingPrefix::BracketClose;
        let ctrl_h = Key {
            ctrl: true,
            ..key(KeyCode::Char('h'))
        };
        let action = resolve(Mode::Normal, &ctrl_h, &mut pending, false);
        assert_eq!(action, Action::FocusPane(FocusDir::Left));
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_bracket_prefix_with_unknown_key_is_ignored() {
        let mut pending = PendingPrefix::BracketClose;
        let action = resolve(Mode::Normal, &key(KeyCode::Char('x')), &mut pending, false);
        assert_eq!(action, Action::Ignore);
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_slash_starts_search() {
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('/'))),
            Action::SearchStart
        );
    }

    #[test]
    fn test_n_goes_to_next_search_match() {
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('n'))),
            Action::SearchNext
        );
    }

    #[test]
    fn test_y_yanks_block() {
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('y'))),
            Action::YankBlock
        );
    }

    #[test]
    fn test_za_toggles_fold() {
        let mut pending = PendingPrefix::None;
        let action = resolve(Mode::Normal, &key(KeyCode::Char('z')), &mut pending, false);
        assert_eq!(action, Action::Ignore);
        assert_eq!(pending, PendingPrefix::Z);
        let action = resolve(Mode::Normal, &key(KeyCode::Char('a')), &mut pending, false);
        assert_eq!(action, Action::ToggleFold);
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_z_followed_by_unknown_is_ignored() {
        let mut pending = PendingPrefix::Z;
        let action = resolve(Mode::Normal, &key(KeyCode::Char('x')), &mut pending, false);
        assert_eq!(action, Action::Ignore);
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_q_enters_quick_select() {
        let mut pending = PendingPrefix::None;
        let action = resolve(Mode::Normal, &key(KeyCode::Char('q')), &mut pending, false);
        assert_eq!(action, Action::QuickSelect);
        assert_eq!(pending, PendingPrefix::QuickSelect);
    }

    #[test]
    fn test_quick_select_label_jumps() {
        let mut pending = PendingPrefix::QuickSelect;
        let action = resolve(Mode::Normal, &key(KeyCode::Char('s')), &mut pending, false);
        assert_eq!(action, Action::QuickJump('s'));
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_quick_select_escape_cancels() {
        let mut pending = PendingPrefix::QuickSelect;
        let action = resolve(Mode::Normal, &key(KeyCode::Escape), &mut pending, false);
        assert_eq!(action, Action::QuickCancel);
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_quick_select_non_alpha_cancels() {
        let mut pending = PendingPrefix::QuickSelect;
        let action = resolve(Mode::Normal, &key(KeyCode::Enter), &mut pending, false);
        assert_eq!(action, Action::QuickCancel);
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_slash_enters_search_input() {
        let mut pending = PendingPrefix::None;
        let action = resolve(Mode::Normal, &key(KeyCode::Char('/')), &mut pending, false);
        assert_eq!(action, Action::SearchStart);
        assert_eq!(pending, PendingPrefix::SearchInput);
    }

    #[test]
    fn test_search_input_collects_chars() {
        let mut pending = PendingPrefix::SearchInput;
        let action = resolve(Mode::Normal, &key(KeyCode::Char('h')), &mut pending, false);
        assert_eq!(action, Action::SearchChar('h'));
        assert_eq!(pending, PendingPrefix::SearchInput);
        let action = resolve(Mode::Normal, &key(KeyCode::Char('i')), &mut pending, false);
        assert_eq!(action, Action::SearchChar('i'));
        assert_eq!(pending, PendingPrefix::SearchInput);
    }

    #[test]
    fn test_search_input_enter_executes() {
        let mut pending = PendingPrefix::SearchInput;
        let action = resolve(Mode::Normal, &key(KeyCode::Enter), &mut pending, false);
        assert_eq!(action, Action::SearchExecute);
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_search_input_escape_cancels() {
        let mut pending = PendingPrefix::SearchInput;
        let action = resolve(Mode::Normal, &key(KeyCode::Escape), &mut pending, false);
        assert_eq!(action, Action::SearchCancel);
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_search_input_backspace() {
        let mut pending = PendingPrefix::SearchInput;
        let action = resolve(Mode::Normal, &key(KeyCode::Backspace), &mut pending, false);
        assert_eq!(action, Action::SearchBackspace);
        assert_eq!(pending, PendingPrefix::SearchInput);
    }
}
