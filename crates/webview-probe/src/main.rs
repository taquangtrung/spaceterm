//! `webview-probe` — does a `wry` webview paint in a GTK-backed (`tao`) window?
//!
//! The app uses `winit`, where `wry` webviews get no GDK frame clock and stay
//! blank on Linux/X11 (`_gdk_frame_clock_freeze` assertion). This probe opens a
//! `tao` window (GTK-backed on Linux) with one `wry` webview showing a colored
//! page. If it renders on your machine, moving the app's windowing from `winit`
//! to `tao` fixes native block/image rendering. Throwaway diagnostic.

use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::window::WindowBuilder;
use wry::WebViewBuilder;

const HTML: &str = "<!DOCTYPE html><html><body style=\"margin:0;background:#1e1e2e;\
color:#fff;font-family:sans-serif\"><div style=\"padding:24px\">\
<h1 style=\"color:#44cc88\">wry + tao renders</h1>\
<p>If you can read this, GTK-backed windowing fixes native rendering.</p>\
<svg width=\"140\" height=\"140\"><rect width=\"140\" height=\"140\" fill=\"#cc4488\"/>\
<circle cx=\"70\" cy=\"70\" r=\"45\" fill=\"#44cc88\"/></svg></div></body></html>";

fn main() {
    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title("spaceterm webview probe (tao + wry)")
        .build(&event_loop)
        .expect("build tao window");
    let _webview = WebViewBuilder::new()
        .with_html(HTML)
        .build(&window)
        .expect("build wry webview");

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        if let Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            *control_flow = ControlFlow::Exit;
        }
    });
}
