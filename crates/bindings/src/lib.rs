//! Node native addon exposing `SpaceTerm-core` to JavaScript.
//!
//! This is the bridge for the VSCode target: the extension feeds PTY bytes in and
//! reads the parsed block list back out as JSON, reusing the exact same `core`
//! pipeline as the standalone app. The block JSON shape is defined by the serde
//! derives on `spaceterm_core::CommandBlock` / `Segment`.

use napi::bindgen_prelude::Buffer;
use napi_derive::napi;
use spaceterm_core::Terminal;

// ============================================================================
// Data Structures
// ============================================================================

/// A terminal session driven from JavaScript.
#[napi(js_name = "Terminal")]
pub struct JsTerminal {
    inner: Terminal,
}

// ============================================================================
// JsTerminal
// ============================================================================

#[napi]
impl JsTerminal {
    /// Create a terminal with an empty scrollback.
    #[napi(constructor)]
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            inner: Terminal::new(),
        }
    }

    /// Feed a chunk of PTY output into the parser.
    #[napi]
    pub fn feed(&mut self, bytes: Buffer) {
        self.inner.feed(&bytes);
    }

    /// The current scrollback block list, serialized as JSON.
    #[napi]
    pub fn blocks_json(&self) -> String {
        self.inner.scrollback().to_json()
    }

    /// All plain text across the scrollback (search and smoke-test helper).
    #[napi]
    pub fn plain_text(&self) -> String {
        self.inner.scrollback().plain_text()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use spaceterm_core::Terminal;

    #[test]
    fn test_feed_then_plain_text() {
        let mut term = Terminal::new();
        term.feed(b"hello world");
        assert_eq!(term.scrollback().plain_text(), "hello world");
    }

    #[test]
    fn test_blocks_json_is_valid_json() {
        let mut term = Terminal::new();
        term.feed(b"test");
        let json = term.scrollback().to_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert!(parsed.is_array());
    }
}
