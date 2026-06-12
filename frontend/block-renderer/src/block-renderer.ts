import { htmlHost } from "./sandbox";
import type {
  CommandBlock,
  EmitBlock,
  LinkSpan,
  MimeRenderer,
  Segment,
  TrustTier,
} from "./types";

// ========================================================================
// Constants
// ========================================================================

const TEXT_PLAIN = "text/plain";

// MIME types from richest to plainest; the first present in a bundle wins.
const RICHNESS_ORDER = [
  "text/html",
  "text/markdown",
  "text/latex",
  "image/svg+xml",
  "image/avif",
  "image/webp",
  "image/png",
  "image/jpeg",
  "image/gif",
  TEXT_PLAIN,
];

const RASTER_IMAGE_MIMES = [
  "image/avif",
  "image/gif",
  "image/jpeg",
  "image/png",
  "image/webp",
];

// URL schemes a hyperlink may navigate to; anything else (e.g. `javascript:`)
// keeps its text but drops the href.
const SAFE_URL_SCHEMES = ["file:", "http:", "https:", "mailto:"];

const RENDERERS = new Map<string, MimeRenderer>();

// ========================================================================
// Registry
// ========================================================================

/// Register (or override) the renderer for a MIME type. Hosts use this to add
/// KaTeX (`text/latex`), Markdown, Vega, etc. without the core package depending
/// on them.
export function registerRenderer(mime: string, render: MimeRenderer): void {
  RENDERERS.set(mime, render);
}

export function hasRenderer(mime: string): boolean {
  return RENDERERS.has(mime);
}

// ========================================================================
// Rendering
// ========================================================================

/// Render a whole command block: its command line, output segments, and a
/// `data-exit-code` marker when the command failed.
export function renderBlock(
  block: CommandBlock,
  doc: Document = document,
): HTMLElement {
  const root = doc.createElement("section");
  root.className = "spaceterm-block";

  if (block.command.length > 0) {
    const command = doc.createElement("div");
    command.className = "spaceterm-command";
    command.textContent = block.command;
    root.appendChild(command);
  }

  if (block.cwd) {
    const cwd = doc.createElement("div");
    cwd.className = "spaceterm-cwd";
    cwd.textContent = block.cwd;
    root.appendChild(cwd);
  }

  const output = doc.createElement("div");
  output.className = "spaceterm-output";
  for (const segment of block.output) {
    output.appendChild(renderSegment(segment, doc));
  }
  root.appendChild(output);

  if (block.exit_code !== null && block.exit_code !== 0) {
    root.dataset.exitCode = String(block.exit_code);
  }
  return root;
}

export function renderSegment(
  segment: Segment,
  doc: Document = document,
): HTMLElement {
  switch (segment.kind) {
    case "text":
      return renderText(segment.data, "restricted", doc);
    case "link":
      return renderLink(segment.data, doc);
    case "content":
      return renderBundle(segment.data, doc);
  }
}

/// Render the richest representation a content block offers.
export function renderBundle(
  block: EmitBlock,
  doc: Document = document,
): HTMLElement {
  const mime = pickRichest(block.bundle.mime);
  return renderMime(mime, block.bundle.mime[mime], block.trust, doc);
}

export function renderMime(
  mime: string,
  value: unknown,
  trust: TrustTier,
  doc: Document = document,
): HTMLElement {
  const render = RENDERERS.get(mime);
  return render ? render(value, trust, doc) : renderText(value, trust, doc);
}

// -----------------------------------------------------------
// Built-in renderers
// -----------------------------------------------------------

function renderText(
  value: unknown,
  _trust: TrustTier,
  doc: Document,
): HTMLElement {
  const pre = doc.createElement("pre");
  pre.className = "spaceterm-text";
  pre.textContent = asText(value);
  return pre;
}

function renderHtml(
  value: unknown,
  trust: TrustTier,
  doc: Document,
): HTMLElement {
  return htmlHost(asText(value), trust, doc);
}

// SVG can carry scripts, so it is sandboxed exactly like HTML unless trusted.
function renderSvg(
  value: unknown,
  trust: TrustTier,
  doc: Document,
): HTMLElement {
  return htmlHost(asText(value), trust, doc);
}

function rasterImageRenderer(mime: string): MimeRenderer {
  return (value, _trust, doc) => {
    const img = doc.createElement("img");
    img.className = "spaceterm-image";
    img.src = `data:${mime};base64,${asText(value)}`;
    return img;
  };
}

function renderLink(span: LinkSpan, doc: Document): HTMLElement {
  const anchor = doc.createElement("a");
  anchor.className = "spaceterm-link";
  anchor.textContent = span.text;
  if (isSafeUrl(span.url)) {
    anchor.setAttribute("href", span.url);
  }
  return anchor;
}

// -----------------------------------------------------------
// Helpers
// -----------------------------------------------------------

function pickRichest(mime: Record<string, unknown>): string {
  for (const candidate of RICHNESS_ORDER) {
    if (candidate in mime) {
      return candidate;
    }
  }
  return Object.keys(mime)[0] ?? TEXT_PLAIN;
}

function asText(value: unknown): string {
  if (typeof value === "string") {
    return value;
  }
  if (value === null || value === undefined) {
    return "";
  }
  return JSON.stringify(value);
}

function isSafeUrl(url: string): boolean {
  try {
    return SAFE_URL_SCHEMES.includes(new URL(url).protocol);
  } catch {
    return false;
  }
}

// ========================================================================
// Default registrations
// ========================================================================

registerRenderer(TEXT_PLAIN, renderText);
registerRenderer("text/html", renderHtml);
registerRenderer("image/svg+xml", renderSvg);
for (const mime of RASTER_IMAGE_MIMES) {
  registerRenderer(mime, rasterImageRenderer(mime));
}
