//! The block-list scrollback model: typed output units.
//!
//! Scrollback is a list of [`CommandBlock`]s, not a flat line buffer. Each block
//! holds one command's output as an ordered sequence of [`Segment`]s, where a
//! segment is either plain terminal text or a rich content block emitted via TBP.

use spaceterm_proto::{EmitBlock, OpenBlock};
use serde::Serialize;

// ============================================================================
// Data Structures
// ============================================================================

/// One command's output unit: the command line, its working directory, exit
/// status, and the ordered segments it produced.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct CommandBlock {
    pub command: String,
    pub cwd: Option<String>,
    pub exit_code: Option<i32>,
    pub output: Vec<Segment>,
}

/// One run of output within a command block.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase", tag = "kind", content = "data")]
pub enum Segment {
    /// A rich block emitted via TBP (`OSC 9001 ; emit`).
    Content(EmitBlock),
    /// A hyperlinked run of text (`OSC 8`).
    Link(LinkSpan),
    /// A live block opened via TBP (`OSC 9001 ; open`), updated via `patch`.
    Live(LiveBlock),
    /// Plain terminal text.
    Text(String),
}

/// A live block that can be updated in-place via `patch` messages.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LiveBlock {
    pub id: spaceterm_proto::BlockId,
    pub initial: OpenBlock,
    pub patches: Vec<serde_json::Value>,
}

/// A run of text carrying an OSC 8 hyperlink target.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LinkSpan {
    pub text: String,
    pub url: String,
}

// ============================================================================
// CommandBlock
// ============================================================================

impl CommandBlock {
    /// Append text to the output, coalescing with a trailing text segment so a
    /// run of `print` calls does not fragment into one segment per character.
    pub(crate) fn append_text(&mut self, text: &str) {
        match self.output.last_mut() {
            Some(Segment::Text(buf)) => buf.push_str(text),
            Some(Segment::Content(_)) | Some(Segment::Link(_)) | Some(Segment::Live(_)) | None => {
                self.output.push(Segment::Text(text.to_string()))
            }
        }
    }

    /// Append hyperlinked text, coalescing with a trailing link to the same URL.
    pub(crate) fn append_link(&mut self, text: &str, url: &str) {
        match self.output.last_mut() {
            Some(Segment::Link(span)) if span.url == url => span.text.push_str(text),
            Some(Segment::Content(_))
            | Some(Segment::Link(_))
            | Some(Segment::Live(_))
            | Some(Segment::Text(_))
            | None => self.output.push(Segment::Link(LinkSpan {
                text: text.to_string(),
                url: url.to_string(),
            })),
        }
    }

    /// Whether the block has captured neither a command line nor any output, so
    /// it can be reused rather than leaving an empty block in the scrollback.
    pub(crate) fn is_empty(&self) -> bool {
        self.command.is_empty() && self.output.is_empty()
    }

    /// How many terminal rows this block occupies, given a column width.
    pub fn row_count(&self, cols: usize) -> usize {
        let text = self.plain_text();
        if text.is_empty() {
            return if self.command.is_empty() { 0 } else { 1 };
        }
        let mut rows = 0;
        for line in text.split('\n') {
            if cols == 0 {
                rows += 1;
            } else {
                rows += line.len().max(1).div_ceil(cols);
            }
        }
        rows.max(1)
    }

    /// All plain text in this block's output, concatenated.
    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        for segment in &self.output {
            if let Segment::Text(text) = segment {
                out.push_str(text);
            }
        }
        out
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use spaceterm_proto::{BlockId, EmitBlock, MimeBundle, TrustTier};

    use super::*;

    #[test]
    fn test_append_text_coalesces_consecutive_prints() {
        let mut block = CommandBlock::default();
        block.append_text("hel");
        block.append_text("lo");
        assert_eq!(block.output, vec![Segment::Text("hello".to_string())]);
    }

    #[test]
    fn test_append_text_separates_after_content() {
        let mut block = CommandBlock::default();
        block.append_text("before");
        let mut bundle = MimeBundle::new();
        bundle.insert("text/plain", serde_json::Value::from("img"));
        block.output.push(Segment::Content(EmitBlock {
            bundle,
            id: BlockId(1),
            trust: TrustTier::Restricted,
        }));
        block.append_text("after");
        assert_eq!(block.output.len(), 3);
        assert_eq!(&block.output[0], &Segment::Text("before".to_string()));
        assert_eq!(&block.output[2], &Segment::Text("after".to_string()));
    }

    #[test]
    fn test_append_link_coalesces_same_url() {
        let mut block = CommandBlock::default();
        block.append_link("click ", "https://example.com");
        block.append_link("here", "https://example.com");
        assert_eq!(
            block.output,
            vec![Segment::Link(LinkSpan {
                text: "click here".to_string(),
                url: "https://example.com".to_string(),
            })]
        );
    }

    #[test]
    fn test_append_link_separates_different_urls() {
        let mut block = CommandBlock::default();
        block.append_link("a", "https://a.com");
        block.append_link("b", "https://b.com");
        assert_eq!(block.output.len(), 2);
    }

    #[test]
    fn test_is_empty_on_fresh_block() {
        let block = CommandBlock::default();
        assert!(block.is_empty());
    }

    #[test]
    fn test_is_not_empty_after_appending_text() {
        let mut block = CommandBlock::default();
        block.append_text("x");
        assert!(!block.is_empty());
    }

    #[test]
    fn test_row_count_single_line() {
        let mut block = CommandBlock::default();
        block.append_text("hello");
        assert_eq!(block.row_count(80), 1);
    }

    #[test]
    fn test_row_count_wraps_long_line() {
        let mut block = CommandBlock::default();
        block.append_text("abcdefgh");
        assert_eq!(block.row_count(4), 2);
    }

    #[test]
    fn test_row_count_multi_line() {
        let mut block = CommandBlock::default();
        block.append_text("abc\ndef\nghi");
        assert_eq!(block.row_count(80), 3);
    }

    #[test]
    fn test_row_count_empty_block() {
        let block = CommandBlock::default();
        assert_eq!(block.row_count(80), 0);
    }

    #[test]
    fn test_row_count_command_only() {
        let mut block = CommandBlock::default();
        block.command = "ls".to_string();
        assert_eq!(block.row_count(80), 1);
    }
}
