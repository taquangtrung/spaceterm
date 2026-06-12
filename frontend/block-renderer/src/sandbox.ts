import type { TrustTier } from "./types";

// ========================================================================
// Constants
// ========================================================================

// iframe `sandbox` attribute tokens per untrusted tier. `restricted` keeps the
// frame same-origin (so CSP and styles apply) but grants no scripts; `isolated`
// grants nothing at all (unique origin, no scripts). `trusted` is never framed.
const SANDBOX_TOKENS: Record<Exclude<TrustTier, "trusted">, string> = {
  isolated: "",
  restricted: "allow-same-origin",
};

// ========================================================================
// Sandboxing
// ========================================================================

export function isTrusted(trust: TrustTier): trust is "trusted" {
  return trust === "trusted";
}

/// Host a fragment of HTML according to its trust tier: trusted content renders
/// inline, everything else goes into a sandboxed iframe so it cannot script the
/// main UI context.
export function htmlHost(
  html: string,
  trust: TrustTier,
  doc: Document,
): HTMLElement {
  if (isTrusted(trust)) {
    const host = doc.createElement("div");
    host.className = "spaceterm-html";
    host.innerHTML = html;
    return host;
  }

  const frame = doc.createElement("iframe");
  frame.className = "spaceterm-sandbox";
  frame.setAttribute("sandbox", SANDBOX_TOKENS[trust]);
  frame.setAttribute("srcdoc", html);
  return frame;
}
