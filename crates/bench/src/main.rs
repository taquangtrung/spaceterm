//! `spaceterm-bench` — Phase 0 gate for the unified-renderer plan.
//!
//! Feeds one deterministic VT byte corpus through two renderers and reports
//! throughput and frame-time percentiles, so the two numbers can be compared:
//!
//! - `glyphon`: the existing native `wgpu` + `glyphon` path (`spaceterm-render`).
//! - `webgl`: `xterm.js` + `@xterm/addon-webgl` hosted in a `wry` webview (the
//!   candidate the unified renderer would run in both frontends).
//!
//! Both backends open a real window, so this must run on a machine with a
//! display and GPU; it cannot run headless. See `README.md`.

mod corpus;
mod glyphon;
mod report;
mod webgl;

use std::process::ExitCode;

// ========================================================================
// Constants
// ========================================================================

const DEFAULT_LINES: usize = 20_000;
const DEFAULT_RUNS: usize = 5;
const DEFAULT_WARMUP: usize = 1;
const USAGE: &str = "usage: spaceterm-bench <glyphon|webgl> \
    [--lines N] [--runs N] [--warmup N] [--json]";

// ========================================================================
// Data Structures
// ========================================================================

/// One benchmark invocation's settings, shared by both backends.
#[derive(Clone, Copy)]
pub struct BenchConfig {
    pub json: bool,
    pub lines: usize,
    pub runs: usize,
    pub warmup: usize,
}

// ========================================================================
// Entry point
// ========================================================================

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(mode) = args.first() else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };

    let config = BenchConfig {
        json: args.iter().any(|a| a == "--json"),
        lines: parse_usize(&args, "--lines").unwrap_or(DEFAULT_LINES),
        runs: parse_usize(&args, "--runs").unwrap_or(DEFAULT_RUNS).max(1),
        warmup: parse_usize(&args, "--warmup").unwrap_or(DEFAULT_WARMUP),
    };

    let result = match mode.as_str() {
        "glyphon" => glyphon::run(config),
        "webgl" => webgl::run(config),
        other => {
            eprintln!("spaceterm-bench: unknown mode {other:?}\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("spaceterm-bench: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// Parse `--flag N` from the argument list, if present and valid.
fn parse_usize(args: &[String], flag: &str) -> Option<usize> {
    let pos = args.iter().position(|a| a == flag)?;
    args.get(pos + 1)?.parse().ok()
}
