//! The per-pane interaction-mode state machine (§2.4).
//!
//! A strict outer layer above the shell's own modality: keystrokes route to the
//! PTY (Insert), to SpaceTerm's block navigation (Normal), or to a focused
//! interactive block's WebView (Block-focus). The machine is per pane.

// ========================================================================
// Data Structures
// ========================================================================

/// Where keystrokes in a pane currently go.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Mode {
    /// Keys route to a focused interactive block's WebView.
    BlockFocus,
    /// Keys route to the PTY (a normal terminal). The default.
    #[default]
    Insert,
    /// SpaceTerm intercepts keys to traverse the block list.
    Normal,
}

/// A mode-changing input, already resolved from a keybinding (the entry chord is
/// configurable, §5.7; resolving keys to events is the keymap's job, not this
/// machine's).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModeEvent {
    /// The configurable entry chord (default `Ctrl-Shift-Space`).
    EnterNormal,
    /// `Enter` on an interactive block.
    FocusBlock,
    /// `Esc` (state-dependent: leaves Block-focus, or exits Normal to Insert).
    Escape,
    /// `i` in Normal mode.
    ToInsert,
}

// ========================================================================
// Mode
// ========================================================================

impl Mode {
    /// The mode reached by applying `event`. Entering Normal needs the
    /// non-colliding chord; exits are safe because SpaceTerm owns the keymap while in
    /// Normal or Block-focus.
    pub fn apply(self, event: ModeEvent) -> Mode {
        match (self, event) {
            (Mode::Insert, ModeEvent::EnterNormal) => Mode::Normal,
            (Mode::Insert, ModeEvent::FocusBlock | ModeEvent::Escape | ModeEvent::ToInsert) => {
                Mode::Insert
            }
            (Mode::Normal, ModeEvent::Escape | ModeEvent::ToInsert) => Mode::Insert,
            (Mode::Normal, ModeEvent::FocusBlock) => Mode::BlockFocus,
            (Mode::Normal, ModeEvent::EnterNormal) => Mode::Normal,
            (Mode::BlockFocus, ModeEvent::Escape) => Mode::Normal,
            (
                Mode::BlockFocus,
                ModeEvent::EnterNormal | ModeEvent::FocusBlock | ModeEvent::ToInsert,
            ) => Mode::BlockFocus,
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_mode_is_insert() {
        assert_eq!(Mode::default(), Mode::Insert);
    }

    #[test]
    fn test_chord_enters_normal_then_i_returns_to_insert() {
        let mode = Mode::Insert.apply(ModeEvent::EnterNormal);
        assert_eq!(mode, Mode::Normal);
        assert_eq!(mode.apply(ModeEvent::ToInsert), Mode::Insert);
    }

    #[test]
    fn test_escape_in_normal_returns_to_insert() {
        assert_eq!(Mode::Normal.apply(ModeEvent::Escape), Mode::Insert);
    }

    #[test]
    fn test_focus_block_and_escape_cycle_through_block_focus() {
        let mode = Mode::Normal.apply(ModeEvent::FocusBlock);
        assert_eq!(mode, Mode::BlockFocus);
        assert_eq!(mode.apply(ModeEvent::Escape), Mode::Normal);
    }

    #[test]
    fn test_escape_in_insert_is_a_noop() {
        // Esc in Insert belongs to the PTY; the mode does not change.
        assert_eq!(Mode::Insert.apply(ModeEvent::Escape), Mode::Insert);
    }
}
