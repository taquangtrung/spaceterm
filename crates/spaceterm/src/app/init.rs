//! GPU/window/PTY bootstrap — the heavy init that runs once on `resumed`.

use std::sync::Arc;

use winit::event_loop::ActiveEventLoop;
use winit::window::WindowAttributes;

use crate::config::ThemeSetting;
use crate::terminal::pane::Pane;
use spaceterm_render::renderer::GpuRenderer;
use spaceterm_render::{FontConfig, Theme};

use super::{content_rows, App, APPROX_CELL_HEIGHT, APPROX_CELL_WIDTH, DEFAULT_COLS, DEFAULT_ROWS};

// ========================================================================
// App — window initialization
// ========================================================================

impl App {
    /// Full one-time setup: font scan (background), PTY spawn (background), GTK
    /// init, window creation, wgpu instance/adapter/device, renderer, theme, and
    /// the initial pane. Called from `ApplicationHandler::resumed`.
    pub(crate) fn init_window(&mut self, event_loop: &ActiveEventLoop) {
        // Kick off the system-font scan first so it runs concurrently with
        // window creation and the whole wgpu instance/adapter/device setup.
        let font_load = spaceterm_render::start_font_load();

        // The side-channel directory must exist before the shell is spawned so
        // the child inherits SPACETERM_SIDECHANNEL_DIR via the environment.
        let side_channel_dir =
            std::env::temp_dir().join(format!("spaceterm-sidechannel-{}", std::process::id()));
        std::fs::create_dir_all(&side_channel_dir).ok();
        std::env::set_var("SPACETERM_SIDECHANNEL_DIR", &side_channel_dir);

        // Spawn the PTY early so fork/exec overlaps with the entire GPU init
        // sequence (instance → adapter → device), which dominates startup time.
        // The pane is created with default dimensions and resized to the
        // renderer's precise cell grid after the GPU is ready.
        let pane_init_cols = DEFAULT_COLS as usize;
        let pane_init_rows = content_rows(DEFAULT_ROWS as usize);
        let pane_handle = std::thread::spawn(move || Pane::new(pane_init_cols, pane_init_rows));

        #[cfg(target_os = "linux")]
        {
            let _ = gtk::init();
        }

        let window = Arc::new(
            event_loop
                .create_window(
                    WindowAttributes::default()
                        .with_title("SpaceTerm")
                        .with_transparent(true)
                        .with_inner_size(winit::dpi::LogicalSize::new(
                            DEFAULT_COLS * APPROX_CELL_WIDTH,
                            DEFAULT_ROWS * APPROX_CELL_HEIGHT,
                        )),
                )
                .expect("create window"),
        );

        let size = window.inner_size();

        // GPU validation layers (enabled by `InstanceFlags::default()` in debug
        // builds) cost ~100ms at startup and slow every draw call. Off by
        // default; opt in with SPACETERM_GPU_DEBUG=1 when debugging the renderer.
        let gpu_flags = if std::env::var_os("SPACETERM_GPU_DEBUG").is_some() {
            wgpu::InstanceFlags::debugging()
        } else {
            wgpu::InstanceFlags::empty()
        };
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY),
            flags: gpu_flags,
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

        let font = FontConfig {
            family: std::env::var("SPACETERM_FONT")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .or_else(|| self.config.font_family.clone()),
            size: std::env::var("SPACETERM_FONT_SIZE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(self.config.font_size),
        };
        let mut renderer =
            GpuRenderer::new(surface, adapter, size.width, size.height, font, font_load);

        let mut initial_theme = match self.config.theme {
            ThemeSetting::Auto => window
                .theme()
                .map(|t| match t {
                    winit::window::Theme::Dark => Theme::dark(),
                    winit::window::Theme::Light => Theme::light(),
                })
                .unwrap_or_default(),
            ThemeSetting::Dark => Theme::dark(),
            ThemeSetting::Light => Theme::light(),
        };
        self.config.colors.apply(&mut initial_theme);
        renderer.set_theme(initial_theme);

        let (cols, rows) = renderer.grid_size();
        let want_rows = content_rows(rows);
        let mut pane = pane_handle.join().expect("PTY spawn thread panicked");
        if cols != pane_init_cols || want_rows != pane_init_rows {
            pane.resize(cols, want_rows);
        }
        let (cell_w, cell_h) = renderer.cell_size();
        pane.set_cell_size(cell_w, cell_h);
        let focused = self.tab.focused();
        self.panes.insert(focused, pane);

        self.window = Some(window);
        self.renderer = Some(renderer);
        self.dirty = true;
    }
}
