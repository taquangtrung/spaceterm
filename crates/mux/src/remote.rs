//! Remote domain support: connects to a remote mux server over SSH.
//!
//! Uses `ssh -W` (or a direct TCP forward) to tunnel the Unix socket
//! through an SSH connection to a remote SpaceTerm-mux server.

use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};

use crate::protocol::{self, ClientMessage, ServerMessage};

// ========================================================================
// Data Structures
// ========================================================================

pub struct RemoteClient {
    child: Child,
}

// ========================================================================
// Implementation
// ========================================================================

impl RemoteClient {
    /// Connect to a remote mux server via SSH.
    ///
    /// Spawns `ssh host SpaceTerm mux proxy` which proxies stdin/stdout to the
    /// remote Unix socket. This avoids needing to expose a TCP port.
    pub fn connect(host: &str, socket_path: Option<&str>) -> anyhow::Result<Self> {
        let socket = socket_path.unwrap_or("default");
        let child = Command::new("ssh")
            .arg(host)
            .arg("SpaceTerm")
            .arg("mux")
            .arg("proxy")
            .arg(socket)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;

        Ok(RemoteClient { child })
    }

    pub fn send(&mut self, msg: &ClientMessage) -> anyhow::Result<()> {
        let encoded = protocol::encode(msg);
        if let Some(stdin) = self.child.stdin.as_mut() {
            stdin.write_all(&encoded)?;
            stdin.flush()?;
        }
        Ok(())
    }

    pub fn recv(&mut self) -> anyhow::Result<Option<ServerMessage>> {
        let mut buf = [0u8; 8192];
        if let Some(stdout) = self.child.stdout.as_mut() {
            match stdout.read(&mut buf) {
                Ok(0) => Ok(None),
                Ok(n) => Ok(protocol::decode(&buf[..n])),
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
                Err(e) => Err(e.into()),
            }
        } else {
            Ok(None)
        }
    }
}

impl Drop for RemoteClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connect_returns_instance() {
        let result = RemoteClient::connect("nonexistent.host.invalid", None);
        assert!(result.is_ok());
        let mut client = result.unwrap();
        let _ = client.send(&crate::protocol::ClientMessage::ListSessions);
    }
}
