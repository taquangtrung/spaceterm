//! Session manager: owns PTY children, reads their output, and routes I/O.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::mpsc;
use std::thread;

use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

use crate::protocol::ServerMessage;

// ========================================================================
// Data Structures
// ========================================================================

struct Session {
    master: Box<dyn portable_pty::MasterPty + Send>,
    writer: Box<dyn Write + Send>,
}

pub struct SessionManager {
    next_id: u64,
    sessions: HashMap<String, Session>,
    output_tx: mpsc::Sender<ServerMessage>,
}

// ========================================================================
// Implementation
// ========================================================================

impl SessionManager {
    pub fn new(output_tx: mpsc::Sender<ServerMessage>) -> Self {
        SessionManager {
            next_id: 1,
            sessions: HashMap::new(),
            output_tx,
        }
    }

    pub fn create(&mut self, name: &str, cols: u16, rows: u16) -> anyhow::Result<()> {
        if self.sessions.contains_key(name) {
            return Ok(());
        }

        let pty_system = NativePtySystem::default();
        let pair = pty_system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let cmd = CommandBuilder::new_default_prog();
        let _child = pair.slave.spawn_command(cmd)?;

        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let tx = self.output_tx.clone();
        let session_name = name.to_string();

        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            let mut reader = reader;
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        let _ = tx.send(ServerMessage::Exit {
                            session: session_name.clone(),
                            code: Some(0),
                        });
                        break;
                    }
                    Ok(n) => {
                        let _ = tx.send(ServerMessage::Output {
                            session: session_name.clone(),
                            bytes: buf[..n].to_vec(),
                        });
                    }
                    Err(_) => {
                        let _ = tx.send(ServerMessage::Exit {
                            session: session_name.clone(),
                            code: None,
                        });
                        break;
                    }
                }
            }
        });

        self.sessions.insert(
            name.to_string(),
            Session {
                master: pair.master,
                writer,
            },
        );
        self.next_id += 1;
        Ok(())
    }

    pub fn write(&mut self, name: &str, bytes: &[u8]) -> anyhow::Result<()> {
        if let Some(session) = self.sessions.get_mut(name) {
            session.writer.write_all(bytes)?;
            session.writer.flush()?;
        }
        Ok(())
    }

    pub fn resize(&mut self, name: &str, cols: u16, rows: u16) -> anyhow::Result<()> {
        if let Some(session) = self.sessions.get(name) {
            session.master.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })?;
        }
        Ok(())
    }

    pub fn kill(&mut self, name: &str) {
        self.sessions.remove(name);
    }

    pub fn session_names(&self) -> Vec<String> {
        self.sessions.keys().cloned().collect()
    }

    pub fn has(&self, name: &str) -> bool {
        self.sessions.contains_key(name)
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_list_sessions() {
        let (tx, _rx) = mpsc::channel();
        let mut mgr = SessionManager::new(tx);
        mgr.create("test", 80, 24).unwrap();
        assert!(mgr.has("test"));
        assert_eq!(mgr.session_names(), vec!["test"]);
    }

    #[test]
    fn test_create_duplicate_is_ok() {
        let (tx, _rx) = mpsc::channel();
        let mut mgr = SessionManager::new(tx);
        mgr.create("dup", 80, 24).unwrap();
        mgr.create("dup", 80, 24).unwrap();
        assert_eq!(mgr.session_names().len(), 1);
    }

    #[test]
    fn test_kill_removes_session() {
        let (tx, _rx) = mpsc::channel();
        let mut mgr = SessionManager::new(tx);
        mgr.create("temp", 80, 24).unwrap();
        mgr.kill("temp");
        assert!(!mgr.has("temp"));
    }

    #[test]
    fn test_write_to_nonexistent_is_ok() {
        let (tx, _rx) = mpsc::channel();
        let mut mgr = SessionManager::new(tx);
        mgr.write("missing", b"hello").unwrap();
    }

    #[test]
    fn test_resize_nonexistent_is_ok() {
        let (tx, _rx) = mpsc::channel();
        let mut mgr = SessionManager::new(tx);
        mgr.resize("missing", 100, 50).unwrap();
    }
}
