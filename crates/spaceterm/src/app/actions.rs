//! Keyboard action dispatch — translates resolved [`Action`]s into state changes.

use std::time::Instant;

use crate::model::input::{self, Action, VisualKind};
use crate::model::layout::{PaneId, Rect};
use crate::model::mode::{Mode, ModeEvent};

use super::prompt_edit::{prompt_delete_bytes, PromptDelete};
use super::{is_boundary_ding_key, App};

// ========================================================================
// App — action handling
// ========================================================================

impl App {
    pub(crate) fn handle_action(&mut self, action: Action, focused: PaneId) {
        match action {
            Action::SendBytes(bytes) => {
                if is_boundary_ding_key(&bytes) {
                    self.last_edit_key = Some(Instant::now());
                }
                if let Some(pane) = self.panes.get_mut(&focused) {
                    pane.write(&bytes);
                }
            }
            Action::SwitchMode(new_mode) => {
                let old_mode = self.modes.get(&focused).copied().unwrap_or_default();
                self.modes.insert(focused, new_mode);
                // Leaving Visual: the selection and anchor were ours to clear.
                if old_mode == Mode::Visual {
                    self.visual_anchor = None;
                    self.selection = None;
                }
                match new_mode {
                    // Entering Normal afresh (from Insert/Block) seeds the nav
                    // cursor at the prompt; returning from Visual keeps it put.
                    Mode::Normal => {
                        if old_mode != Mode::Normal && old_mode != Mode::Visual {
                            self.init_nav_cursor(focused);
                        }
                    }
                    Mode::Insert | Mode::BlockFocus => self.nav_cursor = None,
                    Mode::Visual => {}
                }
                self.dirty = true;
            }
            Action::MoveCursor(mv) => {
                self.move_nav_cursor(mv, focused);
                if self.modes.get(&focused) == Some(&Mode::Visual) {
                    self.update_visual_selection(focused);
                }
            }
            Action::EnterVisual(kind) => {
                self.toggle_visual(kind, focused);
            }
            Action::FindChar(find) => {
                self.last_find = Some(find);
                self.find_char_move(find, focused);
            }
            Action::FindRepeat { reverse } => {
                if let Some(find) = self.last_find {
                    let target = if reverse { find.reversed() } else { find };
                    self.find_char_move(target, focused);
                }
            }
            Action::YankSelection => {
                self.copy_selection();
                self.modes
                    .insert(focused, Mode::Visual.apply(ModeEvent::Escape));
                self.visual_anchor = None;
                self.selection = None;
                self.dirty = true;
            }
            Action::Paste => {
                self.paste_from_clipboard();
            }
            Action::DeleteCharForward => self.delete_on_prompt(PromptDelete::CharForward, focused),
            Action::DeleteLine => self.delete_on_prompt(PromptDelete::Line, focused),
            Action::DeleteToLineEnd => self.delete_on_prompt(PromptDelete::ToLineEnd, focused),
            Action::DeleteToLineStart => self.delete_on_prompt(PromptDelete::ToLineStart, focused),
            Action::DeleteWordBack => self.delete_on_prompt(PromptDelete::WordBack, focused),
            Action::DeleteWordForward => self.delete_on_prompt(PromptDelete::WordForward, focused),
            Action::SplitPane(direction) => {
                self.split_pane(direction);
            }
            Action::ClosePane => {
                self.close_pane(focused);
            }
            Action::CloseOtherPanes => {
                self.close_other_panes(focused);
            }
            Action::NewTab => {
                self.new_tab();
            }
            Action::NextTab => {
                self.cycle_tab(true);
            }
            Action::PrevTab => {
                self.cycle_tab(false);
            }
            Action::GotoTab(n) => {
                self.switch_tab(n.saturating_sub(1));
            }
            Action::CloseTab(which) => {
                let index = which
                    .map(|n| n.saturating_sub(1))
                    .unwrap_or(self.active_tab);
                self.close_tab(index);
            }
            Action::FocusPane(dir) => {
                let viewport = self.viewport_rect();
                let layout_vp = Rect::new(viewport.x, viewport.y, viewport.width, viewport.height);
                self.tab_mut().focus_in_direction(dir, layout_vp);
            }
            Action::FocusBlock(nav) => {
                self.focus_block(nav, focused);
            }
            Action::ForwardToBlock(bytes) => {
                self.webview_mgr.forward_key_event(focused, &bytes);
            }
            Action::SearchStart => {
                self.search_query = Some(String::new());
                self.dirty = true;
            }
            Action::SearchChar(c) => {
                if let Some(q) = &mut self.search_query {
                    q.push(c);
                }
                self.dirty = true;
            }
            Action::SearchBackspace => {
                if let Some(q) = &mut self.search_query {
                    q.pop();
                }
                self.dirty = true;
            }
            Action::SearchExecute => {
                self.search_in_pane(focused, input::BlockNav::Next);
                self.dirty = true;
            }
            Action::SearchCancel => {
                self.search_query = None;
                self.dirty = true;
            }
            Action::SearchNext => {
                self.search_in_pane(focused, input::BlockNav::Next);
            }
            Action::SearchPrevious => {
                self.search_in_pane(focused, input::BlockNav::Previous);
            }
            Action::YankBlock => {
                self.yank_block_source(focused);
            }
            Action::ToggleFold => {
                self.toggle_fold(focused);
            }
            Action::QuickSelect => {
                self.enter_quick_select(focused);
            }
            Action::QuickJump(c) => {
                self.quick_jump(focused, c);
            }
            Action::QuickCancel => {
                self.quick_select = None;
            }
            Action::Ignore => {}
        }
    }

    /// Apply a Vim delete operator to the last prompt by sending the shell the
    /// equivalent readline edit. Only the live prompt line (the row holding the
    /// shell cursor) is editable; deletes aimed at scrollback history are ignored.
    fn delete_on_prompt(&mut self, op: PromptDelete, focused: PaneId) {
        let Some(pane) = self.panes.get(&focused) else {
            return;
        };
        let (prompt_row, pty_col) = pane.grid().cursor();
        let (nav_row, nav_col) = self.nav_cursor.unwrap_or((prompt_row, pty_col));
        if nav_row != prompt_row {
            // The cursor is on scrollback history, not the live prompt: only the
            // prompt line is editable, so report the attempt instead of silently
            // dropping it.
            self.set_error("Cannot delete: not on the editable prompt line");
            return;
        }
        let bytes = prompt_delete_bytes(op, pty_col, nav_col);
        if let Some(pane) = self.panes.get_mut(&focused) {
            pane.write(&bytes);
        }
        self.nav_resync_pending = true;
        self.dirty = true;
    }

    /// Enter Visual mode from Normal, toggle it off when the same kind is pressed
    /// again, or switch between charwise and linewise while staying in Visual.
    fn toggle_visual(&mut self, kind: VisualKind, focused: PaneId) {
        let mode = self.modes.get(&focused).copied().unwrap_or_default();
        let want_line = matches!(kind, VisualKind::Line);
        match mode {
            Mode::Normal => {
                self.modes
                    .insert(focused, Mode::Normal.apply(ModeEvent::EnterVisual));
                self.visual_anchor = Some(self.nav_cursor.unwrap_or((0, 0)));
                self.visual_line = want_line;
                self.update_visual_selection(focused);
            }
            Mode::Visual if self.visual_line == want_line => {
                // Same kind again leaves Visual, back to Normal.
                self.modes
                    .insert(focused, Mode::Visual.apply(ModeEvent::EnterVisual));
                self.visual_anchor = None;
                self.selection = None;
            }
            Mode::Visual => {
                // Switch charwise <-> linewise, keeping the anchor.
                self.visual_line = want_line;
                self.update_visual_selection(focused);
            }
            Mode::Insert | Mode::BlockFocus => {}
        }
        self.dirty = true;
    }
}
