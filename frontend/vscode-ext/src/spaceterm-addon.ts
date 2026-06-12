// ========================================================================
// Data Structures
// ========================================================================

/// The JS view of `spaceterm_core::Terminal`, exposed by the `spaceterm-bindings` napi
/// addon. The block JSON matches `@spaceterm/block-renderer`'s `CommandBlock[]`.
export interface SpaceTermTerminal {
  feed(bytes: Buffer): void;
  blocksJson(): string;
  plainText(): string;
}

export interface SpaceTermAddon {
  Terminal: new () => SpaceTermTerminal;
}

// ========================================================================
// Loading
// ========================================================================

/// Load the native addon from an absolute path. A plain `require` is used (the
/// addon is a prebuilt `.node`, resolved at runtime, not bundled).
export function loadAddon(addonPath: string): SpaceTermAddon {
  // eslint-disable-next-line @typescript-eslint/no-var-requires
  const nodeRequire = require as NodeRequire;
  return nodeRequire(addonPath) as SpaceTermAddon;
}
