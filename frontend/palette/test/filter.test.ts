import { describe, expect, it } from "vitest";

import { filterItems, fuzzyScore } from "../src/index";
import type { PickItem } from "../src/index";

function items(...labels: string[]): PickItem<string>[] {
  return labels.map((label) => ({ label, value: label }));
}

describe("fuzzyScore", () => {
  it("rejects a non-subsequence", () => {
    expect(fuzzyScore("split pane", "xyz")).toBe(-1);
  });

  it("rewards adjacent matches over scattered ones", () => {
    expect(fuzzyScore("splitpane", "split")).toBeGreaterThan(
      fuzzyScore("s.p.l.i.t", "split"),
    );
  });
});

describe("filterItems", () => {
  it("returns every item unchanged for an empty query", () => {
    const all = items("Split Pane", "Close Tab", "New Window");
    expect(filterItems(all, "  ").map((i) => i.label)).toEqual([
      "Split Pane",
      "Close Tab",
      "New Window",
    ]);
  });

  it("keeps only fuzzy-matching items", () => {
    const all = items("Split Pane", "Close Tab", "New Window");
    const result = filterItems(all, "new").map((i) => i.label);
    expect(result).toEqual(["New Window"]);
  });

  it("can match against the description when asked", () => {
    const withDesc: PickItem<string>[] = [
      { label: "Action A", description: "switch pane", value: "a" },
      { label: "Action B", description: "close tab", value: "b" },
    ];
    const result = filterItems(withDesc, "pane", true).map((i) => i.value);
    expect(result).toEqual(["a"]);
  });
});
