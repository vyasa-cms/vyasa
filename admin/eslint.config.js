import js from "@eslint/js";
import tseslint from "typescript-eslint";

export default tseslint.config(
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    rules: {
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_" },
      ],
      "@typescript-eslint/consistent-type-imports": "error",
    },
  },
  {
    // `public/` ships as-is to the browser: plain scripts, not TS modules,
    // so the TypeScript-project rules do not apply to them.
    ignores: [
      "src/api/schema.d.ts",
      "src/routeTree.gen.ts",
      "public/",
      "dist/",
      "node_modules/",
    ],
  },
);
