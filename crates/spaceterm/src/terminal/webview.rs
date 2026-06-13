//! WebView tile manager: creates and positions child WebViews for rich blocks.

use std::collections::HashMap;

use winit::window::Window;
use wry::{WebView, WebViewBuilder};

use super::block_queue::BlockEntry;
use spaceterm_core::spaceterm_proto::{EmitBlock, TrustTier};

// ========================================================================
// Constants
// ========================================================================

const BLOCK_HEIGHT_ROWS: usize = 8;
const BLOCK_HTML_SHELL: &str = include_str!("block_shell.html");

const CSP_ISOLATED: &str = "default-src 'none'; style-src 'unsafe-inline'; img-src data:;";
const CSP_RESTRICTED: &str =
    "default-src 'none'; style-src 'unsafe-inline'; img-src data:; script-src 'none';";

const MIME_RICHNESS: &[&str] = &[
    "text/html",
    "image/svg+xml",
    "text/markdown",
    "text/csv",
    "image/png",
    "image/jpeg",
    "image/gif",
    "application/json",
    "text/plain",
];

// ========================================================================
// Data Structures
// ========================================================================

pub struct TileParams {
    pub grid_row: usize,
    pub height: u32,
    pub html: String,
    pub width: u32,
    pub x: i32,
    pub y: i32,
}

/// Manages child WebViews that render rich content blocks inline in the
/// terminal. Each content block gets its own WebView positioned at the
/// block's pixel coordinates within the parent window.
pub struct WebViewManager {
    tiles: HashMap<TileKey, TileSlot>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct TileKey {
    pane_id: crate::model::layout::PaneId,
    block_index: usize,
    segment_index: usize,
}

struct TileSlot {
    grid_row: usize,
    pane_id: crate::model::layout::PaneId,
    trust: TrustTier,
    webview: WebView,
}

// ========================================================================
// WebViewManager
// ========================================================================

impl WebViewManager {
    pub fn new() -> Self {
        Self {
            tiles: HashMap::new(),
        }
    }

    pub fn create_block_tile(
        &mut self,
        pane_id: crate::model::layout::PaneId,
        entry: &BlockEntry,
        params: TileParams,
        window: &Window,
    ) -> Result<(), wry::Error> {
        let key = TileKey {
            pane_id,
            block_index: entry.block_index,
            segment_index: entry.segment_index,
        };

        if self.tiles.contains_key(&key) {
            return Ok(());
        }

        let html = sandboxed_html(&params.html, entry.trust);

        let mut builder = WebViewBuilder::new()
            .with_html(&html)
            .with_bounds(wry::Rect {
                position: wry::dpi::LogicalPosition::new(params.x, params.y).into(),
                size: wry::dpi::LogicalSize::new(params.width, params.height).into(),
            })
            .with_visible(true)
            .with_navigation_handler(|_url| false);

        match entry.trust {
            TrustTier::Trusted => {}
            TrustTier::Restricted | TrustTier::Isolated => {
                builder = builder.with_javascript_disabled();
            }
        }

        let webview = builder.build_as_child(window)?;

        self.tiles.insert(
            key,
            TileSlot {
                grid_row: params.grid_row,
                pane_id,
                trust: entry.trust,
                webview,
            },
        );
        Ok(())
    }

    /// Reposition all tiles based on scroll offset. Tiles that scroll offscreen
    /// are hidden; tiles that come back are re-shown.
    pub fn reposition_tiles(
        &mut self,
        scroll_offset: usize,
        grid_rows: usize,
        cell_height: f32,
        pane_y_offset: f32,
    ) {
        for slot in self.tiles.values_mut() {
            let visible_row = slot.grid_row as isize - scroll_offset as isize;
            if visible_row < 0 || visible_row as usize >= grid_rows {
                let _ = slot.webview.set_visible(false);
            } else {
                let new_y = pane_y_offset + visible_row as f32 * cell_height;
                if let Ok(current_bounds) = slot.webview.bounds() {
                    let current_y = current_bounds.position.to_logical::<i32>(1.0).y;
                    if (current_y as f32 - new_y).abs() > 0.5 {
                        let _ = slot.webview.set_bounds(wry::Rect {
                            position: wry::dpi::LogicalPosition::new(
                                current_bounds.position.to_logical::<i32>(1.0).x,
                                new_y as i32,
                            )
                            .into(),
                            size: current_bounds.size,
                        });
                    }
                }
                let _ = slot.webview.set_visible(true);
            }
        }
    }

    /// The default block height in pixels given a cell height.
    pub fn block_pixel_height(cell_height: f32) -> u32 {
        (BLOCK_HEIGHT_ROWS as f32 * cell_height) as u32
    }

    /// Update the HTML content of an existing tile (for live-block patches).
    pub fn update_tile_html(
        &mut self,
        pane_id: crate::model::layout::PaneId,
        entry: &BlockEntry,
        html: &str,
    ) -> Result<(), wry::Error> {
        let key = TileKey {
            pane_id,
            block_index: entry.block_index,
            segment_index: entry.segment_index,
        };
        if let Some(slot) = self.tiles.get(&key) {
            let sandboxed = sandboxed_html(html, slot.trust);
            let js = format!(
                "document.documentElement.innerHTML = {};",
                serde_json::to_string(&sandboxed).unwrap_or_default()
            );
            let _ = slot.webview.evaluate_script(&js);
        }
        Ok(())
    }

    /// Remove all WebView tiles belonging to a closed pane.
    pub fn remove_tiles_for_pane(&mut self, pane_id: crate::model::layout::PaneId) {
        self.tiles.retain(|key, _| key.pane_id != pane_id);
    }

    /// Hide all WebView tiles for a folded block.
    pub fn fold_block(&mut self, pane_id: crate::model::layout::PaneId, block_index: usize) {
        for (key, slot) in self.tiles.iter_mut() {
            if key.pane_id == pane_id && key.block_index == block_index {
                let _ = slot.webview.set_visible(false);
            }
        }
    }

    /// Show all WebView tiles for an unfolded block.
    pub fn unfold_block(&mut self, pane_id: crate::model::layout::PaneId, block_index: usize) {
        for (key, slot) in self.tiles.iter_mut() {
            if key.pane_id == pane_id && key.block_index == block_index {
                let _ = slot.webview.set_visible(true);
            }
        }
    }

    /// Forward a key event to the focused block's WebView by dispatching a
    /// synthetic KeyboardEvent via JavaScript. Returns true if a tile existed
    /// for the focused pane.
    pub fn forward_key_event(
        &mut self,
        pane_id: crate::model::layout::PaneId,
        bytes: &[u8],
    ) -> bool {
        let key = String::from_utf8_lossy(bytes);
        let js = format!(
            "if(document.activeElement)document.activeElement.dispatchEvent(new KeyboardEvent('keydown',{{key:{},bubbles:true}}));",
            serde_json::to_string(&key).unwrap_or_default()
        );
        let mut dispatched = false;
        for slot in self.tiles.values_mut() {
            if slot.pane_id == pane_id {
                let _ = slot.webview.evaluate_script(&js);
                dispatched = true;
            }
        }
        dispatched
    }

    #[cfg(test)]
    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }
}

// ========================================================================
// Block HTML generation
// ========================================================================

fn sandboxed_html(content_html: &str, trust: TrustTier) -> String {
    let csp_meta = match trust {
        TrustTier::Isolated => Some(CSP_ISOLATED),
        TrustTier::Restricted => Some(CSP_RESTRICTED),
        TrustTier::Trusted => None,
    };
    match csp_meta {
        Some(policy) => {
            if content_html.contains("<head>") {
                content_html.replace(
                    "<head>",
                    &format!("<head><meta http-equiv=\"Content-Security-Policy\" content=\"{policy}\">"),
                )
            } else {
                format!(
                    "<html><head><meta http-equiv=\"Content-Security-Policy\" content=\"{policy}\"></head><body>{content_html}</body></html>"
                )
            }
        }
        None => content_html.to_string(),
    }
}

pub fn render_block_html(emit: &EmitBlock) -> String {
    let content = richest_content(emit);
    BLOCK_HTML_SHELL.replace("{{CONTENT}}", &content)
}

fn richest_content(emit: &EmitBlock) -> String {
    for mime in MIME_RICHNESS {
        if let Some(value) = emit.bundle.get(mime) {
            return render_mime(mime, value);
        }
    }
    escape_html(emit.bundle.text_plain().unwrap_or("[block]"))
}

fn render_mime(mime: &str, value: &serde_json::Value) -> String {
    match mime {
        "text/html" => {
            let html = value.as_str().unwrap_or("");
            format!("<div style=\"padding:8px;\">{html}</div>")
        }
        "image/svg+xml" => {
            let svg = value.as_str().unwrap_or("");
            format!("<div style=\"padding:8px;\">{svg}</div>")
        }
        "text/markdown" => {
            let md = value.as_str().unwrap_or("");
            let html = markdown_to_html(md);
            format!("<div style=\"padding:8px;\">{html}</div>")
        }
        "text/csv" => {
            let csv = value.as_str().unwrap_or("");
            let html = csv_to_table(csv);
            format!("<div style=\"padding:8px;\">{html}</div>")
        }
        "application/json" => {
            let formatted = serde_json::to_string_pretty(value).unwrap_or_default();
            format!(
                "<pre style=\"padding:8px;white-space:pre-wrap;font-size:13px;\">{}</pre>",
                escape_html(&formatted)
            )
        }
        "text/plain" => {
            let text = value.as_str().unwrap_or("");
            format!(
                "<pre style=\"padding:8px;white-space:pre-wrap;\">{}</pre>",
                escape_html(text)
            )
        }
        other if other.starts_with("image/") => {
            let data = value.as_str().unwrap_or("");
            format!("<div style=\"padding:8px;\"><img src=\"data:{mime};base64,{data}\" style=\"max-width:100%;\" /></div>")
        }
        _ => {
            let text = value.as_str().unwrap_or("?");
            format!("<pre style=\"padding:8px;\">{}</pre>", escape_html(text))
        }
    }
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn markdown_to_html(md: &str) -> String {
    let mut html = String::new();
    let mut in_list = false;
    for line in md.lines() {
        if let Some(rest) = line.strip_prefix("# ") {
            if in_list {
                html.push_str("</ul>");
                in_list = false;
            }
            html.push_str(&format!("<h2>{}</h2>", escape_html(rest)));
        } else if let Some(rest) = line.strip_prefix("## ") {
            if in_list {
                html.push_str("</ul>");
                in_list = false;
            }
            html.push_str(&format!("<h3>{}</h3>", escape_html(rest)));
        } else if line.starts_with("- ") || line.starts_with("* ") {
            if !in_list {
                html.push_str("<ul>");
                in_list = true;
            }
            html.push_str(&format!("<li>{}</li>", escape_html(&line[2..])));
        } else if line.starts_with("```") {
            if in_list {
                html.push_str("</ul>");
                in_list = false;
            }
            html.push_str("<pre><code>");
        } else if !line.is_empty() {
            if in_list {
                html.push_str("</ul>");
                in_list = false;
            }
            html.push_str(&format!("<p>{}</p>", escape_html(line)));
        }
    }
    if in_list {
        html.push_str("</ul>");
    }
    html
}

fn csv_to_table(csv: &str) -> String {
    let mut rows = Vec::new();
    for line in csv.lines() {
        let cells: Vec<String> = line
            .split(',')
            .map(|cell| escape_html(cell.trim()))
            .collect();
        if !cells.is_empty() {
            rows.push(cells);
        }
    }
    if rows.is_empty() {
        return String::new();
    }
    let mut html = String::from("<table style=\"border-collapse:collapse;\">");
    for (i, row) in rows.iter().enumerate() {
        let tag = if i == 0 { "th" } else { "td" };
        html.push_str("<tr>");
        for cell in row {
            html.push_str(&format!(
                "<{tag} style=\"border:1px solid #ccc;padding:4px 8px;\">{cell}</{tag}>"
            ));
        }
        html.push_str("</tr>");
    }
    html.push_str("</table>");
    html
}

impl Default for WebViewManager {
    fn default() -> Self {
        Self::new()
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use spaceterm_core::spaceterm_proto::{BlockId, EmitBlock, MimeBundle, TrustTier};
    use serde_json::Value;

    use super::*;

    fn svg_emit() -> EmitBlock {
        let mut bundle = MimeBundle::new();
        bundle.insert("image/svg+xml", Value::from("<svg width='10'/>"));
        bundle.insert("text/plain", Value::from("[svg]"));
        EmitBlock {
            bundle,
            id: BlockId(1),
            trust: TrustTier::Restricted,
        }
    }

    #[test]
    fn test_new_manager_has_no_tiles() {
        let mgr = WebViewManager::new();
        assert_eq!(mgr.tile_count(), 0);
    }

    #[test]
    fn test_block_pixel_height() {
        let h = WebViewManager::block_pixel_height(20.0);
        assert_eq!(h, 160);
    }

    #[test]
    fn test_tile_key_equality() {
        let pid = crate::model::layout::PaneId(0);
        let a = TileKey {
            pane_id: pid,
            block_index: 1,
            segment_index: 2,
        };
        let b = TileKey {
            pane_id: pid,
            block_index: 1,
            segment_index: 2,
        };
        assert_eq!(a, b);
    }

    #[test]
    fn test_render_block_html_svg() {
        let html = render_block_html(&svg_emit());
        assert!(html.contains("<svg width='10'/>"), "{html}");
        assert!(!html.contains("{{CONTENT}}"), "{html}");
    }

    #[test]
    fn test_render_block_html_fallback() {
        let mut bundle = MimeBundle::new();
        bundle.insert("text/plain", Value::from("hello <world>"));
        let emit = EmitBlock {
            bundle,
            id: BlockId(2),
            trust: TrustTier::Restricted,
        };
        let html = render_block_html(&emit);
        assert!(html.contains("hello &lt;world&gt;"), "{html}");
    }

    #[test]
    fn test_escape_html() {
        assert_eq!(escape_html("a<b>c&d\"e"), "a&lt;b&gt;c&amp;d&quot;e");
    }

    #[test]
    fn test_richest_content_picks_html_over_svg() {
        let mut bundle = MimeBundle::new();
        bundle.insert("text/html", Value::from("<b>bold</b>"));
        bundle.insert("image/svg+xml", Value::from("<svg/>"));
        let emit = EmitBlock {
            bundle,
            id: BlockId(3),
            trust: TrustTier::Trusted,
        };
        let content = richest_content(&emit);
        assert!(content.contains("<b>bold</b>"), "{content}");
    }

    #[test]
    fn test_sandboxed_html_adds_csp_for_restricted() {
        let html = "<html><head></head><body>hi</body></html>";
        let result = sandboxed_html(html, TrustTier::Restricted);
        assert!(result.contains("Content-Security-Policy"), "{result}");
        assert!(result.contains(CSP_RESTRICTED), "{result}");
    }

    #[test]
    fn test_sandboxed_html_adds_csp_for_isolated() {
        let html = "<html><head></head><body>hi</body></html>";
        let result = sandboxed_html(html, TrustTier::Isolated);
        assert!(result.contains("Content-Security-Policy"), "{result}");
        assert!(result.contains(CSP_ISOLATED), "{result}");
    }

    #[test]
    fn test_sandboxed_html_no_csp_for_trusted() {
        let html = "<html><head></head><body>hi</body></html>";
        let result = sandboxed_html(html, TrustTier::Trusted);
        assert!(!result.contains("Content-Security-Policy"), "{result}");
    }

    #[test]
    fn test_sandboxed_html_wraps_fragment_without_head() {
        let html = "<svg width='10'/>";
        let result = sandboxed_html(html, TrustTier::Restricted);
        assert!(result.contains("Content-Security-Policy"), "{result}");
        assert!(result.starts_with("<html>"), "{result}");
        assert!(result.contains("<svg width='10'/>"), "{result}");
    }

    #[test]
    fn test_render_markdown_produces_html() {
        let mut bundle = MimeBundle::new();
        bundle.insert("text/markdown", Value::from("# Hello\nworld"));
        let emit = EmitBlock {
            bundle,
            id: BlockId(10),
            trust: TrustTier::Trusted,
        };
        let html = render_block_html(&emit);
        assert!(html.contains("<h2>Hello</h2>"), "{html}");
        assert!(html.contains("<p>world</p>"), "{html}");
    }

    #[test]
    fn test_render_json_pretty_prints() {
        let mut bundle = MimeBundle::new();
        bundle.insert("application/json", serde_json::json!({"key": "value"}));
        let emit = EmitBlock {
            bundle,
            id: BlockId(11),
            trust: TrustTier::Restricted,
        };
        let html = render_block_html(&emit);
        assert!(html.contains("&quot;key&quot;"), "{html}");
    }

    #[test]
    fn test_render_csv_produces_table() {
        let mut bundle = MimeBundle::new();
        bundle.insert("text/csv", Value::from("name,score\nAlice,95"));
        let emit = EmitBlock {
            bundle,
            id: BlockId(12),
            trust: TrustTier::Restricted,
        };
        let html = render_block_html(&emit);
        assert!(html.contains("<th"), "{html}");
        assert!(html.contains("<td"), "{html}");
        assert!(html.contains("Alice"), "{html}");
    }

    #[test]
    fn test_markdown_to_html_list() {
        let html = markdown_to_html("- one\n- two\n");
        assert!(html.contains("<ul>"), "{html}");
        assert!(html.contains("<li>one</li>"), "{html}");
        assert!(html.contains("</ul>"), "{html}");
    }

    #[test]
    fn test_csv_to_table_single_row() {
        let html = csv_to_table("a,b");
        assert!(html.contains("<th"), "{html}");
        assert!(html.contains("</table>"), "{html}");
    }
}
