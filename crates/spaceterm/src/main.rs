//! `SpaceTerm` — a web-native terminal.
//!
//! **Window mode** (no args): opens a GPU-accelerated terminal window (winit +
//! wgpu + cosmic-text) with an interactive shell.
//!
//! **Headless demo** (with args): runs the given command under a PTY and prints
//! the live cell grid and parsed block list.

use std::env;
use std::process::ExitCode;

use spaceterm_core::run_to_completion;
use spaceterm_render::Screen;
use portable_pty::CommandBuilder;

// ============================================================================
// Entry point
// ============================================================================

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();

    if args.is_empty() {
        run_window()
    } else if args[0] == "mux" {
        run_mux(&args[1..])
    } else {
        run_headless(&args)
    }
}

// ============================================================================
// Window mode
// ============================================================================

fn run_window() -> ExitCode {
    use winit::event_loop::EventLoop;

    let event_loop = match EventLoop::new() {
        Ok(el) => el,
        Err(error) => {
            eprintln!("spaceterm: {error}");
            return ExitCode::FAILURE;
        }
    };

    let mut app = spaceterm_app::app::App::new();
    match event_loop.run_app(&mut app) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("spaceterm: {error}");
            ExitCode::FAILURE
        }
    }
}

// ============================================================================
// Headless demo
// ============================================================================

fn run_headless(args: &[String]) -> ExitCode {
    let mut command = CommandBuilder::new(&args[0]);
    for arg in &args[1..] {
        command.arg(arg);
    }

    let terminal = match run_to_completion(command) {
        Ok(t) => t,
        Err(error) => {
            eprintln!("spaceterm: {error}");
            return ExitCode::FAILURE;
        }
    };

    println!("=== Blocks ===");
    for (index, block) in terminal.scrollback().blocks().iter().enumerate() {
        println!("--- block {index} ---");
        println!("{block:#?}");
    }

    println!("\n=== Screen Grid ===");
    let mut screen = Screen::new(80, 24);
    screen.feed(terminal.scrollback().plain_text().as_bytes());
    print_grid(screen.grid());

    ExitCode::SUCCESS
}

fn print_grid(grid: &spaceterm_render::Grid) {
    for row in 0..grid.rows() {
        let mut line = String::with_capacity(grid.cols());
        for col in 0..grid.cols() {
            let ch = grid.cell(row, col).map(|c| c.ch).unwrap_or(' ');
            line.push(ch);
        }
        println!("{}", line.trim_end());
    }
}

// ============================================================================
// Mux subcommands
// ============================================================================

fn run_mux(args: &[String]) -> ExitCode {
    if args.is_empty() {
        eprintln!("usage: spaceterm mux <serve|attach|list|kill> [args]");
        return ExitCode::FAILURE;
    }

    match args[0].as_str() {
        "serve" => {
            let path = spaceterm_mux::server::default_socket_path();
            eprintln!("spaceterm mux: listening on {path}");
            let server = spaceterm_mux::server::MuxServer::new(&path);
            match server.run() {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("spaceterm mux: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        "list" => {
            let path = spaceterm_mux::server::default_socket_path();
            let mut client = match spaceterm_mux::client::MuxClient::connect(&path) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("spaceterm mux: cannot connect to server: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let _ = client.list_sessions();
            std::thread::sleep(std::time::Duration::from_millis(100));
            while let Some(msg) = client.recv().unwrap_or(None) {
                if let spaceterm_mux::protocol::ServerMessage::SessionList { sessions } = msg {
                    for s in &sessions {
                        println!("{} ({}x{})", s.name, s.cols, s.rows);
                    }
                    if sessions.is_empty() {
                        println!("(no sessions)");
                    }
                    return ExitCode::SUCCESS;
                }
            }
            eprintln!("spaceterm mux: no response from server");
            ExitCode::FAILURE
        }
        "kill" => {
            let session = args.get(1).map(|s| s.as_str()).unwrap_or("default");
            let path = spaceterm_mux::server::default_socket_path();
            let mut client = match spaceterm_mux::client::MuxClient::connect(&path) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("spaceterm mux: cannot connect: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let _ = client.kill(session);
            println!("killed session: {session}");
            ExitCode::SUCCESS
        }
        "attach" => {
            let session = args.get(1).map(|s| s.as_str()).unwrap_or("default");
            run_mux_attach(session)
        }
        "proxy" => {
            eprintln!("spaceterm mux proxy: not yet implemented");
            ExitCode::FAILURE
        }
        _ => {
            eprintln!("usage: spaceterm mux <serve|attach|list|kill> [args]");
            ExitCode::FAILURE
        }
    }
}

fn run_mux_attach(session: &str) -> ExitCode {
    let path = spaceterm_mux::server::default_socket_path();
    let mut client = match spaceterm_mux::client::MuxClient::connect(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("spaceterm mux: cannot connect: {e}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(e) = client.attach(session) {
        eprintln!("spaceterm mux: attach failed: {e}");
        return ExitCode::FAILURE;
    }

    eprintln!("attached to session: {session}");

    use std::io::{self, BufRead, Write};
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        match line {
            Ok(line) => {
                let mut bytes = line.into_bytes();
                bytes.push(b'\n');
                let _ = client.send_input(session, &bytes);
                std::thread::sleep(std::time::Duration::from_millis(50));
                while let Some(msg) = client.recv().unwrap_or(None) {
                    match msg {
                        spaceterm_mux::protocol::ServerMessage::Output { bytes, .. } => {
                            let _ = stdout.write_all(&bytes);
                            let _ = stdout.flush();
                        }
                        spaceterm_mux::protocol::ServerMessage::Exit { code, .. } => {
                            eprintln!("session exited (code: {:?})", code);
                            return ExitCode::SUCCESS;
                        }
                        _ => {}
                    }
                }
            }
            Err(_) => break,
        }
    }

    let _ = client.detach();
    ExitCode::SUCCESS
}
