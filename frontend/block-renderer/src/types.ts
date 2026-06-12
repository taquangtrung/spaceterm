// ========================================================================
// Data Structures
// ========================================================================
//
// These mirror the JSON shape produced by `spaceterm_core`'s `Scrollback::to_json`
// (the serde derives on `CommandBlock` / `Segment` / `EmitBlock`). Field names
// are snake_case to match the wire form exactly.

export type TrustTier = "isolated" | "restricted" | "trusted";

export interface BlockMeta {
  height_hint?: number | null;
  title?: string | null;
}

export interface MimeBundle {
  meta: BlockMeta;
  mime: Record<string, unknown>;
}

export interface EmitBlock {
  bundle: MimeBundle;
  id: number;
  trust: TrustTier;
}

export interface LinkSpan {
  text: string;
  url: string;
}

export type Segment =
  | { kind: "content"; data: EmitBlock }
  | { kind: "link"; data: LinkSpan }
  | { kind: "text"; data: string };

export interface CommandBlock {
  command: string;
  cwd: string | null;
  exit_code: number | null;
  output: Segment[];
}

/// Renders one MIME representation into a DOM element. Registered by MIME type
/// so hosts can plug in heavier renderers (KaTeX, Markdown, Vega) without the
/// core package depending on them.
export type MimeRenderer = (
  value: unknown,
  trust: TrustTier,
  doc: Document,
) => HTMLElement;
