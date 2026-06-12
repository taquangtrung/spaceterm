//! Resilience: automatic reconnection with exponential backoff.
//!
//! Wraps a [`MuxClient`](super::client::MuxClient) and reconnects
//! transparently when the connection drops.

use std::time::{Duration, Instant};

use crate::client::MuxClient;
use crate::protocol::ServerMessage;

// ========================================================================
// Data Structures
// ========================================================================

pub struct ResilientClient {
    backoff: Duration,
    connected: bool,
    last_attempt: Option<Instant>,
    max_backoff: Duration,
    path: String,
    retries: u32,
    session: String,
    inner: Option<MuxClient>,
}

// ========================================================================
// Constants
// ========================================================================

const INITIAL_BACKOFF: Duration = Duration::from_millis(100);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
const MAX_RETRIES: u32 = 50;

// ========================================================================
// Implementation
// ========================================================================

impl ResilientClient {
    pub fn new(path: &str, session: &str) -> Self {
        let mut inner = MuxClient::connect(path).ok();
        let connected = inner.is_some();
        if let Some(ref mut c) = inner {
            let _ = c.attach(session);
        }
        ResilientClient {
            backoff: INITIAL_BACKOFF,
            connected,
            inner,
            last_attempt: None,
            max_backoff: MAX_BACKOFF,
            path: path.to_string(),
            retries: 0,
            session: session.to_string(),
        }
    }

    pub fn send_input(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        if let Some(ref mut client) = self.inner {
            let result = client.send_input(&self.session, bytes);
            if result.is_err() {
                self.connected = false;
            }
            return result;
        }
        Ok(())
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> anyhow::Result<()> {
        if let Some(ref mut client) = self.inner {
            return client.resize(&self.session, cols, rows);
        }
        Ok(())
    }

    pub fn recv(&mut self) -> Option<ServerMessage> {
        if let Some(ref mut client) = self.inner {
            match client.recv() {
                Ok(Some(msg)) => {
                    self.retries = 0;
                    self.backoff = INITIAL_BACKOFF;
                    return Some(msg);
                }
                Ok(None) => return None,
                Err(_) => {
                    self.connected = false;
                }
            }
        }

        self.try_reconnect();
        None
    }

    pub fn is_connected(&self) -> bool {
        self.connected
    }

    fn try_reconnect(&mut self) {
        if self.retries >= MAX_RETRIES {
            return;
        }
        if let Some(last) = self.last_attempt {
            if last.elapsed() < self.backoff {
                return;
            }
        }
        self.last_attempt = Some(Instant::now());
        self.retries += 1;

        if let Ok(mut client) = MuxClient::connect(&self.path) {
            if client.attach(&self.session).is_ok() {
                self.inner = Some(client);
                self.connected = true;
                self.retries = 0;
                self.backoff = INITIAL_BACKOFF;
            }
        }

        self.backoff = (self.backoff * 2).min(self.max_backoff);
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resilient_client_starts_disconnected_for_bad_path() {
        let client = ResilientClient::new("/tmp/nonexistent.sock", "default");
        assert!(!client.is_connected());
    }

    #[test]
    fn test_resilient_client_send_succeeds_gracefully() {
        let mut client = ResilientClient::new("/tmp/nonexistent.sock", "default");
        assert!(client.send_input(b"hello").is_ok());
    }

    #[test]
    fn test_resilient_client_resize_succeeds_gracefully() {
        let mut client = ResilientClient::new("/tmp/nonexistent.sock", "default");
        assert!(client.resize(80, 24).is_ok());
    }

    #[test]
    fn test_resilient_client_recv_returns_none() {
        let mut client = ResilientClient::new("/tmp/nonexistent.sock", "default");
        assert!(client.recv().is_none());
    }
}
