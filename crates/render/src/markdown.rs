//! Minimal Markdown to styled text spans, for rendering markdown blocks
//! natively (laid out by `cosmic-text`, rasterized to a texture) instead of in
//! a WebView. Supports headings, bold/italic, inline and block code, and
//! bullet lists; richer constructs (tables, links, images) fall back to their
//! text. Block boundaries are encoded as newlines in the span text.

use pulldown_cmark::{Event, Parser, Tag, TagEnd};

// ========================================================================
// Data Structures
// ========================================================================

/// One run of text with a uniform style.
pub struct MdSpan {
    pub bold: bool,
    pub italic: bool,
    pub mono: bool,
    pub text: String,
}

// ========================================================================
// Functions
// ========================================================================

/// Parse `markdown` into styled spans. Newlines in `text` mark line and block
/// breaks; the caller lays the spans out as rich text.
pub fn parse(markdown: &str) -> Vec<MdSpan> {
    let mut spans = Vec::new();
    let mut bold = 0u32;
    let mut italic = 0u32;
    let mut heading = false;
    let mut code_block = false;

    let push = |spans: &mut Vec<MdSpan>, text: String, bold, italic, mono| {
        if !text.is_empty() {
            spans.push(MdSpan {
                bold,
                italic,
                mono,
                text,
            });
        }
    };

    for event in Parser::new(markdown) {
        match event {
            Event::Start(Tag::Strong) => bold += 1,
            Event::End(TagEnd::Strong) => bold = bold.saturating_sub(1),
            Event::Start(Tag::Emphasis) => italic += 1,
            Event::End(TagEnd::Emphasis) => italic = italic.saturating_sub(1),
            Event::Start(Tag::Heading { .. }) => heading = true,
            Event::End(TagEnd::Heading(_)) => {
                heading = false;
                push(&mut spans, "\n\n".to_string(), false, false, false);
            }
            Event::Start(Tag::Item) => push(&mut spans, "  • ".to_string(), false, false, false),
            Event::End(TagEnd::Item) => push(&mut spans, "\n".to_string(), false, false, false),
            Event::Start(Tag::CodeBlock(_)) => code_block = true,
            Event::End(TagEnd::CodeBlock) => {
                code_block = false;
                push(&mut spans, "\n".to_string(), false, false, false);
            }
            Event::End(TagEnd::Paragraph) => {
                push(&mut spans, "\n\n".to_string(), false, false, false)
            }
            Event::Text(text) => push(
                &mut spans,
                text.into_string(),
                bold > 0 || heading,
                italic > 0,
                code_block,
            ),
            Event::Code(text) => push(&mut spans, text.into_string(), false, false, true),
            Event::SoftBreak => push(&mut spans, " ".to_string(), false, false, false),
            Event::HardBreak => push(&mut spans, "\n".to_string(), false, false, false),
            _ => {}
        }
    }
    spans
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bold_and_code_spans_carry_style() {
        let spans = parse("a **b** `c`");
        let bold = spans.iter().find(|s| s.text == "b").expect("bold span");
        assert!(bold.bold);
        let code = spans.iter().find(|s| s.text == "c").expect("code span");
        assert!(code.mono);
    }
}
