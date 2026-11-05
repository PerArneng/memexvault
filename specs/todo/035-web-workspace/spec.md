---
id: 035
name: web-workspace
description: Create the TypeScript workspace with package boundaries and the IO rules enforced from the first commit
---

# 035 web-workspace

## Goal
A building workspace with the packages from `docs/frontend-architecture.md`, where the compiler and linters enforce the rules, not convention. A blank
console page loads in dev and in a production build.

## Background
**Depends on:** none. Design: `docs/frontend-architecture.md`, sections From Rust rules to TypeScript, Layout, Security.

## Approach
- A pnpm workspace under `web/` with hoisting off. Node pinned to an LTS version in `.nvmrc`, and the package manager pinned in `package.json`.
- All packages in the layout, mostly empty. Strict compiler flags everywhere, including `noUncheckedIndexedAccess` and
  `exactOptionalPropertyTypes`. Pure packages use `lib: ["ES2023"]` and `types: []`.
- ESLint rules from the mapping table, plus a ban on `dangerouslySetInnerHTML`. dependency-cruiser rules for the layers.
- `Clock` interface, `SystemClock` and `ManualClock`, and `settingsFrom(lookup)` with its tests.
- `apps/console/src/main.tsx` as the one composition root. Vitest in plain Node for pure packages.
- Add the frontend build, test and lint commands to `AGENTS.md`.

## Verification
1. `pnpm build`, `pnpm test`, `pnpm lint` and `pnpm typecheck` pass from a clean checkout.
2. A temporary `fetch` in `core` fails the type check, and a React import in `core` fails too. Revert both and record the output.
3. Temporary `Date.now`, `any` and `dangerouslySetInnerHTML` each fail lint. Revert them and record the output.
4. A temporary import from `edge-http` into `core`, and from `apps/console` views directly into `edge-http`, each fail dependency-cruiser.
5. A check rejects any package with a GPL or AGPL licence, and it fails against a test fixture with a denied licence.
6. Settings tests cover defaults, valid values and bad values.

## Out of scope
Design system (036), screens, API types (040), and CI hosting.

## Result
Not started.
