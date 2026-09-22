import eslint from "@eslint/js";
import tseslint from "typescript-eslint";

export default tseslint.config(
  eslint.configs.recommended,
  ...tseslint.configs.recommended,
  { ignores: ["dist/**"] },
  {
    // The contract-types generator is a Node CLI: it uses Node globals and no DOM.
    files: ["bin/**/*.mjs"],
    languageOptions: {
      globals: { console: "readonly", process: "readonly" },
      sourceType: "module",
    },
  },
  {
    files: ["src/**/*.ts"],
    rules: {
      "no-restricted-imports": ["error", {
        patterns: [{ group: ["@tauri-apps/*"], message: "The author SDK carries no Shell or Tauri host code." }],
      }],
    },
  },
);
