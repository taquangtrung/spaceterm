//! OSC 9001 framing: encode TBP messages onto, and decode them from, the byte
//! stream.
//!
//! A message is one escape: `OSC 9001 ; <verb> ; <params> ; <base64 payload> ST`,
//! where `OSC` is `ESC ]`, `ST` is `ESC \`, fields are separated by `;`,
//! parameters within the params field are `key=value` pairs separated by `,`,
//! and the payload (when present) is base64-encoded JSON. Keeping the payload
//! base64 inside a single OSC keeps the escape opaque to dumb terminals, which
//! ignore it wholesale.

use std::collections::BTreeMap;
use std::str::FromStr;

use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

use crate::bundle::MimeBundle;
use crate::message::{EmitBlock, Message, OpenBlock, PatchBlock};
use crate::tier::TrustTier;
use crate::{BlockId, OSC_NUMBER, PROTOCOL_VERSION};

// ============================================================================
// Constants
// ============================================================================

/// OSC introducer: `ESC ]`.
const OSC_START: &str = "\x1b]";

/// String Terminator: `ESC \`.
const ST: &str = "\x1b\\";

const FIELD_SEP: char = ';';
const PARAM_SEP: char = ',';
const KEY_VALUE_SEP: char = '=';

const VERB_CAPS: &str = "caps";
const VERB_CLOSE: &str = "close";
const VERB_EMIT: &str = "emit";
const VERB_OPEN: &str = "open";
const VERB_PATCH: &str = "patch";

const PARAM_FILE: &str = "file";
const PARAM_ID: &str = "id";
const PARAM_MIME: &str = "mime";
const PARAM_TRUST: &str = "trust";
const PARAM_VERSION: &str = "v";

// ============================================================================
// Data Structures
// ============================================================================

/// Failure decoding a TBP escape.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ProtoError {
    #[error("invalid value for parameter `{0}`")]
    BadParam(String),
    #[error("payload is not valid base64")]
    Base64,
    #[error("payload is not valid JSON")]
    Json,
    #[error("malformed TBP frame")]
    MalformedFrame,
    #[error("missing required parameter `{0}`")]
    MissingParam(String),
    #[error("unknown TBP verb `{0}`")]
    UnknownVerb(String),
    #[error("escape is not a TBP message")]
    WrongOsc,
}

// ============================================================================
// Encoding
// ============================================================================

/// Render a message as a complete escape sequence ready to write to the stream.
pub fn encode(message: &Message) -> String {
    match message {
        Message::Caps => frame(VERB_CAPS, &[], None),
        Message::Close(id) => frame(VERB_CLOSE, &[(PARAM_ID, id.0.to_string())], None),
        Message::Emit(block) => encode_emit(block),
        Message::Open(block) => encode_open(block),
        Message::Patch(block) => encode_patch(block),
    }
}

fn encode_emit(block: &EmitBlock) -> String {
    let params = [
        (PARAM_VERSION, PROTOCOL_VERSION.0.to_string()),
        (PARAM_ID, block.id.0.to_string()),
        (PARAM_TRUST, block.trust.as_str().to_string()),
    ];
    frame(VERB_EMIT, &params, Some(encode_json(&block.bundle)))
}

/// Render a file-referenced emit: the block carries a `file=` reference (a name
/// the terminal resolves through its side-channel directory) and its `mime`,
/// instead of an inline base64 payload. This keeps the escape tiny regardless of
/// payload size, so large images are not dropped by the terminal's OSC length
/// limit. The reader side is [`decode_with_sidechannel`].
pub fn encode_emit_file(id: BlockId, trust: TrustTier, file_name: &str, mime: &str) -> String {
    let params = [
        (PARAM_VERSION, PROTOCOL_VERSION.0.to_string()),
        (PARAM_ID, id.0.to_string()),
        (PARAM_TRUST, trust.as_str().to_string()),
        (PARAM_MIME, mime.to_string()),
        (PARAM_FILE, file_name.to_string()),
    ];
    frame(VERB_EMIT, &params, None)
}

fn encode_open(block: &OpenBlock) -> String {
    let params = [
        (PARAM_ID, block.id.0.to_string()),
        (PARAM_MIME, block.mime.clone()),
    ];
    frame(VERB_OPEN, &params, Some(encode_json(&block.spec)))
}

fn encode_patch(block: &PatchBlock) -> String {
    let params = [(PARAM_ID, block.id.0.to_string())];
    frame(VERB_PATCH, &params, Some(encode_json(&block.patch)))
}

fn frame(verb: &str, params: &[(&str, String)], payload: Option<String>) -> String {
    let mut out = String::from(OSC_START);
    out.push_str(&OSC_NUMBER.to_string());
    out.push(FIELD_SEP);
    out.push_str(verb);
    if !params.is_empty() {
        out.push(FIELD_SEP);
        for (index, (key, value)) in params.iter().enumerate() {
            if index > 0 {
                out.push(PARAM_SEP);
            }
            out.push_str(key);
            out.push(KEY_VALUE_SEP);
            out.push_str(value);
        }
    }
    if let Some(payload) = payload {
        out.push(FIELD_SEP);
        out.push_str(&payload);
    }
    out.push_str(ST);
    out
}

fn encode_json(value: &impl Serialize) -> String {
    let json = serde_json::to_vec(value).expect("proto value is always serializable");
    BASE64_STANDARD.encode(json)
}

// ============================================================================
// Decoding
// ============================================================================

/// Parse the content of one OSC escape (the bytes between `ESC ]` and `ST`, with
/// the introducer and terminator already stripped) into a [`Message`].
pub fn decode(body: &str) -> Result<Message, ProtoError> {
    decode_with_sidechannel(body, |_| Err(ProtoError::MalformedFrame))
}

/// Like [`decode`], but resolves `file=` references through `file_reader`.
/// When an emit carries a `file=RELATIVE_PATH` parameter instead of an inline
/// base64 payload, `file_reader` is called with the relative path and must
/// return the raw file bytes. This avoids base64-encoding large payloads
/// (images, PDFs) over the PTY stream.
pub fn decode_with_sidechannel(
    body: &str,
    file_reader: impl Fn(&str) -> Result<Vec<u8>, ProtoError>,
) -> Result<Message, ProtoError> {
    let mut fields = body.split(FIELD_SEP);
    let osc = fields.next().ok_or(ProtoError::MalformedFrame)?;
    if osc.parse::<u32>().ok() != Some(OSC_NUMBER) {
        return Err(ProtoError::WrongOsc);
    }
    let verb = fields.next().ok_or(ProtoError::MalformedFrame)?;
    let rest: Vec<&str> = fields.collect();
    match verb {
        VERB_CAPS => Ok(Message::Caps),
        VERB_CLOSE => Ok(Message::Close(decode_id(&rest)?)),
        VERB_EMIT => decode_emit_with_sidechannel(&rest, &file_reader),
        VERB_OPEN => decode_open(&rest),
        VERB_PATCH => decode_patch(&rest),
        other => Err(ProtoError::UnknownVerb(other.to_string())),
    }
}

fn decode_emit_with_sidechannel(
    rest: &[&str],
    file_reader: &impl Fn(&str) -> Result<Vec<u8>, ProtoError>,
) -> Result<Message, ProtoError> {
    let params = parse_params(rest.first().copied().unwrap_or_default());
    let id = required_id(&params)?;
    let trust = match params.get(PARAM_TRUST) {
        Some(raw) => {
            TrustTier::from_str(raw).map_err(|_| ProtoError::BadParam(PARAM_TRUST.to_string()))?
        }
        None => TrustTier::default(),
    };

    let bundle = if let Some(&file_path) = params.get(PARAM_FILE) {
        let raw_bytes = file_reader(file_path)?;
        let b64 = BASE64_STANDARD.encode(&raw_bytes);
        let mime = params
            .get(PARAM_MIME)
            .copied()
            .unwrap_or("application/octet-stream");
        let mut bundle = MimeBundle::new();
        bundle.insert(mime, Value::from(b64));
        bundle
    } else {
        decode_payload(rest)?
    };

    Ok(Message::Emit(EmitBlock { bundle, id, trust }))
}

fn decode_open(rest: &[&str]) -> Result<Message, ProtoError> {
    let params = parse_params(rest.first().copied().unwrap_or_default());
    let id = required_id(&params)?;
    let mime = params
        .get(PARAM_MIME)
        .ok_or_else(|| ProtoError::MissingParam(PARAM_MIME.to_string()))?
        .to_string();
    let spec: Value = decode_payload(rest)?;
    Ok(Message::Open(OpenBlock { id, mime, spec }))
}

fn decode_patch(rest: &[&str]) -> Result<Message, ProtoError> {
    let params = parse_params(rest.first().copied().unwrap_or_default());
    let id = required_id(&params)?;
    let patch: Value = decode_payload(rest)?;
    Ok(Message::Patch(PatchBlock { id, patch }))
}

fn decode_id(rest: &[&str]) -> Result<BlockId, ProtoError> {
    required_id(&parse_params(rest.first().copied().unwrap_or_default()))
}

fn parse_params(field: &str) -> BTreeMap<&str, &str> {
    field
        .split(PARAM_SEP)
        .filter(|pair| !pair.is_empty())
        .filter_map(|pair| pair.split_once(KEY_VALUE_SEP))
        .collect()
}

fn required_id(params: &BTreeMap<&str, &str>) -> Result<BlockId, ProtoError> {
    let raw = params
        .get(PARAM_ID)
        .ok_or_else(|| ProtoError::MissingParam(PARAM_ID.to_string()))?;
    raw.parse::<u64>()
        .map(BlockId)
        .map_err(|_| ProtoError::BadParam(PARAM_ID.to_string()))
}

fn decode_payload<T: DeserializeOwned>(rest: &[&str]) -> Result<T, ProtoError> {
    let payload = rest.get(1).ok_or(ProtoError::MalformedFrame)?;
    let bytes = BASE64_STANDARD
        .decode(payload)
        .map_err(|_| ProtoError::Base64)?;
    serde_json::from_slice(&bytes).map_err(|_| ProtoError::Json)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn strip_frame(escape: &str) -> &str {
        escape
            .strip_prefix(OSC_START)
            .and_then(|rest| rest.strip_suffix(ST))
            .expect("encoded escape is OSC-framed")
    }

    fn round_trip(message: &Message) -> Message {
        decode(strip_frame(&encode(message))).expect("re-decodes")
    }

    fn sample_emit() -> Message {
        let mut bundle = MimeBundle::new();
        bundle.insert("text/plain", Value::from("rows: 3"));
        bundle.insert("image/svg+xml", Value::from("<svg/>"));
        Message::Emit(EmitBlock {
            bundle,
            id: BlockId(42),
            trust: TrustTier::Trusted,
        })
    }

    #[test]
    fn test_emit_round_trips_through_the_wire() {
        let message = sample_emit();
        assert_eq!(round_trip(&message), message);
    }

    #[test]
    fn test_file_referenced_emit_resolves_via_sidechannel() {
        let escape = encode_emit_file(BlockId(7), TrustTier::Restricted, "img.bin", "image/png");
        // The escape stays tiny: no inline payload regardless of file size.
        assert!(!escape.contains(";9001;emit;") || escape.len() < 100);
        let message = decode_with_sidechannel(strip_frame(&escape), |path| {
            assert_eq!(path, "img.bin");
            Ok(b"RAWPNGBYTES".to_vec())
        })
        .expect("decodes");
        match message {
            Message::Emit(block) => {
                assert_eq!(block.id, BlockId(7));
                assert!(block.bundle.get("image/png").is_some());
            }
            other => panic!("expected emit, got {other:?}"),
        }
    }

    #[test]
    fn test_open_patch_close_round_trip() {
        let open = Message::Open(OpenBlock {
            id: BlockId(7),
            mime: "application/vnd.vega-lite+json".to_string(),
            spec: serde_json::json!({ "mark": "bar" }),
        });
        let patch = Message::Patch(PatchBlock {
            id: BlockId(7),
            patch: serde_json::json!([{ "op": "replace", "path": "/mark", "value": "line" }]),
        });
        let close = Message::Close(BlockId(7));
        assert_eq!(round_trip(&open), open);
        assert_eq!(round_trip(&patch), patch);
        assert_eq!(round_trip(&close), close);
    }

    #[test]
    fn test_caps_query_round_trips() {
        assert_eq!(round_trip(&Message::Caps), Message::Caps);
    }

    #[test]
    fn test_encoded_emit_is_framed_by_osc_and_st() {
        let escape = encode(&sample_emit());
        assert!(escape.starts_with(OSC_START));
        assert!(escape.ends_with(ST));
    }

    #[test]
    fn test_emit_without_trust_param_defaults_to_restricted() {
        // An emit escape carrying a payload but no `trust=` parameter.
        let bundle = encode_json(&MimeBundle::new());
        let body = format!("{OSC_NUMBER};{VERB_EMIT};{PARAM_ID}=5;{bundle}");
        match decode(&body) {
            Ok(Message::Emit(block)) => assert_eq!(block.trust, TrustTier::Restricted),
            other => panic!("expected emit, got {other:?}"),
        }
    }

    #[test]
    fn test_non_tbp_osc_is_rejected() {
        let body = "8;;https://example.com";
        assert_eq!(decode(body), Err(ProtoError::WrongOsc));
    }

    #[test]
    fn test_unknown_verb_is_reported() {
        let body = format!("{OSC_NUMBER};teleport;{PARAM_ID}=1");
        assert_eq!(
            decode(&body),
            Err(ProtoError::UnknownVerb("teleport".to_string()))
        );
    }

    #[test]
    fn test_emit_missing_id_is_reported() {
        let bundle = encode_json(&MimeBundle::new());
        let body = format!("{OSC_NUMBER};{VERB_EMIT};;{bundle}");
        assert_eq!(
            decode(&body),
            Err(ProtoError::MissingParam(PARAM_ID.to_string()))
        );
    }

    #[test]
    fn test_emit_with_corrupt_base64_payload_is_reported() {
        let body = format!("{OSC_NUMBER};{VERB_EMIT};{PARAM_ID}=1;not!base64!");
        assert_eq!(decode(&body), Err(ProtoError::Base64));
    }

    #[test]
    fn test_side_channel_emit_reads_file() {
        let png_bytes = vec![0x89u8, b'P', b'N', b'G'];
        let png_clone = png_bytes.clone();
        let body = format!(
            "{OSC_NUMBER};{VERB_EMIT};{PARAM_ID}=10,{PARAM_FILE}=test.png,{PARAM_MIME}=image/png"
        );
        let result = decode_with_sidechannel(&body, move |_path| Ok(png_clone.clone()));
        match result {
            Ok(Message::Emit(block)) => {
                assert_eq!(block.id, BlockId(10));
                let value = block.bundle.get("image/png").expect("has image/png");
                assert!(value.is_string());
                let decoded = BASE64_STANDARD.decode(value.as_str().unwrap()).unwrap();
                assert_eq!(decoded, png_bytes);
            }
            other => panic!("expected emit, got {other:?}"),
        }
    }

    #[test]
    fn test_side_channel_path_traversal_calls_reader() {
        let body = format!("{OSC_NUMBER};{VERB_EMIT};{PARAM_ID}=1,{PARAM_FILE}=../etc/passwd");
        let result = decode_with_sidechannel(&body, |path| {
            assert!(path.contains(".."));
            Err(ProtoError::MalformedFrame)
        });
        assert_eq!(result, Err(ProtoError::MalformedFrame));
    }

    #[test]
    fn test_side_channel_falls_back_to_inline_when_no_file_param() {
        let bundle = encode_json(&MimeBundle::new());
        let body = format!("{OSC_NUMBER};{VERB_EMIT};{PARAM_ID}=5;{bundle}");
        let result = decode_with_sidechannel(&body, |_path| Err(ProtoError::MalformedFrame));
        assert!(result.is_ok());
    }
}
