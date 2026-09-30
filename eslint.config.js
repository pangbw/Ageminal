import js from "@eslint/js";
import globals from "globals";
import pluginVue from "eslint-plugin-vue";
import tseslint from "typescript-eslint";

export default tseslint.config(
  {
    ignores: [
      "dist/**",
      "node_modules/**",
      // Rust 构建产物：tauri-build 会在这里生成 __global-api-script.js
      "target/**",
      "src-tauri/**",
      "prototype/**",
      "docs/**",
      ".scratch/**",
      // 生成物：由 `pnpm check:bindings` 保证与 Rust 源码一致，不 lint。
      "src/bindings.ts",
    ],
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  ...pluginVue.configs["flat/recommended"],
  {
    files: ["**/*.vue"],
    languageOptions: {
      parserOptions: { parser: tseslint.parser },
    },
  },
  {
    languageOptions: {
      globals: { ...globals.browser },
    },
  },
  {
    files: ["scripts/**/*.{js,mjs,ts}", "*.config.{js,mjs,ts}"],
    languageOptions: {
      globals: { ...globals.node },
    },
  },
);
