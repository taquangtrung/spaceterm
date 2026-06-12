//! Wire protocol for the mux client-server connection.
//!
//! Messages are length-prefixed JSON frames: 4-byte big-endian length, then
//! UTF-8 JSON. This keeps the protocol debuggable and language-agnostic.

use serde::{Deserialize, Serialize};

// ========================================================================
// Data Structures
// ========================================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClientMessage {
    /// Attach to an existing session (or create if absent).
    Attach { session: String },
    /// Detach without killing the session.
    Detach,
    /// Send bytes to the PTY.
    Input { session: String, bytes: Vec<u8> },
    /// Resize the PTY.
    Resize { session: String, cols: u16, rows: u16 },
    /// List active sessions.
    ListSessions,
    /// Kill a session.
    Kill { session: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ServerMessage {
    /// PTY output batch.
    Output { session: String, bytes: Vec<u8> },
    /// Session list response.
    SessionList { sessions: Vec<SessionInfo> },
    /// Attach confirmed.
    Attached { session: String, cols: u16, rows: u16 },
    /// Session exited.
    Exit { session: String, code: Option<i32> },
    /// Error.
    Error { message: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionInfo {
    pub cols: u16,
    pub created: u64,
    pub name: String,
    pub rows: u16,
}

// ========================================================================
// Frame encoding
// ========================================================================

pub fn encode<T: Serialize>(msg: &T) -> Vec<u8> {
    let json = serde_json::to_vec(msg).unwrap_or_default();
    let len = json.len() as u32;
    let mut out = len.to_be_bytes().to_vec();
    out.extend_from_slice(&json);
    out
}

pub fn decode<T: serde::de::DeserializeOwned>(buf: &[u8]) -> Option<T> {
    if buf.len() < 4 {
        return None;
    }
    let len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    if buf.len() < 4 + len {
        return None;
    }
    serde_json::from_slice(&buf[4..4 + len]).ok()
}

pub fn frame_len(buf: &[u8]) -> Option<usize> {
    if buf.len() < 4 {
        return None;
    }
    let len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    Some(4 + len)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_decode_client_attach() {
        let msg = ClientMessage::Attach {
            session: "default".into(),
        };
        let encoded = encode(&msg);
        let decoded: ClientMessage = decode(&encoded).unwrap();
        match decoded {
            ClientMessage::Attach { session } => assert_eq!(session, "default"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_encode_decode_server_output() {
        let msg = ServerMessage::Output {
            session: "s1".into(),
            bytes: vec![72, 101, 108, 108, 111],
        };
        let encoded = encode(&msg);
        let decoded: ServerMessage = decode(&encoded).unwrap();
        match decoded {
            ServerMessage::Output { bytes, .. } => assert_eq!(bytes, b"Hello"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_frame_len_incomplete() {
        assert!(frame_len(&[0, 0]).is_none());
    }

    #[test]
    fn test_decode_incomplete() {
        assert!(decode::<ClientMessage>(&[0, 0, 0, 5, 123]).is_none());
    }

    #[test]
    fn test_session_info_round_trip() {
        let info = SessionInfo {
            name: "work".into(),
            cols: 120,
            rows: 40,
            created: 1700000000,
        };
        let encoded = encode(&info);
        let decoded: SessionInfo = decode(&encoded).unwrap();
        assert_eq!(decoded.name, "work");
        assert_eq!(decoded.cols, 120);
    }

    #[test]
    fn test_encode_decode_client_resize() {
        let msg = ClientMessage::Resize {
            session: "default".into(),
            cols: 200,
            rows: 50,
        };
        let encoded = encode(&msg);
        let decoded: ClientMessage = decode(&encoded).unwrap();
        match decoded {
            ClientMessage::Resize { cols, rows, .. } => {
                assert_eq!(cols, 200);
                assert_eq!(rows, 50);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_encode_decode_server_exit() {
        let msg = ServerMessage::Exit {
            session: "s1".into(),
            code: Some(0),
        };
        let encoded = encode(&msg);
        let decoded: ServerMessage = decode(&encoded).unwrap();
        match decoded {
            ServerMessage::Exit { code, .. } => assert_eq!(code, Some(0)),
            _ => panic!("wrong variant"),
        }
    }
}
