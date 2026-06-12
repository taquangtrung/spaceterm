//! End-to-end check that the PTY runtime drives the parser over a real child.

use spaceterm_core::run_to_completion;
use portable_pty::CommandBuilder;

#[test]
fn test_run_echo_captures_child_output() {
    let mut command = CommandBuilder::new("echo");
    command.arg("hello-spaceterm");

    let terminal = run_to_completion(command).expect("echo runs under a PTY");
    assert!(
        terminal.scrollback().plain_text().contains("hello-spaceterm"),
        "captured: {:?}",
        terminal.scrollback().plain_text()
    );
}
