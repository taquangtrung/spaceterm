//! MIME bundles: the alternative representations carried by a block.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ============================================================================
// Constants
// ============================================================================

/// The mandatory fallback representation every well-formed bundle should carry.
pub const TEXT_PLAIN: &str = "text/plain";

// ============================================================================
// Data Structures
// ============================================================================

/// A set of alternative representations of one block, keyed by MIME type. The
/// terminal renders the richest type it supports and falls back toward
/// [`TEXT_PLAIN`]. Values are JSON: a string for text and image payloads, an
/// object for structured specs (e.g. Vega-Lite).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct MimeBundle {
    #[serde(default, skip_serializing_if = "BlockMeta::is_empty")]
    pub meta: BlockMeta,
    pub mime: BTreeMap<String, Value>,
}

/// Optional presentation hints attached to a bundle.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct BlockMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height_hint: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

// ============================================================================
// MimeBundle
// ============================================================================

impl MimeBundle {
    /// An empty bundle with no representations and no metadata.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add or replace the representation for one MIME type.
    pub fn insert(&mut self, mime: impl Into<String>, value: Value) {
        self.mime.insert(mime.into(), value);
    }

    /// The representation for a MIME type, if present.
    pub fn get(&self, mime: &str) -> Option<&Value> {
        self.mime.get(mime)
    }

    /// The mandatory `text/plain` fallback, if the tool supplied one.
    pub fn text_plain(&self) -> Option<&str> {
        self.get(TEXT_PLAIN).and_then(Value::as_str)
    }
}

// ============================================================================
// BlockMeta
// ============================================================================

impl BlockMeta {
    /// Whether every hint is absent, so the field can be omitted on the wire.
    pub fn is_empty(&self) -> bool {
        self.height_hint.is_none() && self.title.is_none()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_plain_returns_string_fallback() {
        let mut bundle = MimeBundle::new();
        bundle.insert(TEXT_PLAIN, Value::from("score: 0.92"));
        bundle.insert("image/svg+xml", Value::from("<svg/>"));
        assert_eq!(bundle.text_plain(), Some("score: 0.92"));
    }

    #[test]
    fn test_text_plain_absent_when_only_rich_types_present() {
        let mut bundle = MimeBundle::new();
        bundle.insert("image/svg+xml", Value::from("<svg/>"));
        assert_eq!(bundle.text_plain(), None);
    }

    #[test]
    fn test_non_string_text_plain_is_not_returned_as_str() {
        let mut bundle = MimeBundle::new();
        bundle.insert(TEXT_PLAIN, Value::from(42));
        assert_eq!(bundle.text_plain(), None);
    }
}
