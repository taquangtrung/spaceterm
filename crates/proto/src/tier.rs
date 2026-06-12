//! Trust tiers governing how much capability a block's content is granted.

use std::str::FromStr;

use serde::{Deserialize, Serialize};

// ============================================================================
// Data Structures
// ============================================================================

/// How much capability a block's rendered content is granted. The emitting tool
/// *requests* a tier; the terminal *clamps* it by policy.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TrustTier {
    /// Content from the network or AI output: sandboxed iframe, unique origin,
    /// no scripts unless explicitly opted in.
    Isolated,
    /// Unknown local CLIs (the default): CSP applied, no network, no top-level
    /// navigation.
    #[default]
    Restricted,
    /// First-party tools or a user-configured allowlist: full DOM and scripts.
    Trusted,
}

// ============================================================================
// TrustTier
// ============================================================================

impl TrustTier {
    /// The canonical wire spelling used in TBP escape parameters.
    pub fn as_str(self) -> &'static str {
        match self {
            TrustTier::Isolated => "isolated",
            TrustTier::Restricted => "restricted",
            TrustTier::Trusted => "trusted",
        }
    }
}

impl FromStr for TrustTier {
    type Err = UnknownTier;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "isolated" => Ok(TrustTier::Isolated),
            "restricted" => Ok(TrustTier::Restricted),
            "trusted" => Ok(TrustTier::Trusted),
            _ => Err(UnknownTier),
        }
    }
}

/// Returned when a parameter value is not a recognized trust tier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnknownTier;

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wire_spelling_round_trips() {
        for tier in [
            TrustTier::Isolated,
            TrustTier::Restricted,
            TrustTier::Trusted,
        ] {
            assert_eq!(TrustTier::from_str(tier.as_str()), Ok(tier));
        }
    }

    #[test]
    fn test_unknown_spelling_is_rejected() {
        assert_eq!(TrustTier::from_str("admin"), Err(UnknownTier));
    }
}
