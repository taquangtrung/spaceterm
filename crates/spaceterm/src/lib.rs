//! SpaceTerm native app: winit window, GPU text renderer, interactive PTY panes,
//! split-tree layout, and interaction modes. The `SpaceTerm` binary is a thin entry
//! point that creates an [`app::App`] and runs the winit event loop.

pub mod app;
pub mod config;
pub mod model;
pub mod session;
pub mod terminal;

pub use model::input::{resolve, Action, BlockNav, Key, KeyCode};
pub use model::layout::{Direction, FocusDir, PaneId, Rect, Tab};
pub use model::mode::{Mode, ModeEvent};

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
