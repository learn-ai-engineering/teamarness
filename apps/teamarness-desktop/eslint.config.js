import eslint from "@eslint/js";
import tseslint from "typescript-eslint";

export default tseslint.config(
  { ignores: ["dist/**", "src-tauri/target/**"] },
  eslint.configs.recommended,
  ...tseslint.configs.recommended,
  {
    files: ["src/**/*.ts", "vite.config.ts"],
    languageOptions: {
      globals: {
        document: "readonly",
        navigator: "readonly",
        HTMLFormElement: "readonly",
        HTMLInputElement: "readonly",
      },
    },
  },
);
