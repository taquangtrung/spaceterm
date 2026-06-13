//! Interactive PTY pane: owns a PTY child, a [`CombinedPerformer`] (unified
//! `vte` performer that drives both the visual grid and the block parser), and
//! a background thread that reads PTY output.

use std::io::Write;
use std::sync::mpsc;
use std::thread;

use spaceterm_core::{Performer, Scrollback};
use spaceterm_render::Grid;
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use vte::{Params, Perform};

use super::block_queue::BlockQueue;

// ========================================================================
// Constants
// ========================================================================

const READ_CHUNK: usize = 4096;
const BELL: u8 = 0x07;
const LINE_FEED: u8 = b'\n';
const CARRIAGE_RETURN: u8 = b'\r';
const BACKSPACE: u8 = 0x08;
const HORIZONTAL_TAB: u8 = b'\t';

// ========================================================================
// Data Structures
// ========================================================================

/// A single `vte::Perform` that fans out every callback to both a [`Grid`]
/// (visual cell grid) and a core [`Performer`] (block parser). This replaces
/// the previous dual-parser setup where every PTY byte was parsed twice.
struct CombinedPerformer {
    bell: bool,
    grid: Grid,
    performer: Performer,
}

// ========================================================================
// CombinedPerformer
// ========================================================================

impl CombinedPerformer {
    fn new(cols: usize, rows: usize) -> Self {
        Self {
            bell: false,
            grid: Grid::new(cols, rows),
            performer: Performer::new(),
        }
    }

    fn grid(&self) -> &Grid {
        &self.grid
    }

    fn grid_mut(&mut self) -> &mut Grid {
        &mut self.grid
    }

    fn scrollback(&self) -> &Scrollback {
        self.performer.scrollback()
    }

    fn take_title(&mut self) -> Option<String> {
        self.performer.take_title()
    }

    fn take_bell(&mut self) -> bool {
        std::mem::take(&mut self.bell)
    }

    fn resize(&mut self, cols: usize, rows: usize) {
        self.grid.resize(cols, rows);
    }
}

impl Perform for CombinedPerformer {
    fn print(&mut self, c: char) {
        self.grid.print(c);
        Perform::print(&mut self.performer, c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            LINE_FEED => self.grid.line_feed(),
            CARRIAGE_RETURN => self.grid.carriage_return(),
            BACKSPACE => self.grid.backspace(),
            HORIZONTAL_TAB => self.grid.tab(),
            _ => {}
        }
        Perform::execute(&mut self.performer, byte);
        if byte == BELL {
            self.bell = true;
        }
    }

    fn csi_dispatch(
        &mut self,
        params: &Params,
        intermediates: &[u8],
        ignore: bool,
        action: char,
    ) {
        Perform::csi_dispatch(&mut self.grid, params, intermediates, ignore, action);
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], bell_terminated: bool) {
        Perform::osc_dispatch(&mut self.performer, params, bell_terminated);
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, byte: u8) {
        Perform::esc_dispatch(&mut self.grid, intermediates, ignore, byte);
    }

    fn hook(
        &mut self,
        _params: &Params,
        _intermediates: &[u8],
        _ignore: bool,
        _action: char,
    ) {
    }

    fn put(&mut self, _byte: u8) {}

    fn unhook(&mut self) {}
}

// ========================================================================
// Pane
// ========================================================================

/// One interactive pane: a shell running under a PTY whose output feeds a
/// terminal cell grid. PTY reads happen on a background thread; the main
/// thread drains pending output via [`Pane::drain_output`].
pub struct Pane {
    block_queue: BlockQueue,
    child: Box<dyn portable_pty::Child + Send>,
    combined: CombinedPerformer,
    master: Box<dyn portable_pty::MasterPty + Send>,
    parser: vte::Parser,
    writer: Box<dyn Write + Send>,
    rx: mpsc::Receiver<Vec<u8>>,
    _read_thread: Option<thread::JoinHandle<()>>,
}

impl Pane {
    /// Spawn the default shell under a PTY with the given grid dimensions.
    pub fn new(cols: usize, rows: usize) -> Self {
        let shell = if let Ok(s) = std::env::var("SPACETERM_SHELL") {
            s
        } else if let Ok(s) = std::env::var("SHELL") {
            s
        } else {
            "/bin/bash".to_string()
        };

        let command = CommandBuilder::new(shell);
        Self::with_command(cols, rows, command)
    }

    /// Spawn `command` under a PTY with the given grid dimensions.
    pub fn with_command(cols: usize, rows: usize, command: CommandBuilder) -> Self {
        let pty_system = NativePtySystem::default();
        let pair = pty_system
            .openpty(PtySize {
                rows: rows as u16,
                cols: cols as u16,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open PTY");

        let child = pair.slave.spawn_command(command).expect("spawn command");
        drop(pair.slave);

        let reader = pair.master.try_clone_reader().expect("clone PTY reader");
        let writer = pair.master.take_writer().expect("take PTY writer");

        let (tx, rx) = mpsc::channel::<Vec<u8>>();

        let read_thread = thread::Builder::new()
            .name("spaceterm pty read".into())
            .spawn(move || {
                let mut buf = [0u8; READ_CHUNK];
                let mut reader = reader;
                loop {
                    match std::io::Read::read(&mut reader, &mut buf) {
                        Ok(0) => break,
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                        Ok(count) => {
                            if tx.send(buf[..count].to_vec()).is_err() {
                                break;
                            }
                        }
                    }
                }
            })
            .expect("spawn PTY read thread");

        Self {
            block_queue: BlockQueue::new(),
            child,
            combined: CombinedPerformer::new(cols, rows),
            master: pair.master,
            parser: vte::Parser::new(),
            writer,
            rx,
            _read_thread: Some(read_thread),
        }
    }

    /// Drain all pending PTY output into the cell grid and block parser.
    /// Returns `true` if any output was processed.
    pub fn drain_output(&mut self) -> bool {
        let mut got_any = false;
        while let Ok(chunk) = self.rx.try_recv() {
            for &byte in &chunk {
                self.parser.advance(&mut self.combined, byte);
            }
            got_any = true;
        }
        if got_any {
            let (row, _) = self.combined.grid().cursor();
            self.block_queue.update(self.combined.scrollback(), row);
        }
        got_any
    }

    /// Write bytes to the PTY (keyboard input).
    pub fn write(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    /// Resize the PTY and the cell grid. Signals the child process via
    /// `SIGWINCH` so the shell knows about the new dimensions.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let had_region = {
            let g = self.combined.grid();
            g.scroll_top() != 0 || g.scroll_bottom() != g.rows().saturating_sub(1)
        };
        let _ = self.master.resize(PtySize {
            rows: rows as u16,
            cols: cols as u16,
            pixel_width: 0,
            pixel_height: 0,
        });
        self.combined.resize(cols, rows);
        if had_region {
            self.write(b"\x1b[r");
        }
    }

    /// The terminal cell grid (read-only for rendering).
    pub fn grid(&self) -> &Grid {
        self.combined.grid()
    }

    /// The terminal cell grid (mutable, for scrollback navigation).
    pub fn grid_mut(&mut self) -> &mut Grid {
        self.combined.grid_mut()
    }

    /// The scrollback parsed so far.
    pub fn scrollback(&self) -> &Scrollback {
        self.combined.scrollback()
    }

    /// Whether bracketed paste mode (CSI ?2004h) is active.
    pub fn bracketed_paste(&self) -> bool {
        self.combined.grid().bracketed_paste()
    }

    /// Whether any mouse tracking mode is active.
    pub fn mouse_tracking(&self) -> bool {
        self.combined.grid().mouse_tracking()
    }

    /// Whether drag tracking specifically is active.
    pub fn mouse_drag_tracking(&self) -> bool {
        self.combined.grid().mouse_drag_tracking()
    }

    /// Whether focus event mode (CSI ?1004h) is active.
    pub fn focus_event(&self) -> bool {
        self.combined.grid().focus_event()
    }

    /// Whether SGR extended mouse mode is active.
    pub fn mouse_sgr(&self) -> bool {
        self.combined.grid().mouse_sgr()
    }

    /// Current cursor shape set by DECSCUSR.
    pub fn cursor_shape(&self) -> spaceterm_render::CursorShape {
        self.combined.grid().cursor_shape()
    }

    /// Take the pending window title set by OSC 0/2, if any.
    pub fn take_title(&mut self) -> Option<String> {
        self.combined.take_title()
    }

    /// Whether a bell character was received since the last check.
    pub fn take_bell(&mut self) -> bool {
        self.combined.take_bell()
    }

    pub fn block_queue(&self) -> &BlockQueue {
        &self.block_queue
    }

    pub fn block_queue_mut(&mut self) -> &mut BlockQueue {
        &mut self.block_queue
    }

    pub fn drain_live_patches(&mut self) -> Vec<usize> {
        let blocks = self.combined.scrollback().blocks().to_vec();
        self.block_queue.drain_patched_live(&blocks)
    }

    /// Whether the child process has exited.
    pub fn is_alive(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(Some(_)) => false,
            Ok(None) => true,
            Err(_) => false,
        }
    }
}

impl Drop for Pane {
    fn drop(&mut self) {
        self.writer.flush().ok();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pane_echo() {
        let mut pane = Pane::with_command(40, 10, CommandBuilder::new("bash"));
        pane.write(b"echo hello\n");
        thread::sleep(std::time::Duration::from_millis(100));
        pane.drain_output();
        let text = pane.grid().to_text();
        assert!(
            text.contains("hello"),
            "expected 'hello' in output, got: {text}"
        );
    }

    #[test]
    fn test_pane_resize_signals_pty() {
        let mut pane = Pane::with_command(20, 5, CommandBuilder::new("bash"));
        pane.resize(40, 10);
        thread::sleep(std::time::Duration::from_millis(50));
        assert!(pane.is_alive());
    }

    #[test]
    fn test_combined_performer_print_feeds_both() {
        let mut cp = CombinedPerformer::new(10, 2);
        cp.print('x');
        assert_eq!(cp.grid().cell(0, 0).map(|c| c.ch), Some('x'));
        assert!(cp.scrollback().plain_text().contains('x'));
    }

    #[test]
    fn test_combined_performer_csi_moves_cursor() {
        let mut cp = CombinedPerformer::new(5, 2);
        cp.print('a');
        cp.print('b');
        cp.print('c');
        let mut parser = vte::Parser::new();
        for &b in b"\x1b[1;1HX" {
            parser.advance(&mut cp, b);
        }
        assert_eq!(cp.grid().cell(0, 0).map(|c| c.ch), Some('X'));
        assert_eq!(cp.grid().cell(0, 1).map(|c| c.ch), Some('b'));
    }

    #[test]
    fn test_combined_performer_bell() {
        let mut cp = CombinedPerformer::new(10, 1);
        cp.execute(BELL);
        assert!(cp.take_bell());
        assert!(!cp.take_bell());
    }
}
