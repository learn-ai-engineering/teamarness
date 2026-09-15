import { defineConfig } from "vite";

export default defineConfig({
  base: "./",
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
