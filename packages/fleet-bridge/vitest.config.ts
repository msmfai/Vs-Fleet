import * as path from "path";
import { defineConfig } from "vitest/config";

export default defineConfig({
  resolve: {
    alias: {
      vscode: path.resolve(__dirname, "test/vscode-mock.ts"),
    },
  },
  test: {
    environment: "node",
    coverage: {
      provider: "v8",
      include: ["src/**"],
      reporter: ["text", "lcov"],
      thresholds: {
        lines: 100,
        functions: 100,
        branches: 100,
        statements: 100,
      },
    },
  },
});
