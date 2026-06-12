//! Mux server: listens on a Unix domain socket, manages PTY sessions,
//! and routes output to connected clients.
//!
//! Usage:
//!
//! ```text
//! SpaceTerm mux serve
//! SpaceTerm mux attach default
//! ```

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc;
use std::thread;

use crate::protocol::{
    self, ClientMessage, ServerMessage,
};
use crate::session::SessionManager;

// ========================================================================
// Data Structures
// ========================================================================

struct Client {
    attachments: Vec<String>,
    stream: UnixStream,
}

pub struct MuxServer {
    path: String,
}

// ========================================================================
// Implementation
// ========================================================================

impl MuxServer {
    pub fn new(path: &str) -> Self {
        MuxServer {
            path: path.to_string(),
        }
    }

    pub fn run(self) -> anyhow::Result<()> {
        let _ = std::fs::remove_file(&self.path);
        let listener = UnixListener::bind(&self.path)?;
        listener.set_nonblocking(true)?;

        let (output_tx, output_rx) = mpsc::channel::<ServerMessage>();
        let mut manager = SessionManager::new(output_tx);

        let mut clients: HashMap<u64, Client> = HashMap::new();
        let mut next_client_id: u64 = 1;

        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(true).ok();
                    clients.insert(next_client_id, Client {
                        attachments: Vec::new(),
                        stream,
                    });
                    next_client_id += 1;
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => {}
            }

            let mut to_remove = Vec::new();
            let mut pending_messages: Vec<(u64, ClientMessage)> = Vec::new();
            for (&cid, client) in &mut clients {
                let mut buf = [0u8; 4096];
                match client.stream.read(&mut buf) {
                    Ok(0) => {
                        to_remove.push(cid);
                        continue;
                    }
                    Ok(n) => {
                        if let Some(msg) = protocol::decode::<ClientMessage>(&buf[..n]) {
                            pending_messages.push((cid, msg));
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => {
                        to_remove.push(cid);
                    }
                }
            }

            for (cid, msg) in pending_messages {
                Self::handle_message(&mut manager, cid, &mut clients, msg);
            }

            while let Ok(msg) = output_rx.try_recv() {
                let sessions = match &msg {
                    ServerMessage::Output { session, .. } => vec![session.clone()],
                    ServerMessage::Exit { session, .. } => vec![session.clone()],
                    _ => Vec::new(),
                };
                let encoded = protocol::encode(&msg);
                for client in clients.values_mut() {
                    if sessions.is_empty() || client.attachments.iter().any(|s| sessions.contains(s)) {
                        let _ = client.stream.write_all(&encoded);
                    }
                }
                if let ServerMessage::Exit { session, .. } = &msg {
                    manager.kill(session);
                }
            }

            for cid in &to_remove {
                clients.remove(cid);
            }

            if clients.is_empty() && to_remove.is_empty() {
                thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }

    fn handle_message(
        manager: &mut SessionManager,
        cid: u64,
        clients: &mut HashMap<u64, Client>,
        msg: ClientMessage,
    ) {
        match msg {
            ClientMessage::Attach { session } => {
                if !manager.has(&session) {
                    if let Err(e) = manager.create(&session, 80, 24) {
                        let err = ServerMessage::Error {
                            message: e.to_string(),
                        };
                        if let Some(client) = clients.get_mut(&cid) {
                            let encoded = protocol::encode(&err);
                            let _ = client.stream.write_all(&encoded);
                        }
                        return;
                    }
                }
                if let Some(client) = clients.get_mut(&cid) {
                    if !client.attachments.contains(&session) {
                        client.attachments.push(session.clone());
                    }
                    let attached = ServerMessage::Attached {
                        session: session.clone(),
                        cols: 80,
                        rows: 24,
                    };
                    let encoded = protocol::encode(&attached);
                    let _ = client.stream.write_all(&encoded);
                }
            }
            ClientMessage::Detach => {
                if let Some(client) = clients.get_mut(&cid) {
                    client.attachments.clear();
                }
            }
            ClientMessage::Input { session, bytes } => {
                let _ = manager.write(&session, &bytes);
            }
            ClientMessage::Resize { session, cols, rows } => {
                let _ = manager.resize(&session, cols, rows);
            }
            ClientMessage::ListSessions => {
                let names = manager.session_names();
                let info = names
                    .into_iter()
                    .map(|name| protocol::SessionInfo {
                        name,
                        cols: 80,
                        rows: 24,
                        created: 0,
                    })
                    .collect();
                let msg = ServerMessage::SessionList { sessions: info };
                if let Some(client) = clients.get_mut(&cid) {
                    let encoded = protocol::encode(&msg);
                    let _ = client.stream.write_all(&encoded);
                }
            }
            ClientMessage::Kill { session } => {
                manager.kill(&session);
            }
        }
    }
}

impl Drop for MuxServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub fn default_socket_path() -> String {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
        .or_else(|_| std::env::var("HOME").map(|h| format!("{h}/.run")));
    match runtime_dir {
        Ok(dir) => format!("{dir}/spaceterm-mux.sock"),
        Err(_) => "/tmp/spaceterm-mux.sock".to_string(),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_socket_path_is_deterministic() {
        let p1 = default_socket_path();
        let p2 = default_socket_path();
        assert_eq!(p1, p2);
    }

    #[test]
    fn test_server_new_creates_instance() {
        let server = MuxServer::new("/tmp/test-spaceterm-mux.sock");
        assert_eq!(server.path, "/tmp/test-spaceterm-mux.sock");
    }
}
