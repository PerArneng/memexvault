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
Built `web/`, a pnpm workspace with every package from the layout, on Node 24.22.2 pinned by `.nvmrc` and `use-node-version` in `.npmrc`. `pnpm install` fetches that Node, so the machine's Node 25 isn't used. Package manager pinned to pnpm 9.15.9. Hoisting is off.

**One decision changed.** TypeScript is 6.0.3, not the planned 7.0: `typescript-eslint` 8.72 requires below 6.1. `docs/frontend-architecture.md` records this.

**Verification.**
1. From a clean checkout (`node_modules` deleted, `pnpm install --frozen-lockfile`): `pnpm build`, `pnpm test` (9 tests), `pnpm lint` and `pnpm typecheck` all pass. The page is served in dev (Vite answers with the root element and transformed `main.tsx`), and in a production build it loads in Chromium with no errors, tested in spec 036.
2. `fetch` in `core`: `TS2304: Cannot find name 'fetch'`. `window` likewise. A React import: `TS2307: Cannot find module 'react'`.
3. `Date.now`, `new Date()`, `Math.random`, `setTimeout`, `any` and `dangerouslySetInnerHTML` each fail lint.
4. `edge-http` into `core` fails with `pure-no-edge-ui-app`, a view into `edge-http` fails with `only-main-builds-edges`, and React into `core` also fails depcruise with `pure-no-react`.
5. `tools/check-licenses.mjs` rejects the fixture with AGPL and GPL entries. The real tree passes.
6. Settings tests cover defaults, valid values, five bad buffer sizes and another origin.

`tools/check_rules.sh` reruns all 13 violations and reverts them, and `rule-checks.txt` holds the output. A first run wrongly reported the three depcruise checks as caught, because `depcruise` refuses to start on Node 25. The script now runs everything through `pnpm exec`, and the output above is from that run.

**Details for later specs.** Packages export their TypeScript source directly, so there's no per-package build. Test files have their own `tsconfig.test.json`, because Vitest's types could otherwise reintroduce Node globals such as `fetch` into a pure package. `ManualClock` is at `@memex/core/testing`. `AGENTS.md` lists the commands.
