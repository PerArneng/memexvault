import js from "@eslint/js";
import tseslint from "typescript-eslint";

const rawColour = "/#[0-9a-fA-F]{3,8}|rgba?.[0-9]|hsla?.[0-9]|oklch.[0-9]/";
const noRawColours = [
  { selector: `Literal[value=${rawColour}]`, message: "Use a design token from packages/ui/src/tokens.css." },
  { selector: `TemplateElement[value.raw=${rawColour}]`, message: "Use a design token from packages/ui/src/tokens.css." },
];

const pureBans = {
  "no-restricted-globals": ["error",
    { name: "setTimeout", message: "Time goes through the Clock interface." },
    { name: "setInterval", message: "Time goes through the Clock interface." },
    { name: "console", message: "Pure packages don't log." },
    { name: "fetch", message: "IO belongs in an edge package." }],
  "no-restricted-properties": ["error",
    { object: "Date", property: "now", message: "Use the Clock interface." },
    { object: "Math", property: "random", message: "Randomness is injected." }],
  "no-restricted-syntax": ["error",
    { selector: "NewExpression[callee.name='Date'][arguments.length=0]", message: "Use the Clock interface." },
    { selector: "JSXAttribute[name.name='dangerouslySetInnerHTML']", message: "Vault content is untrusted." }],
};

export default tseslint.config(
  { ignores: ["**/dist/**", "**/node_modules/**", "tools/**", "*.cjs", "**/test-results/**", "**/playwright-report/**"] },
  js.configs.recommended,
  ...tseslint.configs.strict,
  {
    files: ["**/*.{ts,tsx}"],
    rules: {
      "@typescript-eslint/no-explicit-any": "error",
      "no-restricted-syntax": ["error",
        { selector: "JSXAttribute[name.name='dangerouslySetInnerHTML']", message: "Vault content is untrusted." },
        ...noRawColours],
    },
  },
  { files: ["packages/types/**/*.ts", "packages/core/**/*.ts", "packages/api/**/*.ts"], rules: pureBans },
);
