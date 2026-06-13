//! Detect a file's MIME type and build the content block to emit.

use std::path::Path;

use base64::Engine;
use serde_json::Value;
use spaceterm_proto::{BlockId, EmitBlock, MimeBundle, TrustTier, TEXT_PLAIN};

// ========================================================================
// Constants
// ========================================================================

const GIF_MAGIC: &[u8] = b"GIF8";
const JPEG_MAGIC: &[u8] = &[0xFF, 0xD8, 0xFF];
const PNG_MAGIC: &[u8] = &[0x89, b'P', b'N', b'G'];

// ========================================================================
// Data Structures
// ========================================================================

/// A file's detected MIME type and how its bytes map onto a representation.
/// `binary` (raster images) is rendered from base64; everything else is text.
pub struct Kind {
    pub binary: bool,
    pub mime: &'static str,
}

// ========================================================================
// Functions
// ========================================================================

/// The MIME type and representation for `path`, taken from its extension, then
/// magic-byte sniffing, then `text/plain`.
pub fn detect(path: &Path, bytes: &[u8]) -> Kind {
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        match ext.to_ascii_lowercase().as_str() {
            "png" => {
                return Kind {
                    binary: true,
                    mime: "image/png",
                }
            }
            "jpg" | "jpeg" => {
                return Kind {
                    binary: true,
                    mime: "image/jpeg",
                }
            }
            "gif" => {
                return Kind {
                    binary: true,
                    mime: "image/gif",
                }
            }
            "webp" => {
                return Kind {
                    binary: true,
                    mime: "image/webp",
                }
            }
            "svg" => {
                return Kind {
                    binary: false,
                    mime: "image/svg+xml",
                }
            }
            "md" | "markdown" => {
                return Kind {
                    binary: false,
                    mime: "text/markdown",
                }
            }
            "html" | "htm" => {
                return Kind {
                    binary: false,
                    mime: "text/html",
                }
            }
            "csv" => {
                return Kind {
                    binary: false,
                    mime: "text/csv",
                }
            }
            "json" => {
                return Kind {
                    binary: false,
                    mime: "application/json",
                }
            }
            "tex" | "latex" => {
                return Kind {
                    binary: false,
                    mime: "text/latex",
                }
            }
            _ => {}
        }
    }
    sniff_magic(bytes)
}

/// The plain-text fallback printed when SpaceTerm is not the active terminal: a
/// short descriptor for images, the raw text for everything else.
pub fn fallback(path: &Path, kind: &Kind, bytes: &[u8]) -> String {
    if kind.binary {
        format!("[spacecat: {} ({})]", path.display(), kind.mime)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// Build a block whose payload is carried inline (base64 for images, raw text
/// otherwise), always with a `text/plain` fallback. Used when no side-channel is
/// available; large inline payloads can exceed the terminal's OSC length limit.
pub fn inline_emit(
    kind: &Kind,
    bytes: &[u8],
    fallback: &str,
    id: u64,
    trust: TrustTier,
) -> EmitBlock {
    let mut bundle = MimeBundle::new();
    if kind.binary {
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        bundle.insert(kind.mime, Value::from(encoded));
    } else {
        bundle.insert(
            kind.mime,
            Value::from(String::from_utf8_lossy(bytes).into_owned()),
        );
    }
    bundle.insert(TEXT_PLAIN, Value::from(fallback.to_string()));
    EmitBlock {
        bundle,
        id: BlockId(id),
        trust,
    }
}

/// Recognize raster images by their leading bytes; default to `text/plain`.
fn sniff_magic(bytes: &[u8]) -> Kind {
    if bytes.starts_with(PNG_MAGIC) {
        Kind {
            binary: true,
            mime: "image/png",
        }
    } else if bytes.starts_with(JPEG_MAGIC) {
        Kind {
            binary: true,
            mime: "image/jpeg",
        }
    } else if bytes.starts_with(GIF_MAGIC) {
        Kind {
            binary: true,
            mime: "image/gif",
        }
    } else {
        Kind {
            binary: false,
            mime: TEXT_PLAIN,
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn png_extension_is_binary_image_with_plain_fallback() {
        let kind = detect(Path::new("c.png"), b"\x89PNG..");
        assert!(kind.binary);
        assert_eq!(kind.mime, "image/png");

        let fb = fallback(Path::new("c.png"), &kind, b"\x89PNG..");
        assert!(fb.contains("c.png"));
        let block = inline_emit(&kind, b"\x89PNG..", &fb, 1, TrustTier::Restricted);
        assert!(block.bundle.get("image/png").is_some());
        assert!(block.bundle.text_plain().is_some());
    }

    #[test]
    fn unknown_extension_falls_back_to_magic_then_plain() {
        assert!(detect(Path::new("noext"), b"\x89PNG..").binary);

        let kind = detect(Path::new("notes"), b"hello");
        assert!(!kind.binary);
        assert_eq!(kind.mime, TEXT_PLAIN);
        assert_eq!(fallback(Path::new("notes"), &kind, b"hello"), "hello");
    }
}
