//! Terminal Block Protocol (TBP) v1: wire types and codec.
//!
//! TBP is an OSC escape sequence carrying a MIME bundle, directly inspired by
//! Jupyter's `display_data` message. A tool emits a block as one escape; the
//! terminal selects the richest MIME representation it can render and falls back
//! toward `text/plain`. Terminals that do not understand TBP ignore the escape.
//!
//! The wire form is documented normatively in
//! `docs/terminal-block-protocol-spec.md`; this crate is its reference codec.
//! See [`encode`] and [`decode`] for the byte-stream entry points.

mod bundle;
mod caps;
mod message;
mod tier;
mod wire;

pub use bundle::{BlockMeta, MimeBundle, TEXT_PLAIN};
pub use caps::Caps;
pub use message::{EmitBlock, Message, OpenBlock, PatchBlock};
pub use tier::TrustTier;
pub use wire::{decode, decode_with_sidechannel, encode, ProtoError};

// ============================================================================
// Constants
// ============================================================================

/// Protocol version this crate emits and is the highest it accepts.
pub const PROTOCOL_VERSION: Version = Version(1);

/// Private-use OSC number that frames every TBP message. Provisional; the final
/// number is coordinated with other terminals before 1.0.
pub const OSC_NUMBER: u32 = 9001;

// ============================================================================
// Data Structures
// ============================================================================

/// TBP protocol version. Wire form is a bare integer.
#[derive(
    Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
#[serde(transparent)]
pub struct Version(pub u16);

/// Identifier correlating live-block updates (`open`/`patch`/`close`) and
/// distinguishing concurrent blocks within one session.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
#[serde(transparent)]
pub struct BlockId(pub u64);

// ============================================================================
// Version
// ============================================================================

impl Version {
    /// Whether a bundle declaring this version can be rendered by this crate.
    /// A terminal accepts any version at or below the one it implements.
    pub fn is_supported(self) -> bool {
        self.0 <= PROTOCOL_VERSION.0
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_current_version_is_supported() {
        assert!(PROTOCOL_VERSION.is_supported());
    }

    #[test]
    fn test_future_version_is_unsupported() {
        let future = Version(PROTOCOL_VERSION.0 + 1);
        assert!(!future.is_supported());
    }
}
