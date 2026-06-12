import { describe, expect, it } from "vitest";

import {
  registerRenderer,
  renderBlock,
  renderBundle,
  renderMime,
  renderSegment,
} from "../src/index";
import type { CommandBlock, EmitBlock } from "../src/index";

describe("renderSegment", () => {
  it("renders text as a <pre> preserving exact content", () => {
    const el = renderSegment({ kind: "text", data: "a\nb" });
    expect(el.tagName).toBe("PRE");
    expect(el.textContent).toBe("a\nb");
  });
});

describe("renderMime trust tiers", () => {
  it("wraps untrusted HTML in a same-origin sandboxed iframe", () => {
    const el = renderMime("text/html", "<b>hi</b>", "restricted");
    expect(el.tagName).toBe("IFRAME");
    expect(el.getAttribute("sandbox")).toBe("allow-same-origin");
    expect(el.getAttribute("srcdoc")).toContain("<b>hi</b>");
  });

  it("gives isolated content a no-allowance sandbox", () => {
    const el = renderMime("text/html", "<b/>", "isolated");
    expect(el.tagName).toBe("IFRAME");
    expect(el.getAttribute("sandbox")).toBe("");
  });

  it("renders trusted HTML inline", () => {
    const el = renderMime("text/html", "<b>hi</b>", "trusted");
    expect(el.tagName).toBe("DIV");
    expect(el.innerHTML).toContain("<b>hi</b>");
  });

  it("renders a raster image as a base64 data URL", () => {
    const el = renderMime("image/png", "QUJD", "restricted");
    expect(el.tagName).toBe("IMG");
    expect(el.getAttribute("src")).toBe("data:image/png;base64,QUJD");
  });
});

describe("renderBundle richness selection", () => {
  it("prefers the richest representation over the fallback", () => {
    const block: EmitBlock = {
      bundle: {
        meta: {},
        mime: { "text/html": "<i>x</i>", "text/plain": "fallback" },
      },
      id: 1,
      trust: "trusted",
    };
    const el = renderBundle(block);
    expect(el.tagName).toBe("DIV");
    expect(el.innerHTML).toContain("<i>x</i>");
  });

  it("falls back to text/plain when nothing richer exists", () => {
    const block: EmitBlock = {
      bundle: { meta: {}, mime: { "text/plain": "only text" } },
      id: 2,
      trust: "restricted",
    };
    const el = renderBundle(block);
    expect(el.tagName).toBe("PRE");
    expect(el.textContent).toBe("only text");
  });
});

describe("renderSegment hyperlinks", () => {
  it("renders an http link as an anchor with href", () => {
    const el = renderSegment({
      kind: "link",
      data: { text: "site", url: "https://example.com" },
    });
    expect(el.tagName).toBe("A");
    expect(el.textContent).toBe("site");
    expect(el.getAttribute("href")).toBe("https://example.com");
  });

  it("keeps the text but drops a javascript: href", () => {
    const el = renderSegment({
      kind: "link",
      data: { text: "x", url: "javascript:alert(1)" },
    });
    expect(el.tagName).toBe("A");
    expect(el.textContent).toBe("x");
    expect(el.getAttribute("href")).toBeNull();
  });
});

describe("registry", () => {
  it("uses a host-registered renderer for a custom MIME type", () => {
    registerRenderer("text/latex", (value, _trust, doc) => {
      const span = doc.createElement("span");
      span.className = "katex";
      span.textContent = String(value);
      return span;
    });
    const el = renderMime("text/latex", "x^2", "trusted");
    expect(el.className).toBe("katex");
  });
});

describe("renderBlock", () => {
  it("renders the command, output, and a nonzero-exit marker", () => {
    const block: CommandBlock = {
      command: "ls",
      cwd: "/tmp",
      exit_code: 2,
      output: [{ kind: "text", data: "boom" }],
    };
    const el = renderBlock(block);
    expect(el.querySelector(".spaceterm-command")?.textContent).toBe("ls");
    expect(el.querySelector(".spaceterm-output")?.textContent).toBe("boom");
    expect(el.dataset.exitCode).toBe("2");
  });
});
