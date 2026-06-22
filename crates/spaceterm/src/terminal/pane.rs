//! Interactive PTY pane: owns a PTY child, a [`CombinedPerformer`] (unified
//! `vte` performer that drives both the visual grid and the block parser), and
//! a background thread that reads PTY output.

use std::io::Write;
use std::sync::mpsc;
use std::thread;

use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use std::io::Cursor;

use base64::Engine;
use spaceterm_core::spaceterm_proto::EmitBlock;
use spaceterm_core::{Performer, Scrollback, Segment};
use spaceterm_render::Grid;
#[cfg(test)]
use spaceterm_render::MAX_SCROLLBACK;
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

/// Default grid rows reserved for a content block whose displayed height is not
/// known at emit time (markdown, SVG, HTML). Reserved in-sequence (at the
/// escape) so the shell's subsequent output flows below the block instead of
/// under it, without desyncing the shell's cursor.
pub(crate) const BLOCK_RESERVE_ROWS: usize = 12;

/// Maximum APC payload size to accumulate before aborting (guards against
/// malformed or unterminated sequences bloating memory).
const APC_MAX_PAYLOAD: usize = 4 * 1024 * 1024;

/// Upper bound on rows an image block may reserve, so a tall image cannot eat
/// the whole screen. Raster images reserve exactly the rows they occupy, capped
/// here; the app scales them to fit the same cap.
pub(crate) const MAX_IMAGE_ROWS: usize = 24;

/// Raster image MIME types whose displayed height can be computed from their
/// pixel dimensions at emit time (so they reserve an exact band).
const RASTER_MIMES: [&str; 4] = ["image/gif", "image/jpeg", "image/png", "image/webp"];

/// Whether `url`'s scheme is on the safe-open allowlist (`http`, `https`,
/// `mailto`). Rejects `file://`, `javascript:`, custom app schemes, and
/// anything else that could invoke an unexpected OS handler on Ctrl+click.
fn is_safe_url_scheme(url: &str) -> bool {
    let scheme = url.split(':').next().unwrap_or("").to_ascii_lowercase();
    matches!(scheme.as_str(), "http" | "https" | "mailto")
}

/// Number of renderable (`Content`/`Live`) segments across the scrollback, used
/// to detect how many blocks an escape just produced.
fn content_segment_count(scrollback: &Scrollback) -> usize {
    scrollback
        .blocks()
        .iter()
        .flat_map(|block| &block.output)
        .filter(|segment| matches!(segment, Segment::Content(_) | Segment::Live(_)))
        .count()
}

/// The most recently emitted content block, used to size its reserved band.
fn last_content_block(scrollback: &Scrollback) -> Option<&EmitBlock> {
    scrollback
        .blocks()
        .iter()
        .rev()
        .flat_map(|block| block.output.iter().rev())
        .find_map(|segment| match segment {
            Segment::Content(emit) => Some(emit),
            _ => None,
        })
}

/// Exact rows a raster image occupies fit to the pane width, capped at
/// [`MAX_IMAGE_ROWS`]. `None` when the block is not a raster image (the caller
/// then uses the default band).
fn image_reserve_rows(
    emit: &EmitBlock,
    cols: usize,
    cell_width: f32,
    cell_height: f32,
) -> Option<usize> {
    let value = RASTER_MIMES
        .iter()
        .find_map(|mime| emit.bundle.get(mime).and_then(|v| v.as_str()))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(value)
        .ok()?;
    let (nat_w, nat_h) = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()?;
    if nat_w == 0 || nat_h == 0 || cell_height <= 0.0 {
        return None;
    }
    let pane_w = cols as f32 * cell_width;
    let display_w = (nat_w as f32).min(pane_w);
    let display_h = display_w * nat_h as f32 / nat_w as f32;
    let rows = (display_h / cell_height).ceil() as usize;
    Some(rows.clamp(1, MAX_IMAGE_ROWS))
}

// ========================================================================
// Data Structures
// ========================================================================

/// What a call to [`CombinedPerformer::apc_filter`] wants the drain loop to do
/// with the current byte.
enum ApcDecision {
    /// Byte was consumed by the APC state machine; do not forward to vte.
    Drop,
    /// Byte was not APC-related; forward it to vte as-is.
    Pass,
    /// The APC filter had buffered an ESC that turned out not to start an APC
    /// sequence. Forward ESC followed by the current byte to vte.
    ReplayEscThenByte(u8),
}

/// Kitty keyboard protocol flag stack. Apps push a flags bitmask to opt in to
/// progressive keyboard enhancement, then pop it on exit. The current top of
/// the stack is the active mode; an empty stack means legacy xterm encoding.
struct KittyStack(Vec<u32>);

impl KittyStack {
    fn push(&mut self, flags: u32) {
        self.0.push(flags);
    }

    fn pop(&mut self, n: u32) {
        for _ in 0..n {
            if self.0.is_empty() {
                break;
            }
            self.0.pop();
        }
    }

    fn current(&self) -> u32 {
        self.0.last().copied().unwrap_or(0)
    }

    /// Mode-based modification (`CSI = flags ; mode u`):
    /// mode 1 = set (replace current), 2 = unset (AND NOT), 3 = OR.
    /// If the stack is empty, a new entry is pushed; otherwise the top is updated.
    fn modify(&mut self, flags: u32, mode: u32) {
        let current = self.0.last().copied().unwrap_or(0);
        let new = match mode {
            1 => flags,
            2 => current & !flags,
            3 => current | flags,
            _ => return,
        };
        match self.0.last_mut() {
            Some(top) => *top = new,
            None => self.0.push(new),
        }
    }
}

impl Default for KittyStack {
    fn default() -> Self {
        Self(Vec::new())
    }
}

/// A single `vte::Perform` that fans out every callback to both a [`Grid`]
/// (visual cell grid) and a core [`Performer`] (block parser). This replaces
/// the previous dual-parser setup where every PTY byte was parsed twice.
struct CombinedPerformer {
    /// APC (Application Program Command) payload bytes accumulated between
    /// `ESC _` and the String Terminator `ESC \\` / `\x9c`. Used to parse the
    /// Kitty graphics protocol (`APC G ... ST`) which vte 0.13 silently drops.
    apc_buf: Vec<u8>,
    /// True while we are inside an `ESC _ ... ST` APC string.
    apc_in: bool,
    /// True when the last byte was `ESC` (0x1b) — used for lookahead so we can
    /// intercept `ESC _` without consuming unrelated escape sequences.
    apc_pending_esc: bool,
    bell: bool,
    /// Grid rows (one per emitted block, in emission order) where the block was
    /// anchored, drained by [`Pane::drain_output`] into the block queue.
    block_anchors: Vec<usize>,
    /// Pixel cell size, used to convert an image's pixel height into reserved
    /// rows. Set by the app once the renderer is up; defaults are close enough
    /// until then.
    cell_height: f32,
    cell_width: f32,
    grid: Grid,
    /// Accumulated base64 payload across Kitty graphics chunks (`m=1` packets).
    kitty_b64: Vec<u8>,
    /// Pixel dimensions from the first Kitty chunk header (`s=`, `v=`).
    kitty_px_h: u32,
    kitty_px_w: u32,
    /// Format code from the first Kitty chunk header (`f=`).
    kitty_format: u32,
    /// Active Kitty keyboard protocol flags (stack top), updated via
    /// `CSI > flags u` (push) and `CSI < n u` (pop).
    kitty_stack: KittyStack,
    performer: Performer,
    /// Response bytes queued by `CSI ? u` queries, drained into the PTY
    /// writer by [`Pane::drain_output`] after each parse batch.
    pending_responses: Vec<u8>,
    /// Text written via `OSC 52 ; c ; <base64>`, drained into the host
    /// clipboard by [`Pane::take_clipboard_write`] after each parse batch.
    pending_clipboard_write: Option<String>,
}

// ========================================================================
// CombinedPerformer
// ========================================================================

impl CombinedPerformer {
    fn new(cols: usize, rows: usize, max_scrollback: usize) -> Self {
        Self {
            apc_buf: Vec::new(),
            apc_in: false,
            apc_pending_esc: false,
            bell: false,
            block_anchors: Vec::new(),
            // Approximate defaults until the app sets the real cell size.
            cell_height: 20.0,
            cell_width: 9.0,
            grid: Grid::new(cols, rows).with_max_scrollback(max_scrollback),
            kitty_b64: Vec::new(),
            kitty_format: 100,
            kitty_px_h: 0,
            kitty_px_w: 0,
            kitty_stack: KittyStack::default(),
            performer: Performer::new(),
            pending_clipboard_write: None,
            pending_responses: Vec::new(),
        }
    }

    fn kitty_flags(&self) -> u32 {
        self.kitty_stack.current()
    }

    fn take_pending_responses(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pending_responses)
    }

    /// Take the decoded clipboard text from a pending `OSC 52 ; c ; <base64>`
    /// write, if any. Called by [`Pane::take_clipboard_write`] after each
    /// parse batch so the app layer can write it to the OS clipboard.
    fn take_clipboard_write(&mut self) -> Option<String> {
        self.pending_clipboard_write.take()
    }

    /// Handle `OSC 52 ; <selection> ; <data>` clipboard write.
    /// Only the write direction (base64-encoded text) is supported. Read
    /// queries (`<data>` = `?`) are intentionally ignored: responding would
    /// let any process running in the terminal silently exfiltrate the host
    /// clipboard without user confirmation.
    fn handle_osc52(&mut self, params: &[&[u8]]) {
        let data = match params.get(2) {
            Some(d) => *d,
            None => return,
        };
        if data == b"?" {
            // Read queries are dropped for security: clipboard exfiltration
            // would be silent and unconditional.
            return;
        }
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) {
            if let Ok(text) = String::from_utf8(bytes) {
                self.pending_clipboard_write = Some(text);
            }
        }
    }

    /// Anchor rows of blocks emitted since the last call, in emission order.
    fn take_block_anchors(&mut self) -> Vec<usize> {
        std::mem::take(&mut self.block_anchors)
    }

    fn set_cell_size(&mut self, width: f32, height: f32) {
        self.cell_width = width;
        self.cell_height = height;
    }

    /// Rows to reserve for the most recently emitted block: the exact rows a
    /// raster image will occupy (capped), else a default band.
    fn reserve_rows_for_last_block(&self) -> usize {
        match last_content_block(self.performer.scrollback()) {
            Some(emit) => {
                image_reserve_rows(emit, self.grid.cols(), self.cell_width, self.cell_height)
                    .unwrap_or(BLOCK_RESERVE_ROWS)
            }
            None => BLOCK_RESERVE_ROWS,
        }
    }

    /// Pre-filter one PTY byte for the Kitty graphics protocol. vte 0.13
    /// silently discards APC sequences (`ESC _ ... ST`), so we intercept them
    /// before the vte parser sees them.
    ///
    /// The caller must act on the returned [`ApcDecision`] to know what (if
    /// anything) to forward to the vte parser.
    fn apc_filter(&mut self, byte: u8) -> ApcDecision {
        if self.apc_in {
            if self.apc_pending_esc {
                self.apc_pending_esc = false;
                if byte == b'\\' {
                    self.finalize_apc();
                } else {
                    // ESC inside APC not followed by '\\': keep both bytes.
                    self.apc_buf.push(b'\x1b');
                    self.apc_buf.push(byte);
                }
                return ApcDecision::Drop;
            }
            match byte {
                0x9c | 0x07 => self.finalize_apc(),
                b'\x1b' => self.apc_pending_esc = true,
                _ => {
                    if self.apc_buf.len() < APC_MAX_PAYLOAD {
                        self.apc_buf.push(byte);
                    } else {
                        self.apc_buf.clear();
                        self.apc_in = false;
                    }
                }
            }
            return ApcDecision::Drop;
        }
        if self.apc_pending_esc {
            self.apc_pending_esc = false;
            if byte == b'_' {
                self.apc_in = true;
                self.apc_buf.clear();
                return ApcDecision::Drop;
            }
            // Not APC: replay the buffered ESC then the current byte.
            return ApcDecision::ReplayEscThenByte(byte);
        }
        if byte == b'\x1b' {
            self.apc_pending_esc = true;
            return ApcDecision::Drop; // buffer ESC until we see the next byte
        }
        ApcDecision::Pass
    }

    fn finalize_apc(&mut self) {
        self.apc_in = false;
        self.apc_pending_esc = false;
        if self.apc_buf.first() == Some(&b'G') {
            let payload = std::mem::take(&mut self.apc_buf);
            self.handle_kitty_apc(&payload[1..]);
        } else {
            self.apc_buf.clear();
        }
    }

    fn handle_kitty_apc(&mut self, payload: &[u8]) {
        let Ok(text) = std::str::from_utf8(payload) else {
            return;
        };
        let (ctrl_str, b64_data) = text.split_once(';').unwrap_or((text, ""));

        let mut format: u32 = 100;
        let mut more: u32 = 0;
        let mut px_w: u32 = 0;
        let mut px_h: u32 = 0;

        for kv in ctrl_str.split(',') {
            if let Some((k, v)) = kv.split_once('=') {
                match k.trim() {
                    "f" => format = v.trim().parse().unwrap_or(100),
                    "m" => more = v.trim().parse().unwrap_or(0),
                    "s" => px_w = v.trim().parse().unwrap_or(0),
                    "v" => px_h = v.trim().parse().unwrap_or(0),
                    _ => {}
                }
            }
        }

        if self.kitty_b64.is_empty() {
            self.kitty_format = format;
            self.kitty_px_w = px_w;
            self.kitty_px_h = px_h;
        }
        self.kitty_b64.extend_from_slice(b64_data.as_bytes());

        if more == 0 {
            let b64 = std::mem::take(&mut self.kitty_b64);
            self.finalize_kitty_image(&b64);
        }
    }

    fn finalize_kitty_image(&mut self, b64: &[u8]) {
        use base64::Engine;
        use spaceterm_core::spaceterm_proto::{EmitBlock, MimeBundle, TrustTier, TEXT_PLAIN};

        let decoded = match base64::engine::general_purpose::STANDARD.decode(b64) {
            Ok(d) => d,
            Err(_) => return,
        };

        let (mime, bytes): (&str, Vec<u8>) = match self.kitty_format {
            100 => ("image/png", decoded),
            1 => ("image/jpeg", decoded),
            32 => {
                let (w, h) = (self.kitty_px_w, self.kitty_px_h);
                if w == 0 || h == 0 {
                    return;
                }
                let Some(img) = image::RgbaImage::from_raw(w, h, decoded) else {
                    return;
                };
                let mut png: Vec<u8> = Vec::new();
                if image::DynamicImage::ImageRgba8(img)
                    .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
                    .is_err()
                {
                    return;
                }
                ("image/png", png)
            }
            24 => {
                let (w, h) = (self.kitty_px_w, self.kitty_px_h);
                if w == 0 || h == 0 {
                    return;
                }
                let Some(img) = image::RgbImage::from_raw(w, h, decoded) else {
                    return;
                };
                let mut png: Vec<u8> = Vec::new();
                if image::DynamicImage::ImageRgb8(img)
                    .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
                    .is_err()
                {
                    return;
                }
                ("image/png", png)
            }
            _ => return,
        };

        let data_b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let mut bundle = MimeBundle::new();
        bundle.insert(mime, serde_json::Value::from(data_b64.as_str()));
        bundle.insert(TEXT_PLAIN, serde_json::Value::from("[image]"));
        let block = EmitBlock {
            bundle,
            id: self.performer.alloc_block_id(),
            trust: TrustTier::default(),
        };
        let before = content_segment_count(self.performer.scrollback());
        self.performer.emit(block);
        let after = content_segment_count(self.performer.scrollback());
        let rows = self.reserve_rows_for_last_block();
        for _ in before..after {
            self.block_anchors.push(self.grid.cursor().0);
            for _ in 0..rows {
                self.grid.line_feed();
            }
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

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        // Kitty keyboard protocol negotiation (final byte 'u').
        if action == 'u' {
            match intermediates {
                // CSI > flags u — push flags onto the stack.
                [b'>'] => {
                    let flags = params.iter().next().and_then(|p| p.first()).map(|&v| v as u32).unwrap_or(0);
                    self.kitty_stack.push(flags);
                    return;
                }
                // CSI < n u — pop n entries (default 1).
                [b'<'] => {
                    let n = params.iter().next().and_then(|p| p.first()).map(|&v| v as u32).unwrap_or(1);
                    self.kitty_stack.pop(n);
                    return;
                }
                // CSI ? u — query: respond with current flags.
                [b'?'] => {
                    let flags = self.kitty_stack.current();
                    let response = format!("\x1b[?{flags}u");
                    self.pending_responses.extend_from_slice(response.as_bytes());
                    return;
                }
                // CSI = flags ; mode u — mode-based set/unset/or (no stack change).
                [b'='] => {
                    let mut iter = params.iter();
                    let flags = iter.next().and_then(|p| p.first()).map(|&v| v as u32).unwrap_or(0);
                    let mode = iter.next().and_then(|p| p.first()).map(|&v| v as u32).unwrap_or(1);
                    self.kitty_stack.modify(flags, mode);
                    return;
                }
                _ => {}
            }
        }
        Perform::csi_dispatch(&mut self.grid, params, intermediates, ignore, action);
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], bell_terminated: bool) {
        // OSC 52 — clipboard read/write. Handled here because it needs arboard
        // access (the core Performer crate does not depend on arboard).
        if params.first() == Some(&b"52".as_slice()) {
            self.handle_osc52(params);
            return;
        }

        // OSC 8 ; params ; URI — open/close a hyperlink on the visual grid.
        // Only store URLs whose scheme is on the allowlist so that rogue
        // sequences cannot cause Ctrl+click to invoke arbitrary OS handlers
        // (e.g. `file://`, `javascript:`, custom app schemes).
        if params.first() == Some(&b"8".as_slice()) {
            let uri: String = params
                .get(2..)
                .unwrap_or_default()
                .iter()
                .map(|b| String::from_utf8_lossy(b))
                .collect::<Vec<_>>()
                .join(";");
            let safe = !uri.is_empty() && is_safe_url_scheme(&uri);
            self.grid.set_active_link(safe.then_some(uri.as_str()));
        }

        let before = content_segment_count(self.performer.scrollback());
        Perform::osc_dispatch(&mut self.performer, params, bell_terminated);
        let after = content_segment_count(self.performer.scrollback());

        // For each block this escape produced, anchor it at the current row and
        // reserve rows so the shell's following output flows below it.
        let rows = self.reserve_rows_for_last_block();
        for _ in before..after {
            self.block_anchors.push(self.grid.cursor().0);
            for _ in 0..rows {
                self.grid.line_feed();
            }
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, byte: u8) {
        Perform::esc_dispatch(&mut self.grid, intermediates, ignore, byte);
    }

    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _action: char) {}

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
    /// The shell or command path used to spawn this pane.
    command: String,
    combined: CombinedPerformer,
    master: Box<dyn portable_pty::MasterPty + Send>,
    parser: vte::Parser,
    writer: Box<dyn Write + Send>,
    rx: mpsc::Receiver<Vec<u8>>,
    _read_thread: Option<thread::JoinHandle<()>>,
}

impl Pane {
    /// Spawn the default shell under a PTY with the given grid dimensions.
    pub fn new(cols: usize, rows: usize, configured_shell: Option<&str>, max_scrollback: usize) -> Self {
        let shell = configured_shell
            .map(|s| s.to_string())
            .or_else(|| std::env::var("SPACETERM_SHELL").ok())
            .or_else(|| std::env::var("SHELL").ok())
            .unwrap_or_else(|| "/bin/bash".to_string());

        let command = CommandBuilder::new(&shell);
        Self::with_command(cols, rows, command, max_scrollback)
    }

    /// Spawn `command` under a PTY with the given grid dimensions.
    pub fn with_command(cols: usize, rows: usize, command: CommandBuilder, max_scrollback: usize) -> Self {
        let command_str = command
            .get_argv()
            .first()
            .and_then(|a| a.to_str())
            .unwrap_or("sh")
            .to_string();
        Self::with_command_labeled(cols, rows, command, max_scrollback, command_str)
    }

    fn with_command_labeled(
        cols: usize,
        rows: usize,
        mut command: CommandBuilder,
        max_scrollback: usize,
        command_str: String,
    ) -> Self {
        // Advertise SpaceTerm to the child so capability-detecting tools (e.g.
        // `spacecat`, `clients/client.sh`) emit rich blocks instead of the
        // plain-text fallback.
        command.env("TERM_PROGRAM", "spaceterm");
        command.env("SPACETERM", "1");
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");

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
            command: command_str,
            combined: CombinedPerformer::new(cols, rows, max_scrollback),
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
                match self.combined.apc_filter(byte) {
                    ApcDecision::Drop => {}
                    ApcDecision::Pass => {
                        self.parser.advance(&mut self.combined, byte);
                    }
                    ApcDecision::ReplayEscThenByte(b) => {
                        self.parser.advance(&mut self.combined, b'\x1b');
                        self.parser.advance(&mut self.combined, b);
                    }
                }
            }
            got_any = true;
        }
        if got_any {
            let (row, _) = self.combined.grid().cursor();
            let anchors = self.combined.take_block_anchors();
            self.block_queue
                .update(self.combined.scrollback(), row, &anchors);
        }
        let responses = self.combined.take_pending_responses();
        if !responses.is_empty() {
            let _ = self.writer.write_all(&responses);
            let _ = self.writer.flush();
        }
        got_any
    }

    /// Current Kitty keyboard protocol flags active in this pane (0 = legacy).
    pub fn kitty_flags(&self) -> u32 {
        self.combined.kitty_flags()
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

    /// Set the pixel cell size so image blocks reserve the exact rows they
    /// occupy. Called once the renderer's metrics are known.
    pub fn set_cell_size(&mut self, width: f32, height: f32) {
        self.combined.set_cell_size(width, height);
    }

    /// The terminal cell grid (mutable, for scrollback navigation).
    pub fn grid_mut(&mut self) -> &mut Grid {
        self.combined.grid_mut()
    }

    /// The scrollback parsed so far.
    pub fn scrollback(&self) -> &Scrollback {
        self.combined.scrollback()
    }

    /// True when no full-screen process is running. Uses the alternate screen as
    /// a proxy: full-screen apps (vim, fzf, less) enter it; the shell prompt does not.
    pub fn is_at_prompt(&self) -> bool {
        !self.combined.grid().is_alt_screen()
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

    /// Take the clipboard text from a pending `OSC 52` write, if any.
    pub fn take_clipboard_write(&mut self) -> Option<String> {
        self.combined.take_clipboard_write()
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

    /// The shell or command path used to spawn this pane.
    pub fn shell_command(&self) -> &str {
        &self.command
    }

    /// Working directory of the foreground process running in this pane.
    /// On Linux this reads `/proc/{pid}/cwd`; returns `None` on other
    /// platforms or when the PID is not available.
    pub fn cwd(&self) -> Option<String> {
        #[cfg(target_os = "linux")]
        {
            let pid = self.child.process_id()?;
            std::fs::read_link(format!("/proc/{pid}/cwd"))
                .ok()
                .and_then(|p| p.into_os_string().into_string().ok())
        }
        #[cfg(not(target_os = "linux"))]
        { None }
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
        let mut pane = Pane::with_command(40, 10, CommandBuilder::new("bash"), MAX_SCROLLBACK);
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
        let mut pane = Pane::with_command(20, 5, CommandBuilder::new("bash"), MAX_SCROLLBACK);
        pane.resize(40, 10);
        thread::sleep(std::time::Duration::from_millis(50));
        assert!(pane.is_alive());
    }

    #[test]
    fn test_combined_performer_print_feeds_both() {
        let mut cp = CombinedPerformer::new(10, 2, MAX_SCROLLBACK);
        cp.print('x');
        assert_eq!(cp.grid().cell(0, 0).map(|c| c.ch), Some('x'));
        assert!(cp.scrollback().plain_text().contains('x'));
    }

    #[test]
    fn test_combined_performer_csi_moves_cursor() {
        let mut cp = CombinedPerformer::new(5, 2, MAX_SCROLLBACK);
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
        let mut cp = CombinedPerformer::new(10, 1, MAX_SCROLLBACK);
        cp.execute(BELL);
        assert!(cp.take_bell());
        assert!(!cp.take_bell());
    }

    #[test]
    fn test_kitty_stack_empty_returns_zero() {
        let stack = KittyStack::default();
        assert_eq!(stack.current(), 0);
    }

    #[test]
    fn test_kitty_stack_push_pop() {
        let mut stack = KittyStack::default();
        stack.push(1);
        stack.push(3);
        assert_eq!(stack.current(), 3);
        stack.pop(1);
        assert_eq!(stack.current(), 1);
        stack.pop(1);
        assert_eq!(stack.current(), 0);
        // Pop on empty stack is a no-op.
        stack.pop(1);
        assert_eq!(stack.current(), 0);
    }

    #[test]
    fn test_kitty_stack_modify_set_replaces_top() {
        let mut stack = KittyStack::default();
        stack.push(3);
        stack.modify(5, 1); // mode 1 = set
        assert_eq!(stack.current(), 5);
    }

    #[test]
    fn test_kitty_stack_modify_unset_clears_bits() {
        let mut stack = KittyStack::default();
        stack.push(7); // 0b111
        stack.modify(2, 2); // mode 2 = AND NOT: 7 & !2 = 5
        assert_eq!(stack.current(), 5);
    }

    #[test]
    fn test_kitty_stack_modify_or_adds_bits() {
        let mut stack = KittyStack::default();
        stack.push(1);
        stack.modify(6, 3); // mode 3 = OR: 1 | 6 = 7
        assert_eq!(stack.current(), 7);
    }

    #[test]
    fn test_kitty_stack_modify_on_empty_stack_pushes_entry() {
        let mut stack = KittyStack::default();
        stack.modify(3, 1); // set on empty: pushes 3
        assert_eq!(stack.current(), 3);
    }

    #[test]
    fn test_kitty_stack_modify_unknown_mode_is_noop() {
        let mut stack = KittyStack::default();
        stack.push(1);
        stack.modify(99, 99); // unknown mode
        assert_eq!(stack.current(), 1); // unchanged
    }
}
