//! Tracks rich content blocks surfaced from the core block parser.

use std::collections::HashMap;

use spaceterm_core::spaceterm_proto::{EmitBlock, TrustTier};
use spaceterm_core::Scrollback;
use spaceterm_core::Segment;

// ========================================================================
// Data Structures
// ========================================================================

/// A snapshot of all content blocks currently in the scrollback, with their
/// block-list indices for tracking new arrivals.
#[derive(Clone, Debug, Default)]
pub struct BlockQueue {
    entries: Vec<BlockEntry>,
    known_patches: HashMap<usize, usize>,
    scanned_segments: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct BlockEntry {
    pub block_index: usize,
    pub emit: EmitBlock,
    pub grid_row: usize,
    pub kind: BlockKind,
    pub segment_index: usize,
    pub trust: TrustTier,
}

/// Distinguishes one-shot content blocks from live blocks that accept patches.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockKind {
    Content,
    Live,
}

// ========================================================================
// BlockQueue
// ========================================================================

impl BlockQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Scan the scrollback for content segments not yet seen and append them.
    /// `cursor_row` is the grid row where new content was detected.
    pub fn update(&mut self, scrollback: &Scrollback, cursor_row: usize) {
        let blocks = scrollback.blocks();
        for (block_index, block) in blocks.iter().enumerate() {
            if block_index >= self.scanned_segments.len() {
                self.scanned_segments.push(0);
            }
            let start = self.scanned_segments[block_index];
            for (segment_index, segment) in block.output.iter().enumerate().skip(start) {
                match segment {
                    Segment::Content(emit) => {
                        self.entries.push(BlockEntry {
                            block_index,
                            emit: emit.clone(),
                            grid_row: cursor_row,
                            kind: BlockKind::Content,
                            segment_index,
                            trust: emit.trust,
                        });
                    }
                    Segment::Live(live) => {
                        let mut bundle = spaceterm_core::spaceterm_proto::MimeBundle::new();
                        bundle.insert(
                            &live.initial.mime,
                            serde_json::Value::String(live.initial.spec.to_string()),
                        );
                        let emit = EmitBlock {
                            bundle,
                            id: live.id,
                            trust: TrustTier::Restricted,
                        };
                        self.entries.push(BlockEntry {
                            block_index,
                            emit,
                            grid_row: cursor_row,
                            kind: BlockKind::Live,
                            segment_index,
                            trust: TrustTier::Restricted,
                        });
                    }
                    Segment::Link(_) | Segment::Text(_) => {}
                }
            }
            self.scanned_segments[block_index] = block.output.len();
        }
    }

    pub fn entries(&self) -> &[BlockEntry] {
        &self.entries
    }

    /// Returns indices of live-block entries that have accumulated new patches
    /// since the last call. Each call resets the tracked patch count.
    pub fn drain_patched_live(&mut self, blocks: &[spaceterm_core::CommandBlock]) -> Vec<usize> {
        let mut patched = Vec::new();
        for (idx, entry) in self.entries.iter().enumerate() {
            if entry.kind != BlockKind::Live {
                continue;
            }
            if let Some(block) = blocks.get(entry.block_index) {
                if let Some(Segment::Live(live)) = block.output.get(entry.segment_index) {
                    if live.patches.len() > self.known_patches.get(&idx).copied().unwrap_or(0) {
                        self.known_patches.insert(idx, live.patches.len());
                        patched.push(idx);
                    }
                }
            }
        }
        patched
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use spaceterm_core::spaceterm_proto::{BlockId, EmitBlock, MimeBundle, TrustTier};
    use spaceterm_core::Terminal;
    use serde_json::Value;

    use super::*;

    fn svg_emit() -> EmitBlock {
        let mut bundle = MimeBundle::new();
        bundle.insert("image/svg+xml", Value::from("<svg/>"));
        bundle.insert("text/plain", Value::from("[svg]"));
        EmitBlock {
            bundle,
            id: BlockId(1),
            trust: TrustTier::Restricted,
        }
    }

    fn emit_escape(block: &EmitBlock) -> Vec<u8> {
        spaceterm_core::spaceterm_proto::encode(&spaceterm_core::spaceterm_proto::Message::Emit(block.clone()))
            .into_bytes()
    }

    #[test]
    fn test_empty_queue_has_no_entries() {
        let queue = BlockQueue::new();
        assert!(queue.entries().is_empty());
    }

    #[test]
    fn test_update_collects_content_segments() {
        let mut term = Terminal::new();
        term.feed(b"before");
        term.feed(&emit_escape(&svg_emit()));
        term.feed(b"after");

        let mut queue = BlockQueue::new();
        queue.update(term.scrollback(), 0);
        assert_eq!(queue.entries().len(), 1);
        assert_eq!(queue.entries()[0].block_index, 0);
        assert_eq!(queue.entries()[0].segment_index, 1);
    }

    #[test]
    fn test_update_is_incremental() {
        let mut term = Terminal::new();
        term.feed(&emit_escape(&svg_emit()));

        let mut queue = BlockQueue::new();
        queue.update(term.scrollback(), 0);
        assert_eq!(queue.entries().len(), 1);

        let mut second = svg_emit();
        second.id = BlockId(2);
        term.feed(&emit_escape(&second));
        queue.update(term.scrollback(), 0);
        assert_eq!(queue.entries().len(), 2);
    }

    #[test]
    fn test_update_surfaces_live_blocks() {
        use spaceterm_core::spaceterm_proto::{Message, OpenBlock};

        let open = Message::Open(OpenBlock {
            id: BlockId(99),
            mime: "text/markdown".to_string(),
            spec: serde_json::json!("# live"),
        });
        let esc = spaceterm_core::spaceterm_proto::encode(&open);

        let mut term = Terminal::new();
        term.feed(esc.as_bytes());

        let mut queue = BlockQueue::new();
        queue.update(term.scrollback(), 5);
        assert_eq!(queue.entries().len(), 1);
        assert_eq!(queue.entries()[0].kind, BlockKind::Live);
        assert_eq!(queue.entries()[0].grid_row, 5);
    }
}
