//! TBP messages: the verbs a tool sends to the terminal.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::bundle::MimeBundle;
use crate::tier::TrustTier;
use crate::BlockId;

// ============================================================================
// Data Structures
// ============================================================================

/// A single TBP message, decoded from one OSC escape.
#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    /// Query terminal capabilities. The reply travels out of band (see
    /// [`crate::Caps`]), not as another `Message`.
    Caps,
    /// Close a live block.
    Close(BlockId),
    /// Emit a one-shot block.
    Emit(EmitBlock),
    /// Open a live block for subsequent incremental updates.
    Open(OpenBlock),
    /// Apply an incremental update to a live block.
    Patch(PatchBlock),
}

/// Payload of [`Message::Emit`]: a complete block rendered once.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EmitBlock {
    pub bundle: MimeBundle,
    pub id: BlockId,
    pub trust: TrustTier,
}

/// Payload of [`Message::Open`]: the initial state of a live block.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct OpenBlock {
    pub id: BlockId,
    pub mime: String,
    pub spec: Value,
}

/// Payload of [`Message::Patch`]: an RFC 6902 JSON patch applied to a live block.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PatchBlock {
    pub id: BlockId,
    pub patch: Value,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    #[test]
    fn test_message_equality_distinguishes_variants() {
        let mut bundle = MimeBundle::new();
        bundle.insert("text/plain", Value::from("hi"));
        let emit = Message::Emit(EmitBlock {
            bundle,
            id: BlockId(1),
            trust: TrustTier::Restricted,
        });
        assert_ne!(emit, Message::Caps);
        assert_ne!(emit, Message::Close(BlockId(1)));
    }

    #[test]
    fn test_emit_block_fields_round_trip() {
        let mut bundle = MimeBundle::new();
        bundle.insert("text/plain", Value::from("fallback"));
        let block = EmitBlock {
            bundle: bundle.clone(),
            id: BlockId(99),
            trust: TrustTier::Trusted,
        };
        assert_eq!(block.id, BlockId(99));
        assert_eq!(block.trust, TrustTier::Trusted);
        assert_eq!(
            block.bundle.get("text/plain"),
            Some(&Value::from("fallback"))
        );
    }

    #[test]
    fn test_open_block_holds_spec() {
        let spec = serde_json::json!({"mark": "bar"});
        let block = OpenBlock {
            id: BlockId(7),
            mime: "application/vnd.vega-lite+json".to_string(),
            spec: spec.clone(),
        };
        assert_eq!(block.id, BlockId(7));
        assert_eq!(block.mime, "application/vnd.vega-lite+json");
        assert_eq!(block.spec, spec);
    }

    #[test]
    fn test_patch_block_holds_json_patch() {
        let patch = serde_json::json!([{"op": "replace", "path": "/x", "value": 1}]);
        let block = PatchBlock {
            id: BlockId(3),
            patch: patch.clone(),
        };
        assert_eq!(block.id, BlockId(3));
        assert_eq!(block.patch, patch);
    }
}
