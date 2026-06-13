//! Candidate backend: feed the corpus through `xterm.js` + `@xterm/addon-webgl`
//! in a `wry` webview. The page measures throughput and frame times itself and
//! reports back over the wry IPC channel; this module just hosts it.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use base64::Engine;
use serde::Deserialize;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowAttributes, WindowId};
use wry::{WebView, WebViewBuilder};

use crate::report::BenchReport;
use crate::BenchConfig;

// ========================================================================
// Constants
// ========================================================================

const CHUNK_BYTES: usize = 65_536;
const WINDOW_HEIGHT: u32 = 800;
const WINDOW_WIDTH: u32 = 1_200;

// ========================================================================
// Data Structures
// ========================================================================

/// The aggregate the harness posts back over `window.ipc.postMessage` after
/// running its warmup and measured passes.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WebReport {
    backend: String,
    frame_p99_ms_median: f64,
    runs: usize,
    throughput_mb_s_max: f64,
    throughput_mb_s_median: f64,
    throughput_mb_s_min: f64,
    total_bytes: usize,
}

struct WebglBench {
    config: BenchConfig,
    html: String,
    proxy: EventLoopProxy<WebReport>,
    webview: Option<WebView>,
    window: Option<Arc<Window>>,
}

// ========================================================================
// Functions
// ========================================================================

pub fn run(config: BenchConfig) -> anyhow::Result<()> {
    let event_loop = EventLoop::<WebReport>::with_user_event().build()?;
    let mut bench = WebglBench {
        config,
        html: build_html(config)?,
        proxy: event_loop.create_proxy(),
        webview: None,
        window: None,
    };
    event_loop.run_app(&mut bench)?;
    Ok(())
}

/// Assemble the page: inline the bundled harness JS, the xterm CSS, the base64
/// corpus, and the run settings into the `index.html` template.
fn build_html(config: BenchConfig) -> anyhow::Result<String> {
    let frontend = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("frontend");
    let dist = frontend.join("dist");
    let template =
        std::fs::read_to_string(frontend.join("index.html")).context("read frontend/index.html")?;
    let harness = std::fs::read_to_string(dist.join("harness.js")).context(
        "read frontend/dist/harness.js: run `pnpm install && pnpm build` in \
         crates/bench/frontend first",
    )?;
    let css = std::fs::read_to_string(dist.join("xterm.css")).context(
        "read frontend/dist/xterm.css: run `pnpm install && pnpm build` in \
         crates/bench/frontend first",
    )?;

    let corpus = crate::corpus::generate(config.lines);
    let corpus_b64 = base64::engine::general_purpose::STANDARD.encode(&corpus);
    // `{:?}` wraps the base64 (all ASCII, no escaping needed) in a JS string.
    let inject = format!(
        "window.__CORPUS_B64__={corpus_b64:?};\
         window.__CHUNK_BYTES__={CHUNK_BYTES};\
         window.__RUNS__={};window.__WARMUP__={};",
        config.runs, config.warmup
    );

    Ok(template
        .replace("/*XTERM_CSS*/", &css)
        .replace("/*CORPUS*/", &inject)
        .replace("/*HARNESS*/", &harness))
}

// ========================================================================
// ApplicationHandler
// ========================================================================

impl ApplicationHandler<WebReport> for WebglBench {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.webview.is_some() {
            return;
        }

        // wry's Linux backend is webkit2gtk, which needs gtk initialized; the
        // main app does the same in its window bootstrap.
        #[cfg(target_os = "linux")]
        {
            let _ = gtk::init();
        }

        let window = Arc::new(
            event_loop
                .create_window(
                    WindowAttributes::default()
                        .with_title("spaceterm-bench: xterm webgl")
                        .with_inner_size(LogicalSize::new(WINDOW_WIDTH, WINDOW_HEIGHT)),
                )
                .expect("create window"),
        );

        let proxy = self.proxy.clone();
        let webview = WebViewBuilder::new()
            .with_html(&self.html)
            .with_ipc_handler(move |req: wry::http::Request<String>| {
                if let Ok(report) = serde_json::from_str::<WebReport>(req.body()) {
                    let _ = proxy.send_event(report);
                }
            })
            .build(&window)
            .expect("build webview");

        self.window = Some(window);
        self.webview = Some(webview);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, report: WebReport) {
        let report = BenchReport {
            backend: report.backend,
            frame_p99_ms_median: report.frame_p99_ms_median,
            mode: "webgl".to_string(),
            runs: report.runs,
            throughput_mb_s_max: report.throughput_mb_s_max,
            throughput_mb_s_median: report.throughput_mb_s_median,
            throughput_mb_s_min: report.throughput_mb_s_min,
            total_bytes: report.total_bytes,
        };
        report.print();
        if self.config.json {
            report.print_json();
        }
        event_loop.exit();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if matches!(event, WindowEvent::CloseRequested) {
            event_loop.exit();
        }
    }
}
