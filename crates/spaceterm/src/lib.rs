//! SpaceTerm native app: winit window, GPU text renderer, interactive PTY panes,
//! split-tree layout, and interaction modes. The `SpaceTerm` binary is a thin entry
//! point that creates an [`app::App`] and runs the winit event loop.

pub mod app;
mod block_queue;
pub mod config;
mod input;
mod layout;
mod mode;
pub mod palette;
pub mod pane;
pub mod session;
mod webview;

pub use input::{resolve, Action, BlockNav, Key, KeyCode};
pub use layout::{Direction, FocusDir, PaneId, Rect, Tab};
pub use mode::{Mode, ModeEvent};

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    #[test]
    fn test_lib_exports_mode() {
        let _ = super::Mode::default();
    }
}
