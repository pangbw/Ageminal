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
  {
    // 生成物（specta 产出）：一致性由 `pnpm check:bindings` 保证，里面那处
    // `as any` 是生成的运行时辅助函数，改不了也不该改。放在最后 —— flat config
    // 里后面的对象覆盖前面的，否则 recommended 会把这条规则又打开。
    files: ["src/bindings.ts"],
    rules: {
      "@typescript-eslint/no-explicit-any": "off",
    },
  },
);
