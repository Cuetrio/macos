import { defineConfig, globalIgnores } from "eslint/config";
import nextVitals from "eslint-config-next/core-web-vitals";
import nextTs from "eslint-config-next/typescript";

const eslintConfig = defineConfig([
  ...nextVitals,
  ...nextTs,
  {
    files: ["e2e/**/*.js", "playwright.config.js"],
    rules: {
      "@typescript-eslint/no-require-imports": "off",
    },
  },
  {
    files: [
      "tools/desktop-poc/electron/**/*.cjs",
      "tools/desktop-poc/tauri/**/*.cjs",
      "src-tauri/**/*.cjs",
      "test/desktop/**/*.cjs",
      "test/png-native-acceptance-20261022/run-02/seed.cjs",
    ],
    rules: {
      "@typescript-eslint/no-require-imports": "off",
    },
  },
  {
    // JXA invokes run(argv) from the host, not from JavaScript source.
    files: [
      "test/png-native-acceptance-20261022/run-02/panel-diagnostic.js",
      "test/png-native-acceptance-20261022/run-02/panel-event-loop.js",
      "test/png-native-acceptance-20261022/run-02/panel-standard-diagnostic.js",
      "test/png-native-acceptance-20261022/run-02/panel-validation-diagnostic.js",
    ],
    rules: {
      "@typescript-eslint/no-unused-vars": ["warn", { varsIgnorePattern: "^run$" }],
    },
  },
  {
    files: ["test/png-native-acceptance-20261022/run-02/panel-event-loop.js"],
    rules: {
      // The native NSApplicationDelegate supplies this unused callback argument.
      "@typescript-eslint/no-unused-vars": [
        "warn",
        { varsIgnorePattern: "^run$", argsIgnorePattern: "^notification$" },
      ],
    },
  },
  {
    // JXA property access dispatches Objective-C methods (finishLaunching/run).
    files: [
      "test/png-native-acceptance-20261022/run-02/panel-diagnostic.js",
      "test/png-native-acceptance-20261022/run-02/panel-event-loop.js",
      "test/png-native-acceptance-20261022/run-02/panel-validation-diagnostic.js",
    ],
    rules: {
      "@typescript-eslint/no-unused-expressions": "off",
    },
  },
  // Override default ignores of eslint-config-next.
  globalIgnores([
    // Default ignores of eslint-config-next:
    ".next/**",
    "out/**",
    "build/**",
    "src/generated/**",
    "next-env.d.ts",
    "**/target/**",
    // Packaged release bundles and historical native acceptance build output:
    "release/**",
    "test/png-native-acceptance-20261022/run-02/cargo-target/**",
    "test/png-native-acceptance-20261022/run-02/next/**",
  ]),
]);

export default eslintConfig;
