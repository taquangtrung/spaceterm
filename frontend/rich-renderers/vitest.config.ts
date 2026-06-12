import path from "node:path";
import { fileURLToPath } from "node:url";

import { defineConfig } from "vitest/config";

const dir = path.dirname(fileURLToPath(import.meta.url));

export default defineConfig({
  test: { environment: "jsdom" },
  resolve: {
    alias: {
      "@spaceterm/block-renderer": path.join(dir, "../block-renderer/src/index.ts"),
    },
  },
});
