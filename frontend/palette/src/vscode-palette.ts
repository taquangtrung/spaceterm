import type * as vscode from "vscode";

import type { Palette, PickItem, PickOptions, PromptOptions } from "./palette";

// ========================================================================
// Data Structures
// ========================================================================

// A QuickPickItem carrying the original value back out of the picker. VSCode
// preserves extra fields on the chosen item, so the value round-trips.
interface ValueItem<T> extends vscode.QuickPickItem {
  value: T;
}

// ========================================================================
// VSCodePalette
// ========================================================================

/// The VSCode backend: delegates to the host's QuickPick / InputBox. `window` is
/// injected (rather than imported) so the package never depends on the `vscode`
/// runtime module and stays unit-testable with a fake window.
export class VSCodePalette implements Palette {
  constructor(private readonly window: typeof vscode.window) {}

  async pick<T>(
    items: PickItem<T>[],
    options?: PickOptions,
  ): Promise<T | undefined> {
    const choices: ValueItem<T>[] = items.map((item) => ({
      label: item.label,
      description: item.description,
      detail: item.detail,
      value: item.value,
    }));
    const picked = await this.window.showQuickPick(choices, {
      placeHolder: options?.placeholder,
      matchOnDescription: options?.matchOnDescription ?? true,
    });
    return picked?.value;
  }

  prompt(options?: PromptOptions): Promise<string | undefined> {
    return Promise.resolve(
      this.window.showInputBox({
        placeHolder: options?.placeholder,
        value: options?.value,
        password: options?.password,
      }),
    ) as Promise<string | undefined>;
  }
}
