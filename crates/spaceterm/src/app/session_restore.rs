//! Restore a saved session: rebuild the split layout and respawn PTYs.

use portable_pty::CommandBuilder;

use crate::model::layout::PaneId;
use crate::model::mode::Mode;
use crate::session::Session;
use crate::terminal::pane::Pane;


use super::{content_rows, App, APPROX_CELL_HEIGHT, APPROX_CELL_WIDTH, DEFAULT_COLS, DEFAULT_ROWS};

// ========================================================================
// App — session restore
// ========================================================================

impl App {
    /// If a saved session exists, replace the current single-pane layout with
    /// the persisted split tree and reopen each pane at its saved cwd.
    /// Returns `true` when a session was applied.
    pub(crate) fn restore_session_if_present(&mut self) -> bool {
        let Some(session) = Session::load() else { return false };
        let (tab, focused, pane_map) = session.into_tab();

        let (cols, rows) = if let Some(r) = &self.renderer {
            r.grid_size()
        } else {
            (DEFAULT_COLS as usize, DEFAULT_ROWS as usize)
        };
        let (cw, ch) = if let Some(r) = &self.renderer {
            r.cell_size()
        } else {
            (APPROX_CELL_WIDTH as f32, APPROX_CELL_HEIGHT as f32)
        };
        let want_rows = content_rows(rows);

        // Remove the bootstrap pane that init_window created.
        let bootstrap_id = self.tab().focused();
        self.panes.remove(&bootstrap_id);
        self.modes.remove(&bootstrap_id);

        // Spawn a PTY for every pane in the session.
        let max_scrollback = self.config.scrollback_lines.unwrap_or(spaceterm_render::MAX_SCROLLBACK);
        let shell = self.config.shell.clone();
        for (id, (cmd, cwd)) in &pane_map {
            let executable = cmd
                .as_deref()
                .or(shell.as_deref())
                .unwrap_or("/bin/sh");
            let mut builder = CommandBuilder::new(executable);
            if let Some(dir) = cwd {
                builder.cwd(dir);
            }
            let mut pane = Pane::with_command(cols.max(1), want_rows.max(1), builder, max_scrollback);
            pane.set_cell_size(cw, ch);
            self.panes.insert(*id, pane);
            self.modes.insert(*id, Mode::default());
        }

        // Replace the tab layout and update the focused pane counter so future
        // alloc_pane_id() calls yield IDs that don't collide with restored ones.
        self.tabs[self.active_tab] = tab;
        let max_id = pane_map.keys().map(|id| id.0).max().unwrap_or(0);
        if max_id >= self.next_pane_id {
            self.next_pane_id = max_id + 1;
        }

        // Focus the restored pane (if it exists in the layout, else any pane).
        let all_panes = self.tabs[self.active_tab].panes();
        let target = if all_panes.contains(&focused) {
            focused
        } else {
            all_panes.into_iter().next().unwrap_or(PaneId(0))
        };
        self.tabs[self.active_tab].focus(target);

        self.resize_all_panes();
        self.dirty = true;
        true
    }
}
