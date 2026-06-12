import * as vscode from "vscode";

import { VSCodePalette } from "@spaceterm/palette";

import { TerminalSession } from "./terminal-session";

// ========================================================================
// Activation
// ========================================================================

export function activate(context: vscode.ExtensionContext): void {
  const palette = new VSCodePalette(vscode.window);

  context.subscriptions.push(
    vscode.commands.registerCommand("spaceterm.open", () => {
      TerminalSession.create(context);
    }),
    vscode.commands.registerCommand("spaceterm.commandPalette", async () => {
      const action = await palette.pick(
        [
          {
            label: "Open Terminal",
            description: "Start a new SpaceTerm session",
            value: "open" as const,
          },
        ],
        { placeholder: "SpaceTerm command" },
      );
      if (action === "open") {
        TerminalSession.create(context);
      }
    }),
  );
}

export function deactivate(): void {}
