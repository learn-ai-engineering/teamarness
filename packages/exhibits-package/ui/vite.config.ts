import { defineConfig } from "vitest/config";

export default defineConfig({
  base: "./",
  test: {
    css: true,
    // The viewer's frame points at `teamarness-content:`, which only the Shell serves.
    environmentOptions: { happyDOM: { settings: { disableIframePageLoading: true } } },
  },
  build: {
    assetsInlineLimit: 0,
    lib: {
      entry: "src/index.ts",
      formats: ["es"],
      fileName: () => "index.js",
    },
    rollupOptions: { output: { assetFileNames: "style.css" } },
  },
});
