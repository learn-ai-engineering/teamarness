import { defineConfig } from "vitest/config";

export default defineConfig({
  base: "./",
  resolve: {
    // Every import of Loro, including the binding's own, resolves to the build that carries its
    // WebAssembly inline as base64, so the entrypoint fetches nothing and holds one instance.
    alias: [{ find: /^loro-crdt$/, replacement: "loro-crdt/base64" }],
  },
  test: {
    css: true,
    // Bundle the binding in tests too, so it shares the aliased Loro instance.
    server: { deps: { inline: [/loro-prosemirror/] } },
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
