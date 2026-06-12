// ========================================================================
// Data Structures
// ========================================================================
//
// The palette abstraction is deliberately capped at QuickPick's feature level
// (label/description/detail + a free-text prompt) so the native overlay backend
// never grows affordances VSCode cannot match and the two targets stay aligned.
// Anything richer than this is a WebviewPanel, not the palette.

export interface PickItem<T> {
  label: string;
  description?: string;
  detail?: string;
  value: T;
}

export interface PickOptions {
  placeholder?: string;
  matchOnDescription?: boolean;
}

export interface PromptOptions {
  placeholder?: string;
  value?: string;
  password?: boolean;
}

/// One focused, modal input over a fuzzy-filtered list (or a free-text prompt).
/// Implemented by `VSCodePalette` (host QuickPick) and, later, a native overlay.
export interface Palette {
  pick<T>(items: PickItem<T>[], options?: PickOptions): Promise<T | undefined>;
  prompt(options?: PromptOptions): Promise<string | undefined>;
}
