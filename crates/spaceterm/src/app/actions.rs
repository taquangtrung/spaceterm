//! Keyboard action dispatch — translates resolved [`Action`]s into state changes.

use std::time::Instant;

use crate::model::input::{self, Action};
use crate::model::layout::{PaneId, Rect};
use crate::model::mode::Mode;

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
                if new_mode == Mode::Normal && old_mode != Mode::Normal {
                    self.init_nav_cursor(focused);
                } else if new_mode != Mode::Normal {
                    self.nav_cursor = None;
                }
            }
            Action::MoveCursor(mv) => {
                self.move_nav_cursor(mv, focused);
            }
            Action::SplitPane(direction) => {
                self.split_pane(direction);
            }
            Action::ClosePane => {
                self.close_pane(focused);
            }
            Action::FocusPane(dir) => {
                let viewport = self.viewport_rect();
                let layout_vp = Rect::new(viewport.x, viewport.y, viewport.width, viewport.height);
                self.tab.focus_in_direction(dir, layout_vp);
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
}
