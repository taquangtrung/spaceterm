//! Keymap: resolve a key event into an [`Action`], given the pane's [`Mode`].
//!
//! In Insert, keys encode to terminal bytes for the PTY (the entry chord is the
//! one exception). In Normal, SpaceTerm intercepts keys as navigation/layout actions.
//! In Block-focus, keys forward to the block until `Esc`. Most bindings are
//! built in, but the window-management chords (split, close, focus) are
//! configurable through a [`WindowKeymap`]; see [`resolve_with`].

use std::collections::{HashMap, HashSet};

use super::layout::{Direction, FocusDir};
use super::mode::{Mode, ModeEvent};

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
    /// Close the focused pane (current pane).
    ClosePane,
    CloseOtherPanes,
    /// Close a tab: the active one (`None`) or the Nth tab, 1-based (`Some(n)`).
    CloseTab(Option<usize>),
    DeleteCharForward,
    DeleteLine,
    DeleteToLineEnd,
    DeleteToLineStart,
    DeleteWordBack,
    DeleteWordForward,
    EnterVisual(VisualKind),
    /// A char-search within the current line (`f`/`F`/`t`/`T`).
    FindChar(FindChar),
    /// Repeat the last char-search (`;`); `reverse` flips its direction (`,`).
    FindRepeat {
        reverse: bool,
    },
    FocusBlock(BlockNav),
    FocusPane(FocusDir),
    ForwardToBlock(Vec<u8>),
    /// Switch to tab number `n` (1-based).
    GotoTab(usize),
    Ignore,
    MoveCursor(CursorMove),
    NewTab,
    NextTab,
    Paste,
    PrevTab,
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
    YankSelection,
}

/// A vim char-search within the current line. `forward` is `f`/`t`; `till`
/// (`t`/`T`) stops one cell short of the target instead of on it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FindChar {
    pub ch: char,
    pub forward: bool,
    pub till: bool,
}

impl FindChar {
    /// The same search with its direction flipped, used to repeat `f`/`t` the
    /// opposite way on `,`.
    pub fn reversed(self) -> Self {
        Self {
            forward: !self.forward,
            ..self
        }
    }
}

/// Whether a Visual selection spans characters or whole lines (vim `v` vs `V`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VisualKind {
    Char,
    Line,
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
    CtrlW,
    Delete,
    /// Awaiting the target char of a backward find (`F`).
    FindBackward,
    /// Awaiting the target char of a forward find (`f`).
    FindForward,
    G,
    None,
    QuickSelect,
    SearchInput,
    /// Awaiting the target char of a backward till (`T`).
    TillBackward,
    /// Awaiting the target char of a forward till (`t`).
    TillForward,
    Z,
}

/// A window-management command: the configurable subset of Normal-mode bindings.
/// Each maps to a layout-affecting [`Action`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WindowAction {
    Close,
    CloseOthers,
    FocusDown,
    FocusLeft,
    FocusRight,
    FocusUp,
    SplitHorizontal,
    SplitVertical,
}

/// User-configurable window-management key bindings. A binding is either a
/// direct chord (e.g. `Ctrl-h` to focus left) or a two-key sequence opened by
/// the `leader` (e.g. `Ctrl-w` then `v` to split). Built from defaults and
/// overlaid with config via [`WindowKeymap::from_config`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowKeymap {
    /// Direct chords: the full key (with modifiers) triggers the action.
    direct: Vec<(Key, WindowAction)>,
    /// The chord that opens a two-key window-command sequence.
    leader: Key,
    /// Keys that select an action when pressed after the `leader`, matched by
    /// key code alone (modifiers on the follow key are ignored, as in Vim).
    sequence: Vec<(KeyCode, WindowAction)>,
}

// ========================================================================
// Constants
// ========================================================================

const CONTROL_MASK: u8 = 0x1f;
const DELETE: u8 = 0x7f;
const ESCAPE: u8 = 0x1b;
const CARRIAGE_RETURN: u8 = b'\r';

// Kitty keyboard protocol: functional-key codepoints.
// https://sw.kovidgoyal.net/kitty/keyboard-protocol/#functional-key-definitions
const KP_INSERT: u32 = 57348;
const KP_DELETE: u32 = 57349;
const KP_LEFT: u32 = 57350;
const KP_RIGHT: u32 = 57351;
const KP_UP: u32 = 57352;
const KP_DOWN: u32 = 57353;
const KP_PAGE_UP: u32 = 57354;
const KP_PAGE_DOWN: u32 = 57355;
const KP_HOME: u32 = 57356;
const KP_END: u32 = 57357;
const KP_F1: u32 = 57364;

// ========================================================================
// Window keymap
// ========================================================================

impl WindowAction {
    /// The dispatched [`Action`] this window command produces.
    fn to_action(self) -> Action {
        match self {
            WindowAction::Close => Action::ClosePane,
            WindowAction::CloseOthers => Action::CloseOtherPanes,
            WindowAction::FocusDown => Action::FocusPane(FocusDir::Down),
            WindowAction::FocusLeft => Action::FocusPane(FocusDir::Left),
            WindowAction::FocusRight => Action::FocusPane(FocusDir::Right),
            WindowAction::FocusUp => Action::FocusPane(FocusDir::Up),
            WindowAction::SplitHorizontal => Action::SplitPane(Direction::Horizontal),
            WindowAction::SplitVertical => Action::SplitPane(Direction::Vertical),
        }
    }

    /// Parse a config action name (matching the command-palette names).
    fn from_name(name: &str) -> Option<WindowAction> {
        Some(match name {
            "close_pane" => WindowAction::Close,
            "close_other_panes" => WindowAction::CloseOthers,
            "focus_down" => WindowAction::FocusDown,
            "focus_left" => WindowAction::FocusLeft,
            "focus_right" => WindowAction::FocusRight,
            "focus_up" => WindowAction::FocusUp,
            "split_horizontal" => WindowAction::SplitHorizontal,
            "split_vertical" => WindowAction::SplitVertical,
            _ => return None,
        })
    }
}

impl WindowKeymap {
    /// Build a keymap from the `window` keybindings block, falling back to the
    /// built-in defaults. A configured binding replaces every default chord for
    /// that same action, so rebinding `split_vertical` drops the default
    /// `Ctrl-w v`; actions left unmentioned keep their defaults.
    pub fn from_config(bindings: Option<&HashMap<String, String>>) -> Self {
        let mut keymap = Self::default();
        let Some(bindings) = bindings else {
            return keymap;
        };

        let parsed: Vec<(Vec<Key>, WindowAction)> = bindings
            .iter()
            .filter_map(|(spec, name)| {
                Some((parse_chord_sequence(spec)?, WindowAction::from_name(name)?))
            })
            .collect();
        if parsed.is_empty() {
            return keymap;
        }

        let rebound: HashSet<WindowAction> = parsed.iter().map(|(_, action)| *action).collect();
        keymap
            .direct
            .retain(|(_, action)| !rebound.contains(action));
        keymap
            .sequence
            .retain(|(_, action)| !rebound.contains(action));

        for (keys, action) in parsed {
            match keys.as_slice() {
                [single] => keymap.direct.push((single.clone(), action)),
                [leader, follow] => {
                    keymap.leader = leader.clone();
                    keymap.sequence.push((follow.code, action));
                }
                _ => {}
            }
        }
        keymap
    }

    /// The action a direct chord triggers, if any.
    fn direct_action(&self, key: &Key) -> Option<WindowAction> {
        self.direct
            .iter()
            .find(|(chord, _)| chord == key)
            .map(|(_, action)| *action)
    }

    /// The action the follow key selects after the leader, if any.
    fn sequence_action(&self, code: KeyCode) -> Option<WindowAction> {
        self.sequence
            .iter()
            .find(|(follow, _)| *follow == code)
            .map(|(_, action)| *action)
    }
}

impl Default for WindowKeymap {
    /// The built-in Vim-style window bindings: `Ctrl-w` leads `v`/`s`/`S`/`c`/
    /// `q`/`o` for split and close, and `Ctrl-h`/`j`/`k`/`l` focus directly.
    fn default() -> Self {
        let ctrl = |c: char| Key {
            alt: false,
            code: KeyCode::Char(c),
            ctrl: true,
            shift: false,
        };
        Self {
            direct: vec![
                (ctrl('h'), WindowAction::FocusLeft),
                (ctrl('j'), WindowAction::FocusDown),
                (ctrl('k'), WindowAction::FocusUp),
                (ctrl('l'), WindowAction::FocusRight),
            ],
            leader: ctrl('w'),
            sequence: vec![
                (KeyCode::Char('v'), WindowAction::SplitVertical),
                (KeyCode::Char('s'), WindowAction::SplitHorizontal),
                (KeyCode::Char('S'), WindowAction::SplitHorizontal),
                (KeyCode::Char('c'), WindowAction::Close),
                (KeyCode::Char('q'), WindowAction::Close),
                (KeyCode::Char('o'), WindowAction::CloseOthers),
            ],
        }
    }
}

/// Parse a key-binding spec of one or two chords (e.g. `"Ctrl-h"` or
/// `"Ctrl-w v"`) into its keys. Returns `None` on any unrecognized chord or a
/// length outside 1..=2.
fn parse_chord_sequence(spec: &str) -> Option<Vec<Key>> {
    let keys = spec
        .split_whitespace()
        .map(parse_chord)
        .collect::<Option<Vec<Key>>>()?;
    if (1..=2).contains(&keys.len()) {
        Some(keys)
    } else {
        None
    }
}

/// Parse a single chord like `"Ctrl-Shift-Space"` into a [`Key`]. Modifiers are
/// `-`-separated and case-insensitive; the final segment is the key name.
fn parse_chord(token: &str) -> Option<Key> {
    let parts: Vec<&str> = token.split('-').collect();
    let (mods, name) = parts.split_at(parts.len().checked_sub(1)?);

    let mut key = Key {
        alt: false,
        code: parse_key_code(name[0])?,
        ctrl: false,
        shift: false,
    };
    for modifier in mods {
        match modifier.to_ascii_lowercase().as_str() {
            "alt" | "option" | "meta" => key.alt = true,
            "ctrl" | "control" => key.ctrl = true,
            "shift" => key.shift = true,
            _ => return None,
        }
    }
    Some(key)
}

/// Parse a key name into a [`KeyCode`]: a single character is a `Char`, and
/// named keys (`Space`, `Enter`, `F1`, ...) map case-insensitively.
fn parse_key_code(name: &str) -> Option<KeyCode> {
    let mut chars = name.chars();
    let first = chars.next()?;
    if chars.next().is_none() {
        return Some(KeyCode::Char(first));
    }
    Some(match name.to_ascii_lowercase().as_str() {
        "backspace" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "down" => KeyCode::Down,
        "end" => KeyCode::End,
        "enter" | "return" => KeyCode::Enter,
        "escape" | "esc" => KeyCode::Escape,
        "home" => KeyCode::Home,
        "insert" => KeyCode::Insert,
        "left" => KeyCode::Left,
        "pagedown" => KeyCode::PageDown,
        "pageup" => KeyCode::PageUp,
        "right" => KeyCode::Right,
        "space" => KeyCode::Space,
        "tab" => KeyCode::Tab,
        "up" => KeyCode::Up,
        other => return parse_function_key(other),
    })
}

/// Parse an `f1`..`f12` function-key name into [`KeyCode::F`].
fn parse_function_key(name: &str) -> Option<KeyCode> {
    let number: u8 = name.strip_prefix('f')?.parse().ok()?;
    (1..=12).contains(&number).then_some(KeyCode::F(number))
}

// ========================================================================
// Resolution
// ========================================================================

/// Resolve a key in the given mode using the default window keymap.
/// `pending` tracks multi-key sequences (e.g. `]b`); it is updated in place.
/// `flags` is the active Kitty keyboard protocol flags for the focused pane
/// (0 = legacy xterm encoding).
pub fn resolve(mode: Mode, key: &Key, pending: &mut PendingPrefix, flags: u32) -> Action {
    resolve_with(mode, key, pending, &WindowKeymap::default(), flags)
}

/// Like [`resolve`], but using the supplied window keymap for the configurable
/// split/close/focus bindings.
pub fn resolve_with(
    mode: Mode,
    key: &Key,
    pending: &mut PendingPrefix,
    window: &WindowKeymap,
    flags: u32,
) -> Action {
    match mode {
        Mode::Insert => resolve_insert(key, flags),
        Mode::Normal => resolve_normal(key, pending, window),
        Mode::Visual => resolve_visual(key, pending),
        Mode::BlockFocus => resolve_block_focus(key, flags),
    }
}

fn resolve_insert(key: &Key, flags: u32) -> Action {
    if is_entry_chord(key) {
        return Action::SwitchMode(Mode::Insert.apply(ModeEvent::EnterNormal));
    }
    // Escape always goes to the PTY. Context-aware mode switching (e.g. at the
    // shell prompt) is handled one layer up in the application event loop.
    Action::SendBytes(encode(key, flags))
}

/// Build the char-search action for the key that follows `f`/`F`/`t`/`T`. A
/// printable character is the search target; anything else cancels the search.
fn find_char_action(key: &Key, forward: bool, till: bool) -> Action {
    match key.code {
        KeyCode::Char(ch) => Action::FindChar(FindChar { ch, forward, till }),
        _ => Action::Ignore,
    }
}

fn resolve_normal(key: &Key, pending: &mut PendingPrefix, window: &WindowKeymap) -> Action {
    // A window-command sequence is open: the follow key selects its action
    // (matched by code, so `Ctrl-w v` and `Ctrl-w Ctrl-v` both split).
    if *pending == PendingPrefix::CtrlW {
        *pending = PendingPrefix::None;
        return window
            .sequence_action(key.code)
            .map_or(Action::Ignore, WindowAction::to_action);
    }

    // The configurable window leader opens that sequence.
    if key == &window.leader {
        *pending = PendingPrefix::CtrlW;
        return Action::Ignore;
    }

    // A direct window chord (Ctrl-h/j/k/l focus motions by default).
    if let Some(action) = window.direct_action(key) {
        *pending = PendingPrefix::None;
        return action.to_action();
    }

    if key.ctrl {
        // Non-window control chords resolve immediately and clear any prefix.
        *pending = PendingPrefix::None;
        return match key.code {
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
        // The leader sequence is resolved at the top of this function.
        PendingPrefix::CtrlW => Action::Ignore,
        PendingPrefix::Delete => match key.code {
            KeyCode::Char('d') => Action::DeleteLine,
            KeyCode::Char('w') => Action::DeleteWordForward,
            KeyCode::Char('b') => Action::DeleteWordBack,
            KeyCode::Char('$') => Action::DeleteToLineEnd,
            KeyCode::Char('0') => Action::DeleteToLineStart,
            _ => Action::Ignore,
        },
        PendingPrefix::FindForward => find_char_action(key, true, false),
        PendingPrefix::FindBackward => find_char_action(key, false, false),
        PendingPrefix::TillForward => find_char_action(key, true, true),
        PendingPrefix::TillBackward => find_char_action(key, false, true),
        PendingPrefix::G => match key.code {
            KeyCode::Char('g') => Action::MoveCursor(CursorMove::Top),
            KeyCode::Char('t') => Action::NextTab,
            KeyCode::Char('T') => Action::PrevTab,
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
            KeyCode::Char('f') => {
                *pending = PendingPrefix::FindForward;
                Action::Ignore
            }
            KeyCode::Char('F') => {
                *pending = PendingPrefix::FindBackward;
                Action::Ignore
            }
            KeyCode::Char('t') => {
                *pending = PendingPrefix::TillForward;
                Action::Ignore
            }
            KeyCode::Char('T') => {
                *pending = PendingPrefix::TillBackward;
                Action::Ignore
            }
            KeyCode::Char(';') => Action::FindRepeat { reverse: false },
            KeyCode::Char(',') => Action::FindRepeat { reverse: true },
            KeyCode::Char('v') => Action::EnterVisual(VisualKind::Char),
            KeyCode::Char('V') => Action::EnterVisual(VisualKind::Line),
            KeyCode::Char('p') => Action::Paste,
            KeyCode::Char('x') => Action::DeleteCharForward,
            KeyCode::Char('D') => Action::DeleteToLineEnd,
            KeyCode::Char('d') => {
                *pending = PendingPrefix::Delete;
                Action::Ignore
            }
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

/// Resolve a key in Visual mode: the same motions as Normal extend the
/// selection, `y` yanks it, and `v`/`V`/`Esc` leave Visual.
fn resolve_visual(key: &Key, pending: &mut PendingPrefix) -> Action {
    if key.ctrl {
        *pending = PendingPrefix::None;
        return match key.code {
            KeyCode::Char('d') => Action::MoveCursor(CursorMove::HalfPageDown),
            KeyCode::Char('u') => Action::MoveCursor(CursorMove::HalfPageUp),
            _ => Action::Ignore,
        };
    }

    let prev = *pending;
    *pending = PendingPrefix::None;

    match prev {
        PendingPrefix::G => {
            return match key.code {
                KeyCode::Char('g') => Action::MoveCursor(CursorMove::Top),
                _ => Action::Ignore,
            };
        }
        PendingPrefix::FindForward => return find_char_action(key, true, false),
        PendingPrefix::FindBackward => return find_char_action(key, false, false),
        PendingPrefix::TillForward => return find_char_action(key, true, true),
        PendingPrefix::TillBackward => return find_char_action(key, false, true),
        _ => {}
    }

    match key.code {
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
        KeyCode::Char('g') => {
            *pending = PendingPrefix::G;
            Action::Ignore
        }
        KeyCode::Char('f') => {
            *pending = PendingPrefix::FindForward;
            Action::Ignore
        }
        KeyCode::Char('F') => {
            *pending = PendingPrefix::FindBackward;
            Action::Ignore
        }
        KeyCode::Char('t') => {
            *pending = PendingPrefix::TillForward;
            Action::Ignore
        }
        KeyCode::Char('T') => {
            *pending = PendingPrefix::TillBackward;
            Action::Ignore
        }
        KeyCode::Char(';') => Action::FindRepeat { reverse: false },
        KeyCode::Char(',') => Action::FindRepeat { reverse: true },
        KeyCode::Char('y') => Action::YankSelection,
        KeyCode::Char('v') => Action::EnterVisual(VisualKind::Char),
        KeyCode::Char('V') => Action::EnterVisual(VisualKind::Line),
        KeyCode::Char('i') => Action::SwitchMode(Mode::Visual.apply(ModeEvent::ToInsert)),
        KeyCode::Escape => Action::SwitchMode(Mode::Visual.apply(ModeEvent::Escape)),
        _ => Action::Ignore,
    }
}

fn resolve_block_focus(key: &Key, flags: u32) -> Action {
    if key.code == KeyCode::Escape {
        return Action::SwitchMode(Mode::BlockFocus.apply(ModeEvent::Escape));
    }
    Action::ForwardToBlock(encode(key, flags))
}

fn is_entry_chord(key: &Key) -> bool {
    key.ctrl && key.shift && key.code == KeyCode::Space
}

/// Encode a key as the bytes a terminal program expects on the PTY.
/// `flags` is the active Kitty keyboard protocol bitmask for the pane
/// (0 = legacy xterm encoding).
fn encode(key: &Key, flags: u32) -> Vec<u8> {
    if flags != 0 {
        encode_kitty(key)
    } else {
        encode_xterm(key)
    }
}

/// Encode a key-release event for the Kitty protocol.
/// Returns bytes only when `flags & 2 != 0` (bit 1: report event types).
/// Release sequences use `: 3` as the event-type sub-field:
///   `CSI codepoint :: 3 u`  (no modifier)
///   `CSI codepoint ; modifier : 3 u`  (with modifier)
pub fn encode_release(key: &Key, flags: u32) -> Vec<u8> {
    if flags & 2 == 0 {
        return Vec::new();
    }
    encode_kitty_release(key)
}

// ---- xterm baseline encoding ------------------------------------------------

/// xterm modifier byte: 1 + shift + 2*alt + 4*ctrl. Value 1 means no modifier.
fn xterm_modifier(key: &Key) -> u8 {
    1 + (key.shift as u8) + 2 * (key.alt as u8) + 4 * (key.ctrl as u8)
}

/// Prepend an ESC byte (Alt prefix convention).
fn esc_prefix(mut bytes: Vec<u8>) -> Vec<u8> {
    bytes.insert(0, ESCAPE);
    bytes
}

/// Navigation key: bare CSI when no modifier, `\e[1;NX` with one.
fn nav_csi_xterm(final_byte: u8, m: u8) -> Vec<u8> {
    if m == 1 {
        csi(final_byte)
    } else {
        format!("\x1b[1;{m}{}", final_byte as char).into_bytes()
    }
}

/// Tilde-form key: `\e[k~` bare, `\e[k;N~` with modifier.
fn tilde_xterm(param: u8, m: u8) -> Vec<u8> {
    if m == 1 {
        csi_param(param, b'~')
    } else {
        format!("\x1b[{param};{m}~").into_bytes()
    }
}

/// xterm encoding for function keys F1-F12 with optional modifier.
fn encode_f_xterm(n: u8, m: u8) -> Vec<u8> {
    match n {
        1 => {
            if m > 1 { format!("\x1b[1;{m}P").into_bytes() } else { ss3(b'P') }
        }
        2 => {
            if m > 1 { format!("\x1b[1;{m}Q").into_bytes() } else { ss3(b'Q') }
        }
        3 => {
            if m > 1 { format!("\x1b[1;{m}R").into_bytes() } else { ss3(b'R') }
        }
        4 => {
            if m > 1 { format!("\x1b[1;{m}S").into_bytes() } else { ss3(b'S') }
        }
        5 => tilde_xterm(15, m),
        6 => tilde_xterm(17, m),
        7 => tilde_xterm(18, m),
        8 => tilde_xterm(19, m),
        9 => tilde_xterm(20, m),
        10 => tilde_xterm(21, m),
        11 => tilde_xterm(23, m),
        12 => tilde_xterm(24, m),
        _ => Vec::new(),
    }
}

fn encode_xterm(key: &Key) -> Vec<u8> {
    let m = xterm_modifier(key);
    let bytes = match key.code {
        KeyCode::Backspace => vec![DELETE],
        KeyCode::Char('\0') => return Vec::new(),
        KeyCode::Char(c) => {
            if key.ctrl && c.is_ascii_alphabetic() {
                vec![(c.to_ascii_uppercase() as u8) & CONTROL_MASK]
            } else {
                c.to_string().into_bytes()
            }
        }
        KeyCode::Delete => tilde_xterm(3, m),
        KeyCode::Down => nav_csi_xterm(b'B', m),
        KeyCode::End => {
            if m > 1 { format!("\x1b[1;{m}F").into_bytes() } else { csi(b'F') }
        }
        KeyCode::Enter => vec![CARRIAGE_RETURN],
        KeyCode::Escape => vec![ESCAPE],
        KeyCode::F(n) => encode_f_xterm(n, m),
        KeyCode::Home => {
            if m > 1 { format!("\x1b[1;{m}H").into_bytes() } else { csi(b'H') }
        }
        KeyCode::Insert => tilde_xterm(2, m),
        KeyCode::Left => nav_csi_xterm(b'D', m),
        KeyCode::PageDown => tilde_xterm(6, m),
        KeyCode::PageUp => tilde_xterm(5, m),
        KeyCode::Right => nav_csi_xterm(b'C', m),
        KeyCode::Space => vec![b' '],
        KeyCode::Tab => {
            if key.shift {
                vec![ESCAPE, b'[', b'Z'] // backtab / reverse-tab
            } else {
                vec![b'\t']
            }
        }
        KeyCode::Up => nav_csi_xterm(b'A', m),
    };
    // Alt prefix: prepend ESC (never on bare Escape to avoid double-ESC).
    if key.alt && !matches!(key.code, KeyCode::Escape | KeyCode::Char('\0')) {
        esc_prefix(bytes)
    } else {
        bytes
    }
}

// ---- Kitty keyboard protocol encoding ---------------------------------------

/// Kitty modifier value: 1 + shift + 2*alt + 4*ctrl. Value 1 means no modifier.
fn kitty_modifier(key: &Key) -> u32 {
    1 + (key.shift as u32) + 2 * (key.alt as u32) + 4 * (key.ctrl as u32)
}

/// `CSI codepoint u` or `CSI codepoint ; modifier u` (omit `;1`).
fn kitty_csi(codepoint: u32, modifier: u32) -> Vec<u8> {
    if modifier == 1 {
        format!("\x1b[{codepoint}u").into_bytes()
    } else {
        format!("\x1b[{codepoint};{modifier}u").into_bytes()
    }
}

/// Release variant: `CSI codepoint :: 3 u` (no modifier) or
/// `CSI codepoint ; modifier : 3 u` (with modifier).
fn kitty_csi_release(codepoint: u32, modifier: u32) -> Vec<u8> {
    if modifier == 1 {
        format!("\x1b[{codepoint}::3u").into_bytes()
    } else {
        format!("\x1b[{codepoint};{modifier}:3u").into_bytes()
    }
}

/// Kitty release encoding: same key mapping as `encode_kitty` but with the
/// `: 3` event-type suffix. Keys that map to raw bytes on press (bare chars,
/// bare Tab, bare Enter, etc.) get full CSI sequences on release so the app
/// can distinguish press from release.
fn encode_kitty_release(key: &Key) -> Vec<u8> {
    let m = kitty_modifier(key);
    match key.code {
        KeyCode::Char('\0') => Vec::new(),
        KeyCode::Char(c) => kitty_csi_release(base_codepoint(c, key.shift), m),
        KeyCode::Space => kitty_csi_release(32, m),
        KeyCode::Tab => kitty_csi_release(9, m),
        KeyCode::Enter => kitty_csi_release(13, m),
        KeyCode::Escape => kitty_csi_release(27, m),
        KeyCode::Backspace => kitty_csi_release(127, m),
        KeyCode::Insert => kitty_csi_release(KP_INSERT, m),
        KeyCode::Delete => kitty_csi_release(KP_DELETE, m),
        KeyCode::Left => kitty_csi_release(KP_LEFT, m),
        KeyCode::Right => kitty_csi_release(KP_RIGHT, m),
        KeyCode::Up => kitty_csi_release(KP_UP, m),
        KeyCode::Down => kitty_csi_release(KP_DOWN, m),
        KeyCode::PageUp => kitty_csi_release(KP_PAGE_UP, m),
        KeyCode::PageDown => kitty_csi_release(KP_PAGE_DOWN, m),
        KeyCode::Home => kitty_csi_release(KP_HOME, m),
        KeyCode::End => kitty_csi_release(KP_END, m),
        KeyCode::F(n @ 1..=12) => kitty_csi_release(KP_F1 + (n as u32 - 1), m),
        KeyCode::F(_) => Vec::new(),
    }
}

/// The base Unicode codepoint of a character key (lowercase for ASCII alpha
/// so Ctrl+Shift+a uses codepoint 97, not 65).
fn base_codepoint(c: char, shift: bool) -> u32 {
    if shift && c.is_ascii_uppercase() {
        c.to_ascii_lowercase() as u32
    } else {
        c as u32
    }
}

fn encode_kitty(key: &Key) -> Vec<u8> {
    let m = kitty_modifier(key);
    match key.code {
        KeyCode::Char('\0') => Vec::new(),
        KeyCode::Char(c) => {
            // No modifier or lone Shift: send raw UTF-8. Shift is already
            // encoded in the char winit provides ('A' for Shift+a).
            if m == 1 || m == 2 {
                return c.to_string().into_bytes();
            }
            kitty_csi(base_codepoint(c, key.shift), m)
        }
        KeyCode::Space => {
            if m == 1 { vec![b' '] } else { kitty_csi(32, m) }
        }
        KeyCode::Tab => {
            if m == 1 {
                vec![b'\t']
            } else if key.shift && !key.ctrl && !key.alt {
                vec![ESCAPE, b'[', b'Z'] // backtab
            } else {
                kitty_csi(9, m)
            }
        }
        KeyCode::Enter => {
            if m == 1 { vec![CARRIAGE_RETURN] } else { kitty_csi(13, m) }
        }
        KeyCode::Escape => {
            if m == 1 { vec![ESCAPE] } else { kitty_csi(27, m) }
        }
        KeyCode::Backspace => {
            if m == 1 { vec![DELETE] } else { kitty_csi(127, m) }
        }
        KeyCode::Insert => kitty_csi(KP_INSERT, m),
        KeyCode::Delete => kitty_csi(KP_DELETE, m),
        KeyCode::Left => kitty_csi(KP_LEFT, m),
        KeyCode::Right => kitty_csi(KP_RIGHT, m),
        KeyCode::Up => kitty_csi(KP_UP, m),
        KeyCode::Down => kitty_csi(KP_DOWN, m),
        KeyCode::PageUp => kitty_csi(KP_PAGE_UP, m),
        KeyCode::PageDown => kitty_csi(KP_PAGE_DOWN, m),
        KeyCode::Home => kitty_csi(KP_HOME, m),
        KeyCode::End => kitty_csi(KP_END, m),
        KeyCode::F(n @ 1..=12) => kitty_csi(KP_F1 + (n as u32 - 1), m),
        KeyCode::F(_) => Vec::new(),
    }
}

// ---- Low-level sequence builders --------------------------------------------

fn csi(final_byte: u8) -> Vec<u8> {
    vec![ESCAPE, b'[', final_byte]
}

fn csi_param(param: u8, final_byte: u8) -> Vec<u8> {
    vec![ESCAPE, b'[', param, final_byte]
}

fn ss3(final_byte: u8) -> Vec<u8> {
    vec![ESCAPE, b'O', final_byte]
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
        resolve(mode, key, &mut pending, 0)
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
    fn test_esc_is_always_sent_to_pty_in_insert_mode() {
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve(Mode::Insert, &key(KeyCode::Escape), &mut pending, 0),
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
        let action = resolve(Mode::Normal, &key(KeyCode::Char('g')), &mut pending, 0);
        assert_eq!(action, Action::Ignore);
        assert_eq!(pending, PendingPrefix::G);
        let action = resolve(Mode::Normal, &key(KeyCode::Char('g')), &mut pending, 0);
        assert_eq!(action, Action::MoveCursor(CursorMove::Top));
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_char_search_sets_prefix_then_resolves_target() {
        let mut pending = PendingPrefix::None;
        // `t` opens a forward-till search rather than acting immediately.
        let action = resolve(Mode::Normal, &key(KeyCode::Char('t')), &mut pending, 0);
        assert_eq!(action, Action::Ignore);
        assert_eq!(pending, PendingPrefix::TillForward);
        // The next key is the search target.
        let action = resolve(Mode::Normal, &key(KeyCode::Char('x')), &mut pending, 0);
        assert_eq!(
            action,
            Action::FindChar(FindChar {
                ch: 'x',
                forward: true,
                till: true,
            })
        );
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_find_repeat_keys() {
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char(';'))),
            Action::FindRepeat { reverse: false }
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char(','))),
            Action::FindRepeat { reverse: true }
        );
    }

    #[test]
    fn test_gt_switches_tabs() {
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char('g')), &mut pending, 0),
            Action::Ignore
        );
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char('t')), &mut pending, 0),
            Action::NextTab
        );
        resolve(Mode::Normal, &key(KeyCode::Char('g')), &mut pending, 0);
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char('T')), &mut pending, 0),
            Action::PrevTab
        );
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
        let action = resolve(Mode::Normal, &key(KeyCode::Char(']')), &mut pending, 0);
        assert_eq!(action, Action::Ignore);
        assert_eq!(pending, PendingPrefix::BracketClose);
        let action = resolve(Mode::Normal, &key(KeyCode::Char('b')), &mut pending, 0);
        assert_eq!(action, Action::FocusBlock(BlockNav::Next));
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_bracket_open_b_navigates_previous_block() {
        let mut pending = PendingPrefix::None;
        let action = resolve(Mode::Normal, &key(KeyCode::Char('[')), &mut pending, 0);
        assert_eq!(action, Action::Ignore);
        assert_eq!(pending, PendingPrefix::BracketOpen);
        let action = resolve(Mode::Normal, &key(KeyCode::Char('b')), &mut pending, 0);
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
        let action = resolve(Mode::Normal, &ctrl_h, &mut pending, 0);
        assert_eq!(action, Action::FocusPane(FocusDir::Left));
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_bracket_prefix_with_unknown_key_is_ignored() {
        let mut pending = PendingPrefix::BracketClose;
        let action = resolve(Mode::Normal, &key(KeyCode::Char('x')), &mut pending, 0);
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
        let action = resolve(Mode::Normal, &key(KeyCode::Char('z')), &mut pending, 0);
        assert_eq!(action, Action::Ignore);
        assert_eq!(pending, PendingPrefix::Z);
        let action = resolve(Mode::Normal, &key(KeyCode::Char('a')), &mut pending, 0);
        assert_eq!(action, Action::ToggleFold);
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_z_followed_by_unknown_is_ignored() {
        let mut pending = PendingPrefix::Z;
        let action = resolve(Mode::Normal, &key(KeyCode::Char('x')), &mut pending, 0);
        assert_eq!(action, Action::Ignore);
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_q_enters_quick_select() {
        let mut pending = PendingPrefix::None;
        let action = resolve(Mode::Normal, &key(KeyCode::Char('q')), &mut pending, 0);
        assert_eq!(action, Action::QuickSelect);
        assert_eq!(pending, PendingPrefix::QuickSelect);
    }

    #[test]
    fn test_quick_select_label_jumps() {
        let mut pending = PendingPrefix::QuickSelect;
        let action = resolve(Mode::Normal, &key(KeyCode::Char('s')), &mut pending, 0);
        assert_eq!(action, Action::QuickJump('s'));
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_quick_select_escape_cancels() {
        let mut pending = PendingPrefix::QuickSelect;
        let action = resolve(Mode::Normal, &key(KeyCode::Escape), &mut pending, 0);
        assert_eq!(action, Action::QuickCancel);
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_quick_select_non_alpha_cancels() {
        let mut pending = PendingPrefix::QuickSelect;
        let action = resolve(Mode::Normal, &key(KeyCode::Enter), &mut pending, 0);
        assert_eq!(action, Action::QuickCancel);
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_slash_enters_search_input() {
        let mut pending = PendingPrefix::None;
        let action = resolve(Mode::Normal, &key(KeyCode::Char('/')), &mut pending, 0);
        assert_eq!(action, Action::SearchStart);
        assert_eq!(pending, PendingPrefix::SearchInput);
    }

    #[test]
    fn test_search_input_collects_chars() {
        let mut pending = PendingPrefix::SearchInput;
        let action = resolve(Mode::Normal, &key(KeyCode::Char('h')), &mut pending, 0);
        assert_eq!(action, Action::SearchChar('h'));
        assert_eq!(pending, PendingPrefix::SearchInput);
        let action = resolve(Mode::Normal, &key(KeyCode::Char('i')), &mut pending, 0);
        assert_eq!(action, Action::SearchChar('i'));
        assert_eq!(pending, PendingPrefix::SearchInput);
    }

    #[test]
    fn test_search_input_enter_executes() {
        let mut pending = PendingPrefix::SearchInput;
        let action = resolve(Mode::Normal, &key(KeyCode::Enter), &mut pending, 0);
        assert_eq!(action, Action::SearchExecute);
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_search_input_escape_cancels() {
        let mut pending = PendingPrefix::SearchInput;
        let action = resolve(Mode::Normal, &key(KeyCode::Escape), &mut pending, 0);
        assert_eq!(action, Action::SearchCancel);
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_search_input_backspace() {
        let mut pending = PendingPrefix::SearchInput;
        let action = resolve(Mode::Normal, &key(KeyCode::Backspace), &mut pending, 0);
        assert_eq!(action, Action::SearchBackspace);
        assert_eq!(pending, PendingPrefix::SearchInput);
    }

    #[test]
    fn test_v_enters_charwise_visual() {
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('v'))),
            Action::EnterVisual(VisualKind::Char)
        );
    }

    #[test]
    fn test_shift_v_enters_linewise_visual() {
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('V'))),
            Action::EnterVisual(VisualKind::Line)
        );
    }

    #[test]
    fn test_p_pastes_in_normal() {
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('p'))),
            Action::Paste
        );
    }

    #[test]
    fn test_ctrl_w_prefix_splits_panes() {
        let mut pending = PendingPrefix::None;
        let ctrl_w = Key {
            ctrl: true,
            ..key(KeyCode::Char('w'))
        };
        assert_eq!(
            resolve(Mode::Normal, &ctrl_w, &mut pending, 0),
            Action::Ignore
        );
        assert_eq!(pending, PendingPrefix::CtrlW);
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char('v')), &mut pending, 0),
            Action::SplitPane(Direction::Vertical)
        );
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_visual_motion_extends_and_y_yanks() {
        assert_eq!(
            resolve_simple(Mode::Visual, &key(KeyCode::Char('j'))),
            Action::MoveCursor(CursorMove::Down)
        );
        assert_eq!(
            resolve_simple(Mode::Visual, &key(KeyCode::Char('y'))),
            Action::YankSelection
        );
    }

    #[test]
    fn test_visual_escape_returns_to_normal() {
        assert_eq!(
            resolve_simple(Mode::Visual, &key(KeyCode::Escape)),
            Action::SwitchMode(Mode::Normal)
        );
    }

    #[test]
    fn test_visual_v_toggles_back_to_normal() {
        // `v` in Visual resolves to EnterVisual; the handler toggles it off.
        assert_eq!(
            resolve_simple(Mode::Visual, &key(KeyCode::Char('v'))),
            Action::EnterVisual(VisualKind::Char)
        );
    }

    #[test]
    fn test_visual_gg_jumps_to_top() {
        let mut pending = PendingPrefix::None;
        let action = resolve(Mode::Visual, &key(KeyCode::Char('g')), &mut pending, 0);
        assert_eq!(action, Action::Ignore);
        assert_eq!(pending, PendingPrefix::G);
        let action = resolve(Mode::Visual, &key(KeyCode::Char('g')), &mut pending, 0);
        assert_eq!(action, Action::MoveCursor(CursorMove::Top));
    }

    #[test]
    fn test_x_deletes_char_on_prompt() {
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('x'))),
            Action::DeleteCharForward
        );
    }

    #[test]
    fn test_shift_d_deletes_to_line_end() {
        assert_eq!(
            resolve_simple(Mode::Normal, &key(KeyCode::Char('D'))),
            Action::DeleteToLineEnd
        );
    }

    #[test]
    fn test_dd_deletes_line() {
        let mut pending = PendingPrefix::None;
        let action = resolve(Mode::Normal, &key(KeyCode::Char('d')), &mut pending, 0);
        assert_eq!(action, Action::Ignore);
        assert_eq!(pending, PendingPrefix::Delete);
        let action = resolve(Mode::Normal, &key(KeyCode::Char('d')), &mut pending, 0);
        assert_eq!(action, Action::DeleteLine);
        assert_eq!(pending, PendingPrefix::None);
    }

    #[test]
    fn test_dw_deletes_word() {
        let mut pending = PendingPrefix::None;
        resolve(Mode::Normal, &key(KeyCode::Char('d')), &mut pending, 0);
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char('w')), &mut pending, 0),
            Action::DeleteWordForward
        );
    }

    #[test]
    fn test_ctrl_w_c_closes_pane() {
        let mut pending = PendingPrefix::CtrlW;
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char('c')), &mut pending, 0),
            Action::ClosePane
        );
    }

    #[test]
    fn test_ctrl_w_split_aliases() {
        // `s` and `S` both split horizontally; `v` splits vertically.
        for code in [KeyCode::Char('s'), KeyCode::Char('S')] {
            let mut pending = PendingPrefix::CtrlW;
            assert_eq!(
                resolve(Mode::Normal, &key(code), &mut pending, 0),
                Action::SplitPane(Direction::Horizontal)
            );
            assert_eq!(pending, PendingPrefix::None);
        }
        let mut pending = PendingPrefix::CtrlW;
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char('v')), &mut pending, 0),
            Action::SplitPane(Direction::Vertical)
        );
    }

    #[test]
    fn test_parse_chord_modifiers_and_named_keys() {
        assert_eq!(
            parse_chord("Ctrl-w"),
            Some(Key {
                alt: false,
                code: KeyCode::Char('w'),
                ctrl: true,
                shift: false,
            })
        );
        assert_eq!(
            parse_chord("Ctrl-Shift-Space"),
            Some(Key {
                alt: false,
                code: KeyCode::Space,
                ctrl: true,
                shift: true,
            })
        );
        assert_eq!(parse_chord("F5").map(|k| k.code), Some(KeyCode::F(5)));
        assert_eq!(parse_chord("v").map(|k| k.code), Some(KeyCode::Char('v')));
        assert_eq!(parse_chord("Hyper-x"), None);
    }

    #[test]
    fn test_parse_chord_sequence_length_bounds() {
        assert_eq!(parse_chord_sequence("Ctrl-w v").map(|k| k.len()), Some(2));
        assert_eq!(parse_chord_sequence("Ctrl-h").map(|k| k.len()), Some(1));
        assert_eq!(parse_chord_sequence(""), None);
        assert_eq!(parse_chord_sequence("a b c"), None);
    }

    #[test]
    fn test_config_rebinds_window_action_and_drops_default() {
        let mut bindings = HashMap::new();
        bindings.insert("Ctrl-w b".to_string(), "split_vertical".to_string());
        let keymap = WindowKeymap::from_config(Some(&bindings));

        // The rebound follow key now splits vertically.
        let mut pending = PendingPrefix::CtrlW;
        assert_eq!(
            resolve_with(Mode::Normal, &key(KeyCode::Char('b')), &mut pending, &keymap, 0),
            Action::SplitPane(Direction::Vertical)
        );
        // The default `Ctrl-w v` no longer splits vertically.
        let mut pending = PendingPrefix::CtrlW;
        assert_eq!(
            resolve_with(Mode::Normal, &key(KeyCode::Char('v')), &mut pending, &keymap, 0),
            Action::Ignore
        );
        // An unmentioned action keeps its default (`Ctrl-w c` still closes).
        let mut pending = PendingPrefix::CtrlW;
        assert_eq!(
            resolve_with(Mode::Normal, &key(KeyCode::Char('c')), &mut pending, &keymap, 0),
            Action::ClosePane
        );
    }

    #[test]
    fn test_config_custom_leader_and_direct_focus() {
        let mut bindings = HashMap::new();
        bindings.insert("Alt-x".to_string(), "split_horizontal".to_string());
        bindings.insert("Ctrl-b o".to_string(), "close_other_panes".to_string());
        let keymap = WindowKeymap::from_config(Some(&bindings));

        // A direct, non-default chord splits horizontally.
        let mut pending = PendingPrefix::None;
        let alt_x = Key {
            alt: true,
            ..key(KeyCode::Char('x'))
        };
        assert_eq!(
            resolve_with(Mode::Normal, &alt_x, &mut pending, &keymap, 0),
            Action::SplitPane(Direction::Horizontal)
        );

        // The leader is now Ctrl-b; Ctrl-b then o closes other panes.
        let mut pending = PendingPrefix::None;
        let ctrl_b = Key {
            ctrl: true,
            ..key(KeyCode::Char('b'))
        };
        assert_eq!(
            resolve_with(Mode::Normal, &ctrl_b, &mut pending, &keymap, 0),
            Action::Ignore
        );
        assert_eq!(pending, PendingPrefix::CtrlW);
        assert_eq!(
            resolve_with(Mode::Normal, &key(KeyCode::Char('o')), &mut pending, &keymap, 0),
            Action::CloseOtherPanes
        );
    }

    #[test]
    fn test_ctrl_w_close_aliases() {
        // `c` and `q` close the focused pane; `o` closes every other pane.
        for code in [KeyCode::Char('c'), KeyCode::Char('q')] {
            let mut pending = PendingPrefix::CtrlW;
            assert_eq!(
                resolve(Mode::Normal, &key(code), &mut pending, 0),
                Action::ClosePane
            );
        }
        let mut pending = PendingPrefix::CtrlW;
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char('o')), &mut pending, 0),
            Action::CloseOtherPanes
        );
        assert_eq!(pending, PendingPrefix::None);
    }

    // ---- Kitty keyboard protocol encoding -----------------------------------

    fn kitty(code: KeyCode) -> Action {
        let mut pending = PendingPrefix::None;
        resolve(Mode::Insert, &key(code), &mut pending, 1)
    }

    fn kitty_key(k: Key) -> Action {
        let mut pending = PendingPrefix::None;
        resolve(Mode::Insert, &k, &mut pending, 1)
    }

    #[test]
    fn test_xterm_ctrl_i_equals_tab_ambiguity() {
        // In legacy xterm mode, Ctrl+I and Tab produce the same bytes.
        let tab = resolve_simple(Mode::Insert, &key(KeyCode::Tab));
        let ctrl_i = resolve_simple(
            Mode::Insert,
            &Key { ctrl: true, ..key(KeyCode::Char('i')) },
        );
        assert_eq!(tab, ctrl_i);
    }

    #[test]
    fn test_kitty_ctrl_i_differs_from_tab() {
        // In Kitty mode, Tab still produces \t but Ctrl+I is disambiguated.
        let tab = kitty(KeyCode::Tab);
        let ctrl_i = kitty_key(Key { ctrl: true, ..key(KeyCode::Char('i')) });
        assert_ne!(tab, ctrl_i);
        assert_eq!(tab, Action::SendBytes(vec![b'\t']));
        // Ctrl+I: codepoint 105 ('i'), modifier 5 (ctrl=4, +1 base).
        assert_eq!(ctrl_i, Action::SendBytes(b"\x1b[105;5u".to_vec()));
    }

    #[test]
    fn test_kitty_shift_enter() {
        let shift_enter = kitty_key(Key { shift: true, ..key(KeyCode::Enter) });
        // Codepoint 13 (CR), modifier 2 (shift).
        assert_eq!(shift_enter, Action::SendBytes(b"\x1b[13;2u".to_vec()));
    }

    #[test]
    fn test_kitty_bare_enter_unchanged() {
        assert_eq!(kitty(KeyCode::Enter), Action::SendBytes(vec![b'\r']));
    }

    #[test]
    fn test_kitty_ctrl_escape() {
        let ctrl_esc = kitty_key(Key { ctrl: true, ..key(KeyCode::Escape) });
        // Codepoint 27 (ESC), modifier 5 (ctrl).
        assert_eq!(ctrl_esc, Action::SendBytes(b"\x1b[27;5u".to_vec()));
    }

    #[test]
    fn test_kitty_bare_escape_unchanged() {
        assert_eq!(kitty(KeyCode::Escape), Action::SendBytes(vec![ESCAPE]));
    }

    #[test]
    fn test_kitty_ctrl_left_arrow() {
        let ctrl_left = kitty_key(Key { ctrl: true, ..key(KeyCode::Left) });
        // KP_LEFT = 57350, modifier 5 (ctrl).
        assert_eq!(ctrl_left, Action::SendBytes(b"\x1b[57350;5u".to_vec()));
    }

    #[test]
    fn test_kitty_printable_no_modifier_passthrough() {
        assert_eq!(kitty(KeyCode::Char('a')), Action::SendBytes(vec![b'a']));
        assert_eq!(kitty(KeyCode::Char('Z')), Action::SendBytes(vec![b'Z']));
    }

    #[test]
    fn test_kitty_ctrl_printable() {
        // Ctrl+A: codepoint 97 ('a'), modifier 5 (ctrl).
        let ctrl_a = kitty_key(Key { ctrl: true, ..key(KeyCode::Char('a')) });
        assert_eq!(ctrl_a, Action::SendBytes(b"\x1b[97;5u".to_vec()));
    }

    #[test]
    fn test_kitty_f1_through_f4() {
        assert_eq!(kitty(KeyCode::F(1)), Action::SendBytes(b"\x1b[57364u".to_vec()));
        assert_eq!(kitty(KeyCode::F(4)), Action::SendBytes(b"\x1b[57367u".to_vec()));
    }

    #[test]
    fn test_kitty_shift_tab_backtab() {
        let shift_tab = kitty_key(Key { shift: true, ..key(KeyCode::Tab) });
        assert_eq!(shift_tab, Action::SendBytes(vec![ESCAPE, b'[', b'Z']));
    }
}
