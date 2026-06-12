//! Stream parser: drives `vte` and routes escapes into a [`Scrollback`].
//!
//! Printable bytes and newlines build text; OSC 133 marks delimit command blocks;
//! OSC 7 reports the working directory; OSC 9001 carries TBP blocks, decoded via
//! [`spaceterm_proto::decode`]. Everything else is ignored, so unknown escapes degrade
//! gracefully.

use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use spaceterm_proto::{BlockId, EmitBlock, Message, MimeBundle, TrustTier, TEXT_PLAIN};
use serde_json::Value;
use vte::{Params, Parser, Perform};

use crate::scrollback::Scrollback;

// ============================================================================
// Constants
// ============================================================================

const NEWLINE: u8 = b'\n';

const OSC_CWD: &[u8] = b"7";
const OSC_HYPERLINK: &[u8] = b"8";
const OSC_ICON_TITLE: &[u8] = b"0";
const OSC_ITERM2: &[u8] = b"1337";
const OSC_SHELL_MARK: &[u8] = b"133";
const OSC_TITLE: &[u8] = b"2";
const OSC_TBP: &[u8] = b"9001";

const MARK_PROMPT_START: &[u8] = b"A";
const MARK_COMMAND_START: &[u8] = b"B";
const MARK_OUTPUT_START: &[u8] = b"C";
const MARK_COMMAND_END: &[u8] = b"D";

const FIELD_SEP: &str = ";";
const FILE_URI_SCHEME: &str = "file://";

const ITERM2_FILE_PREFIX: &str = "File=";
const ITERM2_DATA_SEP: char = ':';
const IMAGE_FALLBACK: &str = "[image]";

// Image magic numbers used to label a normalized legacy image block.
const PNG_MAGIC: &[u8] = &[0x89, b'P', b'N', b'G'];
const JPEG_MAGIC: &[u8] = &[0xFF, 0xD8];
const GIF_MAGIC: &[u8] = b"GIF8";
const DEFAULT_IMAGE_MIME: &str = "application/octet-stream";

// ============================================================================
// Data Structures
// ============================================================================

/// A terminal that consumes a PTY byte stream and exposes the parsed scrollback.
pub struct Terminal {
    parser: Parser,
    performer: Performer,
}

/// The core block-aware performer: receives `vte` callbacks and updates a
/// [`Scrollback`] with command blocks, CWD changes, hyperlinks, TBP messages,
/// and iTerm2 inline images.
pub struct Performer {
    pending_title: Option<String>,
    scrollback: Scrollback,
    side_channel_dir: Option<String>,
    synthetic_id: u64,
}

// ============================================================================
// Terminal
// ============================================================================

impl Terminal {
    /// A terminal with empty scrollback, ready to be fed bytes.
    pub fn new() -> Self {
        Self {
            parser: Parser::new(),
            performer: Performer::new(),
        }
    }

    /// Consume a chunk of PTY output, updating the scrollback.
    pub fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.parser.advance(&mut self.performer, byte);
        }
    }

    /// The scrollback parsed so far.
    pub fn scrollback(&self) -> &Scrollback {
        &self.performer.scrollback
    }

    /// Take the pending window title set by OSC 0/2.
    pub fn take_title(&mut self) -> Option<String> {
        self.performer.take_title()
    }
}

impl Default for Terminal {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Performer — vte callbacks
// ============================================================================

impl Default for Performer {
    fn default() -> Self {
        Self::new()
    }
}

impl Performer {
    /// A fresh performer with an empty scrollback, ready to be fed bytes.
    /// Reads `SPACETERM_SIDECHANNEL_DIR` from the environment to enable
    /// side-channel file transport for large payloads.
    pub fn new() -> Self {
        Self {
            pending_title: None,
            scrollback: Scrollback::new(),
            side_channel_dir: std::env::var("SPACETERM_SIDECHANNEL_DIR").ok(),
            synthetic_id: 0,
        }
    }

    /// The scrollback parsed so far.
    pub fn scrollback(&self) -> &Scrollback {
        &self.scrollback
    }

    /// Take the pending title set by OSC 0/2, if any.
    pub fn take_title(&mut self) -> Option<String> {
        self.pending_title.take()
    }

    fn route_osc(&mut self, params: &[&[u8]]) {
        let Some(&kind) = params.first() else {
            return;
        };
        if kind == OSC_SHELL_MARK {
            self.handle_shell_mark(params);
        } else if kind == OSC_CWD {
            self.handle_cwd(params);
        } else if kind == OSC_HYPERLINK {
            self.handle_hyperlink(params);
        } else if kind == OSC_TBP {
            self.handle_tbp(params);
        } else if kind == OSC_ITERM2 {
            self.handle_iterm2(params);
        } else if kind == OSC_TITLE || kind == OSC_ICON_TITLE {
            self.handle_title(params);
        }
    }

    fn handle_shell_mark(&mut self, params: &[&[u8]]) {
        let Some(&mark) = params.get(1) else {
            return;
        };
        if mark == MARK_PROMPT_START {
            self.scrollback.prompt_start();
        } else if mark == MARK_COMMAND_START {
            self.scrollback.command_start();
        } else if mark == MARK_OUTPUT_START {
            self.scrollback.output_start();
        } else if mark == MARK_COMMAND_END {
            self.scrollback.command_end(parse_exit_code(params.get(2)));
        }
    }

    fn handle_cwd(&mut self, params: &[&[u8]]) {
        if let Some(path) = params.get(1).and_then(|raw| cwd_from_osc7(raw)) {
            self.scrollback.set_cwd(path);
        }
    }

    fn handle_title(&mut self, params: &[&[u8]]) {
        if let Some(title_bytes) = params.get(1) {
            if let Ok(title) = std::str::from_utf8(title_bytes) {
                if !title.is_empty() {
                    self.pending_title = Some(title.to_string());
                }
            }
        }
    }

    /// `OSC 8 ; params ; URI` opens a hyperlink; an empty URI closes it. The URI
    /// is rejoined in case it contains `;`.
    fn handle_hyperlink(&mut self, params: &[&[u8]]) {
        let uri = join_fields(params.get(2..).unwrap_or_default());
        self.scrollback.set_link((!uri.is_empty()).then_some(uri));
    }

    fn handle_tbp(&mut self, params: &[&[u8]]) {
        let body = join_fields(params);
        let dir = self.side_channel_dir.clone();
        let result = if dir.is_some() {
            let dir_clone = dir.clone();
            spaceterm_proto::decode_with_sidechannel(&body, move |path| {
                let base = dir_clone.as_ref().unwrap();
                let full_path = format!("{}/{}", base, path);
                if full_path.contains("..") {
                    return Err(spaceterm_proto::ProtoError::MalformedFrame);
                }
                std::fs::read(&full_path)
                    .map_err(|_| spaceterm_proto::ProtoError::MalformedFrame)
            })
        } else {
            spaceterm_proto::decode(&body)
        };
        match result {
            Ok(Message::Emit(block)) => self.scrollback.emit(block),
            Ok(Message::Open(open)) => self.scrollback.open(open),
            Ok(Message::Patch(patch)) => self.scrollback.patch(patch),
            Ok(Message::Close(id)) => self.scrollback.close(id),
            Ok(Message::Caps) => {}
            Err(_) => {}
        }
    }

    /// Normalize an iTerm2 inline image (`OSC 1337 ; File=<args>:<base64>`) into a
    /// content block so it flows through the same path as a TBP image.
    fn handle_iterm2(&mut self, params: &[&[u8]]) {
        if let Some(block) = self.iterm2_image_block(params) {
            self.scrollback.emit(block);
        }
    }

    fn iterm2_image_block(&mut self, params: &[&[u8]]) -> Option<EmitBlock> {
        let body = join_fields(params.get(1..)?);
        let file = body.strip_prefix(ITERM2_FILE_PREFIX)?;
        let (_args, data_b64) = file.split_once(ITERM2_DATA_SEP)?;
        let bytes = BASE64_STANDARD.decode(data_b64).ok()?;

        let mut bundle = MimeBundle::new();
        bundle.insert(sniff_image_mime(&bytes), Value::from(data_b64));
        bundle.insert(TEXT_PLAIN, Value::from(IMAGE_FALLBACK));
        Some(EmitBlock {
            bundle,
            id: self.next_synthetic_id(),
            trust: TrustTier::default(),
        })
    }

    fn next_synthetic_id(&mut self) -> BlockId {
        self.synthetic_id += 1;
        BlockId(self.synthetic_id)
    }
}

impl Perform for Performer {
    fn print(&mut self, c: char) {
        let mut buf = [0u8; 4];
        self.scrollback.print(c.encode_utf8(&mut buf));
    }

    fn execute(&mut self, byte: u8) {
        if byte == NEWLINE {
            self.scrollback.line_break();
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        self.route_osc(params);
    }

    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _action: char) {}

    fn put(&mut self, _byte: u8) {}

    fn unhook(&mut self) {}

    fn csi_dispatch(
        &mut self,
        _params: &Params,
        _intermediates: &[u8],
        _ignore: bool,
        _action: char,
    ) {
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, _byte: u8) {}
}

// ============================================================================
// OSC helpers
// ============================================================================

fn join_fields(params: &[&[u8]]) -> String {
    params
        .iter()
        .map(|field| String::from_utf8_lossy(field))
        .collect::<Vec<_>>()
        .join(FIELD_SEP)
}

fn parse_exit_code(field: Option<&&[u8]>) -> Option<i32> {
    let raw = field?;
    std::str::from_utf8(raw).ok()?.parse().ok()
}

fn sniff_image_mime(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(PNG_MAGIC) {
        "image/png"
    } else if bytes.starts_with(JPEG_MAGIC) {
        "image/jpeg"
    } else if bytes.starts_with(GIF_MAGIC) {
        "image/gif"
    } else {
        DEFAULT_IMAGE_MIME
    }
}

fn cwd_from_osc7(value: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(value).ok()?;
    let without_scheme = text.strip_prefix(FILE_URI_SCHEME).unwrap_or(text);
    // The value is `file://<host>/<path>`; drop the host by keeping from the
    // first slash. A schemeless or hostless value is returned as-is.
    let path = match without_scheme.find('/') {
        Some(slash) => &without_scheme[slash..],
        None => without_scheme,
    };
    Some(path.to_string())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use spaceterm_proto::{BlockId, EmitBlock, MimeBundle, TrustTier};

    use super::*;
    use crate::block::Segment;

    fn feed(bytes: &[u8]) -> Terminal {
        let mut term = Terminal::new();
        term.feed(bytes);
        term
    }

    #[test]
    fn test_plain_text_lands_in_scrollback() {
        let term = feed(b"hello world");
        assert_eq!(term.scrollback().plain_text(), "hello world");
    }

    #[test]
    fn test_osc133_marks_delimit_a_command() {
        // prompt 'A', command line 'B', output 'C', then end 'D' with exit 0.
        let stream = b"\x1b]133;A\x1b\\$ \x1b]133;B\x1b\\ls\x1b]133;C\x1b\\out\n\x1b]133;D;0\x1b\\";
        let term = feed(stream);
        let block = &term.scrollback().blocks()[0];
        assert_eq!(block.command, "ls");
        assert_eq!(block.exit_code, Some(0));
        assert_eq!(term.scrollback().plain_text(), "out\n");
    }

    #[test]
    fn test_osc7_sets_working_directory() {
        let term = feed(b"\x1b]7;file://host/home/user\x1b\\");
        assert_eq!(
            term.scrollback().blocks()[0].cwd,
            Some("/home/user".to_string())
        );
    }

    #[test]
    fn test_tbp_emit_surfaces_as_content_block() {
        let mut bundle = MimeBundle::new();
        bundle.insert("text/plain", serde_json::Value::from("fallback"));
        bundle.insert("image/svg+xml", serde_json::Value::from("<svg/>"));
        let emitted = EmitBlock {
            bundle,
            id: BlockId(42),
            trust: TrustTier::Restricted,
        };
        let escape = spaceterm_proto::encode(&Message::Emit(emitted.clone()));

        let term = feed(escape.as_bytes());
        let content: Vec<&EmitBlock> = term.scrollback().blocks()[0]
            .output
            .iter()
            .filter_map(|segment| match segment {
                Segment::Content(block) => Some(block),
                Segment::Link(_) | Segment::Live(_) | Segment::Text(_) => None,
            })
            .collect();
        assert_eq!(content, vec![&emitted]);
    }

    #[test]
    fn test_iterm2_inline_image_becomes_content_block() {
        let png = [0x89u8, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        let data = BASE64_STANDARD.encode(png);
        let escape = format!("\x1b]1337;File=name=Zm9v;inline=1:{data}\x1b\\");

        let term = feed(escape.as_bytes());
        match &term.scrollback().blocks()[0].output[..] {
            [Segment::Content(block)] => {
                assert_eq!(
                    block.bundle.get("image/png"),
                    Some(&serde_json::Value::from(data))
                );
            }
            other => panic!("expected one content block, got {other:?}"),
        }
    }

    #[test]
    fn test_osc8_hyperlink_wraps_text_in_a_link_segment() {
        let term = feed(b"\x1b]8;;https://example.com\x1b\\click\x1b]8;;\x1b\\");
        match &term.scrollback().blocks()[0].output[..] {
            [Segment::Link(span)] => {
                assert_eq!(span.url, "https://example.com");
                assert_eq!(span.text, "click");
            }
            other => panic!("expected one link segment, got {other:?}"),
        }
    }

    #[test]
    fn test_malformed_tbp_escape_is_ignored() {
        let term = feed(b"\x1b]9001;emit;id=1;not!base64!\x1b\\after");
        // The bad block produced nothing; surrounding text still parses.
        assert_eq!(term.scrollback().plain_text(), "after");
    }

    #[test]
    fn test_tbp_osc_constant_matches_proto() {
        let parsed: u32 = std::str::from_utf8(OSC_TBP).unwrap().parse().unwrap();
        assert_eq!(parsed, spaceterm_proto::OSC_NUMBER);
    }

    #[test]
    fn test_tbp_open_patch_close_through_feed() {
        use spaceterm_proto::{Message, OpenBlock, PatchBlock};

        let open = Message::Open(OpenBlock {
            id: BlockId(5),
            mime: "text/markdown".to_string(),
            spec: serde_json::json!("# hello"),
        });
        let patch = Message::Patch(PatchBlock {
            id: BlockId(5),
            patch: serde_json::json!([{"op": "replace", "path": "/0", "value": "# world"}]),
        });
        let close = Message::Close(BlockId(5));

        let open_esc = spaceterm_proto::encode(&open);
        let patch_esc = spaceterm_proto::encode(&patch);
        let close_esc = spaceterm_proto::encode(&close);

        let mut stream = Vec::new();
        stream.extend_from_slice(b"before ");
        stream.extend_from_slice(open_esc.as_bytes());
        stream.extend_from_slice(patch_esc.as_bytes());
        stream.extend_from_slice(close_esc.as_bytes());
        stream.extend_from_slice(b" after");

        let term = feed(&stream);
        assert_eq!(term.scrollback().plain_text(), "before  after");

        match &term.scrollback().blocks()[0].output[..] {
            [Segment::Text(_), Segment::Live(live), Segment::Text(_)] => {
                assert_eq!(live.id, BlockId(5));
                assert_eq!(live.patches.len(), 1);
            }
            other => panic!("expected text/live/text, got {other:?}"),
        }
    }

    #[test]
    fn test_osc2_sets_pending_title() {
        let mut term = feed(b"\x1b]2;my title\x1b\\");
        assert_eq!(term.take_title(), Some("my title".to_string()));
        assert_eq!(term.take_title(), None);
    }

    #[test]
    fn test_osc0_sets_pending_title() {
        let mut term = feed(b"\x1b]0;icon and title\x1b\\");
        assert_eq!(term.take_title(), Some("icon and title".to_string()));
    }
}
