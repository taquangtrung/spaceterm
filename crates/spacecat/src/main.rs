//! `spacecat` — display a file as a rich SpaceTerm block.
//!
//! Like `cat`, but when run inside SpaceTerm it emits the file as a typed,
//! MIME-tagged block (image, SVG, Markdown, HTML, ...) that the terminal
//! renders inline. Outside SpaceTerm it prints the `text/plain` fallback, so it
//! stays safe in pipes, tmux, ssh, and CI. Wire form mirrors
//! `docs/terminal-block-protocol-spec.md` (the same protocol as
//! `clients/client.sh`).

mod media;

use std::path::PathBuf;
use std::process::ExitCode;
use std::str::FromStr;

use anyhow::{bail, Context, Result};
use spaceterm_proto::{encode, encode_emit_file, BlockId, Message, TrustTier};

// ========================================================================
// Constants
// ========================================================================

const USAGE: &str = "usage: spacecat [--force] \
    [--trust isolated|restricted|trusted] <file>...";

/// The terminal's VT parser drops OSC escapes past this length, so an inline
/// payload above it never renders. Images route around it via the side-channel;
/// for inline (text) payloads we can only warn.
const SAFE_OSC_BYTES: usize = 1024;

const SIDECHANNEL_DIR_ENV: &str = "SPACETERM_SIDECHANNEL_DIR";

// ========================================================================
// Data Structures
// ========================================================================

struct Args {
    files: Vec<PathBuf>,
    force: bool,
    trust: TrustTier,
}

// ========================================================================
// Entry point
// ========================================================================

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("spacecat: {error}\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("spacecat: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn parse_args() -> Result<Args> {
    let mut files = Vec::new();
    let mut force = false;
    let mut trust = TrustTier::Restricted;

    let mut rest = std::env::args().skip(1);
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--force" => force = true,
            "--trust" => {
                let value = rest.next().context("--trust needs a value")?;
                trust = TrustTier::from_str(&value)
                    .map_err(|_| anyhow::anyhow!("unknown trust tier {value:?}"))?;
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            _ => files.push(PathBuf::from(arg)),
        }
    }

    if files.is_empty() {
        bail!("no files given");
    }
    Ok(Args {
        files,
        force,
        trust,
    })
}

/// Emit each file as a block when SpaceTerm is active (or `--force`), otherwise
/// print its plain-text fallback. Images go through the side-channel (a tiny
/// `file=` reference) so they survive the terminal's OSC length limit; text
/// payloads are inlined.
fn run(args: &Args) -> Result<()> {
    let emit = args.force || spaceterm_active();
    let sidechannel = std::env::var_os(SIDECHANNEL_DIR_ENV).map(PathBuf::from);
    let pid = std::process::id();

    for (index, path) in args.files.iter().enumerate() {
        let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
        let kind = media::detect(path, &bytes);
        let fallback = media::fallback(path, &kind, &bytes);

        if !emit {
            println!("{fallback}");
            continue;
        }

        let id = index as u64 + 1;
        let osc = match (kind.binary, &sidechannel) {
            (true, Some(dir)) => {
                let name = format!("spacecat-{pid}-{id}");
                std::fs::create_dir_all(dir).ok();
                std::fs::write(dir.join(&name), &bytes)
                    .with_context(|| format!("write side-channel file in {}", dir.display()))?;
                encode_emit_file(BlockId(id), args.trust, &name, kind.mime)
            }
            _ => {
                let block = media::inline_emit(&kind, &bytes, &fallback, id, args.trust);
                let osc = encode(&Message::Emit(block));
                if osc.len() >= SAFE_OSC_BYTES {
                    eprintln!(
                        "spacecat: {} is too large to inline ({} bytes) and may not render; \
                         only images use the side-channel",
                        path.display(),
                        osc.len()
                    );
                }
                osc
            }
        };
        println!("{osc}");
    }
    Ok(())
}

/// SpaceTerm is the active terminal if `$SPACETERM` is set or `$TERM_PROGRAM`
/// is `spaceterm` (matches `clients/client.sh`).
fn spaceterm_active() -> bool {
    std::env::var_os("SPACETERM").is_some()
        || std::env::var("TERM_PROGRAM").is_ok_and(|v| v == "spaceterm")
}
