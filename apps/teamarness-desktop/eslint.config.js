import eslint from "@eslint/js";
import tseslint from "typescript-eslint";

export default tseslint.config(
  { ignores: ["dist/**", "frame-dist/**", "src-tauri/target/**"] },
  eslint.configs.recommended,
  ...tseslint.configs.recommended,
  {
    files: ["src/**/*.ts", "vite.config.ts", "vite.frame.config.ts"],
    languageOptions: {
      globals: {
        document: "readonly",
        navigator: "readonly",
        window: "readonly",
        HTMLFormElement: "readonly",
        HTMLInputElement: "readonly",
        MessageChannel: "readonly",
        MessagePort: "readonly",
        MessageEvent: "readonly",
        DOMException: "readonly",
        CSSStyleSheet: "readonly",
      },
    },
  },
  {
    // The frame runtime runs inside a Package UI Frame, never in the Shell realm.
    files: ["src/package-frame/**/*.ts"],
    ignores: ["src/package-frame/**/*.test.ts"],
    rules: {
      "no-restricted-imports": ["error", {
        patterns: [
          { group: ["@tauri-apps/*"], message: "The Package UI frame runtime never reaches Tauri." },
          { group: ["../*", "../../*"], message: "The Package UI frame runtime imports only its own siblings and the author SDK." },
        ],
      }],
    },
  },
);
