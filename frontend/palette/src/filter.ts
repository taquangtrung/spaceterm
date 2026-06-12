import type { PickItem } from "./palette";

// ========================================================================
// Fuzzy filtering
// ========================================================================
//
// VSCode's QuickPick filters for us, but the native overlay backend  needs
// its own matcher. Keeping it here, host-agnostic and tested, means both
// backends rank candidates identically.

/// Filter and rank items by a fuzzy subsequence match against the query.
/// An empty query returns every item in its original order.
export function filterItems<T>(
  items: PickItem<T>[],
  query: string,
  matchOnDescription = true,
): PickItem<T>[] {
  const needle = query.trim().toLowerCase();
  if (needle.length === 0) {
    return [...items];
  }

  const ranked: Array<{ item: PickItem<T>; score: number }> = [];
  for (const item of items) {
    const haystack = matchOnDescription
      ? `${item.label} ${item.description ?? ""}`
      : item.label;
    const score = fuzzyScore(haystack.toLowerCase(), needle);
    if (score >= 0) {
      ranked.push({ item, score });
    }
  }

  ranked.sort((a, b) => b.score - a.score);
  return ranked.map((entry) => entry.item);
}

/// Score a subsequence match: -1 for no match, higher for tighter matches
/// (adjacent characters and earlier positions score better).
export function fuzzyScore(haystack: string, needle: string): number {
  let score = 0;
  let from = 0;
  let previousMatch = -1;
  for (const char of needle) {
    const index = haystack.indexOf(char, from);
    if (index < 0) {
      return -1;
    }
    score += index === previousMatch + 1 ? 2 : 1;
    previousMatch = index;
    from = index + 1;
  }
  return score;
}
