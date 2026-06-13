//! SGR / legacy mouse event encoding and forwarding to the PTY.

use winit::event::{ElementState, MouseButton};

use crate::model::layout::PaneId;

use super::App;

// ========================================================================
// App — PTY mouse forwarding
// ========================================================================

impl App {
    pub(crate) fn forward_mouse_event(
        &mut self,
        state: ElementState,
        button: MouseButton,
        focused: PaneId,
    ) {
        let (x, y) = self.cursor_pos;
        let Some((_, pane_rect)) = self.pane_at_pixel(x, y) else {
            return;
        };
        let (row, col) = self.pixel_to_cell(x, y, pane_rect);
        let sgr = self.panes.get(&focused).is_some_and(|p| p.mouse_sgr());

        let btn_code = match button {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            MouseButton::Forward => 4,
            MouseButton::Back => 5,
            _ => return,
        };

        let pressed = state == ElementState::Pressed;

        let bytes = if sgr {
            let cb = if pressed { btn_code } else { btn_code + 3 };
            let final_char = if pressed { 'M' } else { 'm' };
            format!("\x1b[<{};{};{}{}", cb, col + 1, row + 1, final_char).into_bytes()
        } else {
            let cb = (32 + if pressed { btn_code } else { btn_code + 3 }) as u8;
            let cv = 32u8.saturating_add((col.min(222) + 1) as u8);
            let ch = 32u8.saturating_add((row.min(222) + 1) as u8);
            format!("\x1b[M{}{}{}", cb as char, cv as char, ch as char).into_bytes()
        };

        if let Some(pane) = self.panes.get_mut(&focused) {
            pane.write(&bytes);
        }
    }

    pub(crate) fn forward_mouse_motion(&mut self, focused: PaneId) {
        let (x, y) = self.cursor_pos;
        let Some((_, pane_rect)) = self.pane_at_pixel(x, y) else {
            return;
        };
        let (row, col) = self.pixel_to_cell(x, y, pane_rect);
        let sgr = self.panes.get(&focused).is_some_and(|p| p.mouse_sgr());

        let btn_code = 0;
        let cb_code = 32 + btn_code;

        let bytes = if sgr {
            format!("\x1b[<{};{};{}M", cb_code, col + 1, row + 1).into_bytes()
        } else {
            let cb = (32 + cb_code) as u8;
            let cv = 32u8.saturating_add((col.min(222) + 1) as u8);
            let ch = 32u8.saturating_add((row.min(222) + 1) as u8);
            format!("\x1b[M{}{}{}", cb as char, cv as char, ch as char).into_bytes()
        };

        if let Some(pane) = self.panes.get_mut(&focused) {
            pane.write(&bytes);
        }
    }

    pub(crate) fn forward_mouse_scroll(&mut self, scroll_lines: isize, focused: PaneId) {
        let (x, y) = self.cursor_pos;
        let Some((_, pane_rect)) = self.pane_at_pixel(x, y) else {
            return;
        };
        let (row, col) = self.pixel_to_cell(x, y, pane_rect);
        let sgr = self.panes.get(&focused).is_some_and(|p| p.mouse_sgr());

        let count = scroll_lines.abs().min(10) as u8;
        let sign: u8 = if scroll_lines > 0 { 0 } else { 1 };

        for _ in 0..count {
            let cb = 64 + sign;
            let bytes = if sgr {
                format!("\x1b[<{};{};{}M", cb, col + 1, row + 1).into_bytes()
            } else {
                let b = 32 + cb;
                let cv = 32u8.saturating_add((col.min(222) + 1) as u8);
                let ch = 32u8.saturating_add((row.min(222) + 1) as u8);
                format!("\x1b[M{}{}{}", b as char, cv as char, ch as char).into_bytes()
            };
            if let Some(pane) = self.panes.get_mut(&focused) {
                pane.write(&bytes);
            }
        }
    }
}
