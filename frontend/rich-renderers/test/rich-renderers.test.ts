import { beforeAll, describe, expect, it } from "vitest";

import { renderMime } from "@spaceterm/block-renderer";

import { registerRichRenderers } from "../src/index";

beforeAll(() => {
  registerRichRenderers();
});

describe("text/latex via KaTeX", () => {
  it("renders math markup", () => {
    const el = renderMime("text/latex", "x^2", "trusted");
    expect(el.className).toBe("spaceterm-latex");
    expect(el.querySelector(".katex")).not.toBeNull();
  });
});

describe("text/markdown via marked", () => {
  it("sandboxes untrusted markdown in an iframe", () => {
    const el = renderMime("text/markdown", "# Title", "restricted");
    expect(el.tagName).toBe("IFRAME");
    expect(el.getAttribute("srcdoc")).toContain("<h1");
  });

  it("renders trusted markdown inline", () => {
    const el = renderMime("text/markdown", "**bold**", "trusted");
    expect(el.tagName).toBe("DIV");
    expect(el.innerHTML).toContain("<strong>bold</strong>");
  });
});
