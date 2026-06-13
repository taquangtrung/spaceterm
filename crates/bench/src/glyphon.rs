//! Baseline backend: feed the corpus through the existing `wgpu` + `glyphon`
//! renderer (`spaceterm-render`), one `render()` call per chunk.

use std::sync::Arc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{WindowAttributes, WindowId};

use spaceterm_render::renderer::{GpuRenderer, PaneRect, PaneView};
use spaceterm_render::{FontConfig, Screen, Theme};

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

struct GlyphonBench {
    config: BenchConfig,
    corpus: Vec<u8>,
    done: bool,
}

/// One measured pass over the corpus.
struct RunResult {
    frame_p99_ms: f64,
    throughput_mb_s: f64,
}

// ========================================================================
// Functions
// ========================================================================

pub fn run(config: BenchConfig) -> anyhow::Result<()> {
    let mut bench = GlyphonBench {
        config,
        corpus: crate::corpus::generate(config.lines),
        done: false,
    };
    EventLoop::new()?.run_app(&mut bench)?;
    Ok(())
}

/// Feed the whole corpus once, one `render()` per chunk, and measure it.
fn run_once(renderer: &mut GpuRenderer, corpus: &[u8]) -> RunResult {
    let (cols, rows) = renderer.grid_size();
    let mut screen = Screen::new(cols, rows);
    let rect = PaneRect {
        height: WINDOW_HEIGHT as f32,
        width: WINDOW_WIDTH as f32,
        x: 0.0,
        y: 0.0,
    };

    let mut frame_times = Vec::new();
    let start = Instant::now();
    for chunk in corpus.chunks(CHUNK_BYTES) {
        screen.feed(chunk);
        let view = PaneView {
            grid: screen.grid(),
            labels: None,
            nav_cursor: None,
            rect,
            selection: None,
        };
        let frame_start = Instant::now();
        renderer.render(std::slice::from_ref(&view), None, false, &[]);
        frame_times.push(frame_start.elapsed().as_secs_f64() * 1000.0);
    }
    let elapsed = start.elapsed().as_secs_f64();
    frame_times.sort_by(|a, b| a.partial_cmp(b).expect("frame times are finite"));

    RunResult {
        frame_p99_ms: percentile(&frame_times, 0.99),
        throughput_mb_s: (corpus.len() as f64 / (1024.0 * 1024.0)) / elapsed,
    }
}

/// Run warmup passes (discarded) then `runs` measured passes, aggregated.
/// Mirrors the app's init sequence (font scan, instance/adapter/device, theme).
fn run_bench(renderer: &mut GpuRenderer, corpus: &[u8], config: &BenchConfig) -> BenchReport {
    for _ in 0..config.warmup {
        run_once(renderer, corpus);
    }
    let results: Vec<RunResult> = (0..config.runs)
        .map(|_| run_once(renderer, corpus))
        .collect();
    let throughputs: Vec<f64> = results.iter().map(|r| r.throughput_mb_s).collect();
    let frame_p99s: Vec<f64> = results.iter().map(|r| r.frame_p99_ms).collect();
    BenchReport::from_runs(
        "glyphon",
        "wgpu+glyphon",
        corpus.len(),
        &throughputs,
        &frame_p99s,
    )
}

/// Nearest-rank percentile over an already-sorted slice.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx]
}

// ========================================================================
// ApplicationHandler
// ========================================================================

impl ApplicationHandler for GlyphonBench {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.done {
            return;
        }
        self.done = true;

        let font_load = spaceterm_render::start_font_load();
        let window = Arc::new(
            event_loop
                .create_window(
                    WindowAttributes::default()
                        .with_title("spaceterm-bench: glyphon")
                        .with_inner_size(LogicalSize::new(WINDOW_WIDTH, WINDOW_HEIGHT)),
                )
                .expect("create window"),
        );
        let size = window.inner_size();

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY),
            flags: wgpu::InstanceFlags::empty(),
            backend_options: wgpu::BackendOptions::default(),
            display: None,
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
        });
        let surface = instance
            .create_surface(Arc::clone(&window))
            .expect("create wgpu surface");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .expect("request wgpu adapter");

        let mut renderer = GpuRenderer::new(
            surface,
            adapter,
            size.width,
            size.height,
            FontConfig::default(),
            font_load,
        );
        renderer.set_theme(Theme::dark());

        let report = run_bench(&mut renderer, &self.corpus, &self.config);
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
