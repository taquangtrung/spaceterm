//! The scrollback state machine: turns semantic terminal events into a block list.
//!
//! This type is independent of `vte` and the PTY so it can be driven directly in
//! tests. The [`crate::parser`] adapter translates byte-stream escapes into the
//! method calls below.

use std::collections::HashMap;

use spaceterm_proto::{BlockId, EmitBlock, OpenBlock, PatchBlock};

use crate::block::{CommandBlock, LiveBlock, Segment};

// ============================================================================
// Data Structures
// ============================================================================

/// A session's output, modeled as an ordered list of command blocks.
#[derive(Clone, Debug)]
pub struct Scrollback {
    active_link: Option<String>,
    blocks: Vec<CommandBlock>,
    cwd: Option<String>,
    live_indices: HashMap<BlockId, LiveIndex>,
    phase: Phase,
}

/// Tracks where a live block lives in the block list so `patch`/`close` can
/// find it without scanning.
#[derive(Clone, Copy, Debug)]
struct LiveIndex {
    block_index: usize,
    segment_index: usize,
}

/// Which region of a command the stream is currently writing into. Driven by
/// OSC 133 marks; absent those, the stream stays in [`Phase::Output`] and fills a
/// single rolling block.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    /// Between the command-start and output-start marks: the command line itself.
    Input,
    /// Command output (and the default when shell integration is absent).
    Output,
    /// Between the prompt-start and command-start marks: the shell prompt.
    Prompt,
}

// ============================================================================
// Scrollback
// ============================================================================

impl Scrollback {
    /// A fresh scrollback holding one empty block, ready to capture output that
    /// precedes the first prompt.
    pub fn new() -> Self {
        Self {
            active_link: None,
            blocks: vec![CommandBlock::default()],
            cwd: None,
            live_indices: HashMap::new(),
            phase: Phase::Output,
        }
    }

    /// The command blocks captured so far, oldest first.
    pub fn blocks(&self) -> &[CommandBlock] {
        &self.blocks
    }

    /// For each block, the starting row offset assuming the first block starts at
    /// row 0. Used by the app layer to scroll to a specific block boundary.
    pub fn block_row_offsets(&self, cols: usize) -> Vec<usize> {
        let mut offsets = Vec::with_capacity(self.blocks.len());
        let mut row = 0;
        for block in &self.blocks {
            offsets.push(row);
            row += block.row_count(cols);
        }
        offsets
    }

    /// Search blocks for `query` (case-insensitive) and return indices of matching
    /// blocks. Text segments are searched; content blocks' text fallbacks are included.
    pub fn search(&self, query: &str) -> Vec<usize> {
        let lower = query.to_lowercase();
        self.blocks
            .iter()
            .enumerate()
            .filter(|(_, block)| {
                block.plain_text().to_lowercase().contains(&lower)
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// The block list serialized as JSON, the boundary form consumed by the Node
    /// addon and the `dump_session` harness.
    pub fn to_json(&self) -> String {
        serde_json::to_string(&self.blocks).expect("command blocks are always serializable")
    }

    /// All plain text across every block's output, concatenated in stream order.
    /// Content blocks are skipped. Useful for search and smoke tests.
    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        for block in &self.blocks {
            for segment in &block.output {
                if let Segment::Text(text) = segment {
                    out.push_str(text);
                }
            }
        }
        out
    }

    pub(crate) fn print(&mut self, text: &str) {
        match self.phase {
            Phase::Input => self.current_mut().command.push_str(text),
            Phase::Output => match self.active_link.clone() {
                Some(url) => self.current_mut().append_link(text, &url),
                None => self.current_mut().append_text(text),
            },
            Phase::Prompt => {}
        }
    }

    pub(crate) fn line_break(&mut self) {
        if self.phase == Phase::Output {
            self.current_mut().append_text("\n");
        }
    }

    pub(crate) fn prompt_start(&mut self) {
        if !self.current_mut().is_empty() {
            self.blocks.push(CommandBlock::default());
        }
        let cwd = self.cwd.clone();
        self.current_mut().cwd = cwd;
        self.active_link = None;
        self.phase = Phase::Prompt;
    }

    pub(crate) fn command_start(&mut self) {
        self.phase = Phase::Input;
    }

    pub(crate) fn output_start(&mut self) {
        self.phase = Phase::Output;
    }

    pub(crate) fn command_end(&mut self, exit_code: Option<i32>) {
        self.current_mut().exit_code = exit_code;
        self.phase = Phase::Output;
    }

    pub(crate) fn set_cwd(&mut self, cwd: String) {
        if self.current_mut().cwd.is_none() {
            self.current_mut().cwd = Some(cwd.clone());
        }
        self.cwd = Some(cwd);
    }

    pub(crate) fn emit(&mut self, block: EmitBlock) {
        self.current_mut().output.push(Segment::Content(block));
    }

    pub(crate) fn open(&mut self, open: OpenBlock) {
        let id = open.id;
        let live = LiveBlock {
            id,
            initial: open,
            patches: Vec::new(),
        };
        let block_index = self.blocks.len() - 1;
        let segment_index = self.current_mut().output.len();
        self.current_mut().output.push(Segment::Live(live));
        self.live_indices.insert(
            id,
            LiveIndex {
                block_index,
                segment_index,
            },
        );
    }

    pub(crate) fn patch(&mut self, patch: PatchBlock) {
        if let Some(idx) = self.live_indices.get(&patch.id).copied() {
            if let Some(Segment::Live(live)) = self
                .blocks
                .get_mut(idx.block_index)
                .and_then(|b| b.output.get_mut(idx.segment_index))
            {
                live.patches.push(patch.patch);
            }
        }
    }

    pub(crate) fn close(&mut self, id: BlockId) {
        self.live_indices.remove(&id);
    }

    /// Open (`Some`) or close (`None`) the active OSC 8 hyperlink target.
    pub(crate) fn set_link(&mut self, url: Option<String>) {
        self.active_link = url;
    }

    fn current_mut(&mut self) -> &mut CommandBlock {
        self.blocks
            .last_mut()
            .expect("scrollback always retains at least one block")
    }
}

impl Default for Scrollback {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use spaceterm_proto::{BlockId, EmitBlock, MimeBundle, OpenBlock, PatchBlock, TrustTier};
    use serde_json::Value;

    use super::*;
    use crate::block::LinkSpan;

    fn svg_block() -> EmitBlock {
        let mut bundle = MimeBundle::new();
        bundle.insert("image/svg+xml", Value::from("<svg/>"));
        EmitBlock {
            bundle,
            id: BlockId(1),
            trust: TrustTier::Restricted,
        }
    }

    #[test]
    fn test_text_without_marks_fills_one_rolling_block() {
        let mut sb = Scrollback::new();
        sb.print("hello");
        sb.line_break();
        assert_eq!(sb.blocks().len(), 1);
        assert_eq!(
            sb.blocks()[0].output,
            vec![Segment::Text("hello\n".to_string())]
        );
    }

    #[test]
    fn test_osc133_sequence_captures_command_and_exit() {
        let mut sb = Scrollback::new();
        sb.prompt_start();
        sb.print("user@host$ "); // prompt text is dropped
        sb.command_start();
        sb.print("ls");
        sb.output_start();
        sb.print("a\nb\n");
        sb.command_end(Some(0));

        let block = &sb.blocks()[0];
        assert_eq!(block.command, "ls");
        assert_eq!(block.output, vec![Segment::Text("a\nb\n".to_string())]);
        assert_eq!(block.exit_code, Some(0));
    }

    #[test]
    fn test_second_prompt_opens_a_new_block() {
        let mut sb = Scrollback::new();
        sb.prompt_start();
        sb.command_start();
        sb.print("true");
        sb.output_start();
        sb.command_end(Some(0));
        sb.prompt_start();
        sb.command_start();
        sb.print("false");
        assert_eq!(sb.blocks().len(), 2);
        assert_eq!(sb.blocks()[1].command, "false");
    }

    #[test]
    fn test_emitted_block_appends_as_content_segment() {
        let mut sb = Scrollback::new();
        sb.print("before");
        sb.emit(svg_block());
        assert_eq!(
            sb.blocks()[0].output,
            vec![
                Segment::Text("before".to_string()),
                Segment::Content(svg_block())
            ]
        );
    }

    #[test]
    fn test_active_link_groups_consecutive_text() {
        let mut sb = Scrollback::new();
        sb.set_link(Some("https://example.com".to_string()));
        sb.print("click ");
        sb.print("here");
        sb.set_link(None);
        sb.print(" plain");

        assert_eq!(
            sb.blocks()[0].output,
            vec![
                Segment::Link(LinkSpan {
                    text: "click here".to_string(),
                    url: "https://example.com".to_string(),
                }),
                Segment::Text(" plain".to_string()),
            ]
        );
    }

    #[test]
    fn test_to_json_tags_text_and_content_segments() {
        let mut sb = Scrollback::new();
        sb.print("hi");
        sb.emit(svg_block());
        let json = sb.to_json();
        assert!(json.contains(r#""kind":"text""#), "{json}");
        assert!(json.contains(r#""kind":"content""#), "{json}");
    }

    #[test]
    fn test_cwd_is_inherited_by_next_block() {
        let mut sb = Scrollback::new();
        sb.set_cwd("/home/user".to_string());
        sb.prompt_start();
        assert_eq!(
            sb.blocks().last().unwrap().cwd,
            Some("/home/user".to_string())
        );
    }

    #[test]
    fn test_open_creates_live_segment() {
        let mut sb = Scrollback::new();
        sb.open(OpenBlock {
            id: BlockId(10),
            mime: "text/markdown".to_string(),
            spec: serde_json::json!("initial"),
        });
        assert_eq!(sb.blocks()[0].output.len(), 1);
        match &sb.blocks()[0].output[0] {
            Segment::Live(live) => {
                assert_eq!(live.id, BlockId(10));
                assert!(live.patches.is_empty());
            }
            other => panic!("expected Live, got {other:?}"),
        }
    }

    #[test]
    fn test_patch_appends_to_live_block() {
        let mut sb = Scrollback::new();
        sb.open(OpenBlock {
            id: BlockId(20),
            mime: "text/plain".to_string(),
            spec: serde_json::json!("v0"),
        });
        sb.patch(PatchBlock {
            id: BlockId(20),
            patch: serde_json::json!([{"op": "replace", "path": "/0", "value": "v1"}]),
        });
        match &sb.blocks()[0].output[0] {
            Segment::Live(live) => assert_eq!(live.patches.len(), 1),
            other => panic!("expected Live, got {other:?}"),
        }
    }

    #[test]
    fn test_close_removes_live_index() {
        let mut sb = Scrollback::new();
        sb.open(OpenBlock {
            id: BlockId(30),
            mime: "text/plain".to_string(),
            spec: serde_json::json!(null),
        });
        assert!(sb.live_indices.contains_key(&BlockId(30)));
        sb.close(BlockId(30));
        assert!(!sb.live_indices.contains_key(&BlockId(30)));
    }

    #[test]
    fn test_patch_for_unknown_id_is_ignored() {
        let mut sb = Scrollback::new();
        sb.patch(PatchBlock {
            id: BlockId(999),
            patch: serde_json::json!([{"op": "add", "path": "/x", "value": 1}]),
        });
        assert!(sb.blocks()[0].output.is_empty());
    }

    #[test]
    fn test_block_row_offsets_tracks_cumulative_rows() {
        let mut sb = Scrollback::new();
        sb.print("aaa\nbbb\n");
        sb.prompt_start();
        sb.command_start();
        sb.print("ls");
        sb.output_start();
        sb.print("output\n");
        let offsets = sb.block_row_offsets(80);
        assert_eq!(offsets.len(), 2);
        assert_eq!(offsets[0], 0);
        assert!(offsets[1] > 0);
    }

    #[test]
    fn test_search_finds_matching_blocks() {
        let mut sb = Scrollback::new();
        sb.print("hello world\n");
        sb.prompt_start();
        sb.command_start();
        sb.print("ls");
        sb.output_start();
        sb.print("goodbye\n");
        let results = sb.search("hello");
        assert_eq!(results, vec![0]);
        let results = sb.search("goodbye");
        assert_eq!(results, vec![1]);
    }

    #[test]
    fn test_search_is_case_insensitive() {
        let mut sb = Scrollback::new();
        sb.print("Hello World\n");
        let results = sb.search("hello");
        assert_eq!(results, vec![0]);
    }

    #[test]
    fn test_search_returns_empty_when_no_match() {
        let mut sb = Scrollback::new();
        sb.print("hello\n");
        let results = sb.search("xyz");
        assert!(results.is_empty());
    }
}
