//! Mux client: connects to the mux server over a Unix socket.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

use crate::protocol::{self, ClientMessage, ServerMessage};

// ========================================================================
// Data Structures
// ========================================================================

pub struct MuxClient {
    stream: UnixStream,
}

// ========================================================================
// Implementation
// ========================================================================

impl MuxClient {
    pub fn connect(path: &str) -> anyhow::Result<Self> {
        let stream = UnixStream::connect(path)?;
        stream.set_nonblocking(true)?;
        Ok(MuxClient { stream })
    }

    pub fn attach(&mut self, session: &str) -> anyhow::Result<()> {
        self.send(&ClientMessage::Attach {
            session: session.to_string(),
        })
    }

    pub fn detach(&mut self) -> anyhow::Result<()> {
        self.send(&ClientMessage::Detach)
    }

    pub fn send_input(&mut self, session: &str, bytes: &[u8]) -> anyhow::Result<()> {
        self.send(&ClientMessage::Input {
            session: session.to_string(),
            bytes: bytes.to_vec(),
        })
    }

    pub fn resize(&mut self, session: &str, cols: u16, rows: u16) -> anyhow::Result<()> {
        self.send(&ClientMessage::Resize {
            session: session.to_string(),
            cols,
            rows,
        })
    }

    pub fn list_sessions(&mut self) -> anyhow::Result<()> {
        self.send(&ClientMessage::ListSessions)
    }

    pub fn kill(&mut self, session: &str) -> anyhow::Result<()> {
        self.send(&ClientMessage::Kill {
            session: session.to_string(),
        })
    }

    pub fn recv(&mut self) -> anyhow::Result<Option<ServerMessage>> {
        let mut buf = [0u8; 8192];
        match self.stream.read(&mut buf) {
            Ok(0) => Ok(None),
            Ok(n) => Ok(protocol::decode(&buf[..n])),
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn send(&mut self, msg: &ClientMessage) -> anyhow::Result<()> {
        let encoded = protocol::encode(msg);
        self.stream.write_all(&encoded)?;
        self.stream.flush()?;
        Ok(())
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connect_to_missing_socket_fails() {
        assert!(MuxClient::connect("/tmp/spaceterm-mux-nonexistent-test.sock").is_err());
    }
}
