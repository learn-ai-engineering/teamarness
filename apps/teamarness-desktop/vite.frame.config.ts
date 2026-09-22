import { defineConfig } from "vite";

/**
 * The Shell-owned Package UI frame runtime. It ships as one self-contained ES
 * module with no imports, embedded into the desktop binary and served under the
 * reserved `__teamarness/` path of the Package resource protocol.
 */
export default defineConfig({
  clearScreen: false,
  build: {
    outDir: "frame-dist",
    emptyOutDir: true,
    minify: false,
    lib: {
      entry: "src/package-frame/runtime-entry.ts",
      formats: ["es"],
      fileName: () => "package-frame-runtime.js",
    },
    rollupOptions: {
      external: [],
    },
  },
});
