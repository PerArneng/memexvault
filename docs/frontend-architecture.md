# memexvault frontend architecture

The shared design for the frontend specs (035, 036, 040-048, 050 and 051). It adapts the `rust-architecture` skill to React and
TypeScript, with the same goal: every piece of logic can be tested in memory, with no network, DOM, clock or storage. Update
this file when a spec changes a decision. The backend design and the API contract are in
[architecture.md](architecture.md).

## Decisions

| Topic | Decision |
|---|---|
| Stack | React and TypeScript, built with Vite, in a pnpm workspace under `web/`. Node 24 LTS, pinned in `.nvmrc` and `.npmrc`. |
| Delivery | The built console is embedded in the `memexvault` binary and served same-origin. Node is needed only at build time. |
| Login | An existing token is pasted once and exchanged for an HttpOnly, SameSite=Strict session cookie. |
| Look | Dark and professional. Dark only for now, and colours are design tokens so a light theme can come later. |
| Documents | Server-rendered page images with highlights drawn from anchor rects. Pages and rects come from one renderer, so they line up on OCR pages too. 100% zoom is one CSS pixel per point, and only visible pages are fetched. See spec 042. |
| Page editor | Markdown with live preview, and autocomplete for `[[links]]` and claim references |
| Chat | Our own UI on `@ag-ui/client`, with citations and proposal cards as first-class parts |
| Chat approvals | The normal review API. A proposal card calls it directly, and a run never pauses for approval. |
| Screens | Desktop-first and usable down to 1024 px wide. The UI text is English only. |

## From Rust rules to TypeScript

| `rust-architecture` rule | TypeScript form |
|---|---|
| Crates with one-way dependencies | pnpm workspace with strict, non-hoisted packages. A package can't import what it doesn't list. |
| Trait in front of every edge | An `interface` with a plain noun name in `core`: `Api`, `ActivityFeed`, `AgentChannel`, `Clock` |
| Prefixed implementations and fakes | `FetchApi`, `SseActivityFeed`, `AgUiAgentChannel`, and in tests `FakeApi`, `ScriptedAgentChannel`, `ManualClock` |
| Pure crates can't name IO | `types`, `core` and `api` compile with `lib: ["ES2023"]` and `types: []`, so `fetch`, `window` and `localStorage` are compile errors. They have no React dependency. |
| `clippy.toml` disallowed lists | ESLint bans `Date.now`, `new Date()`, `Math.random`, `setTimeout`, `console` and `any` in pure packages. dependency-cruiser enforces the layers. |
| Constructor injection, no globals | Factory functions take an `Edges` object. No module-level mutable state or singletons. Constant tables are fine. |
| One composition root | `apps/console/src/main.tsx` builds the edges. React context carries the engine and nothing else. |
| `Engine` facade | One `Engine` with a method per use case. Views call it and never call the API directly. |
| Thin frontends | Views parse input, call the engine and render. Rules, reducers and maths live in `core`. |
| Immutable plain data | `readonly` types and discriminated unions, with no classes in data |
| Settings from a lookup | `settingsFrom(lookup)` is pure, and only `main.tsx` reads the runtime config. |
| Fakes over mocks, happy and bad cases | Vitest in plain Node for pure packages, with no DOM and no sleeping. Time comes from `ManualClock`. |
| One concept per module | The file name says what's in it. A view file doesn't also hold rules or API calls. |

Don't push interfaces into code with no edge behind it. A table cell or a pure formatting function needs none.

## Layout

```
web/
  packages/types/        pure: ids and domain data
  packages/core/         pure: interfaces, Engine, reducers (activity, chat transcript, upload queue), overlay maths (overlay.ts, page-stack.ts)
  packages/api/          pure: generated OpenAPI types and wire-to-domain translation
  packages/edge-http/    edge: FetchApi
  packages/edge-sse/     edge: SseActivityFeed
  packages/edge-agui/    edge: the only package that depends on @ag-ui/client and rxjs
  packages/edge-browser/ edge: SystemClock, storage, upload with progress
  packages/ui/           React design system, presentational only, with no data access
  apps/console/          main.tsx, routes, thin views, TanStack Query. `src/viewer/` holds the page viewer and player, which know nothing of any one screen.
  ui-coverage.json       every OpenAPI operation mapped to a screen, or marked no-UI with a reason
```

```
main.tsx -> views -> Engine (core) -> interfaces + types
                     edges ----------> interfaces + types
views -> ui
```

`core` never imports an edge, `ui` or React. TanStack Query lives only in `apps/console`, where it caches what the engine returns.

## Live data

A live screen fetches a snapshot, then follows the activity stream from the snapshot's sequence number. A pure reducer in `core` merges
events. On a resync frame, the screen fetches a new snapshot. The review queue follows the same stream through `QueueFeed` in `edge-sse`, and refetches its counts when a durable frame touches changesets (spec 046). Rendering is batched, so a burst of events can't block input, and long lists are
virtualized.

## Security

All vault content is untrusted, including document text, OCR output, entity fields, page bodies and model replies.

- No `dangerouslySetInnerHTML`. An ESLint rule enforces this.
- Markdown goes through one sanitizing component, `SafeMarkdown` in `@memex/ui/markdown`, with raw HTML shown as text.
- No remote images or media in rendered content. This also stops data leaking out through image URLs.
- CodeMirror runs in a shadow root, as its runtime styles are then a constructable stylesheet, which the CSP allows. In the document it adds a `<style>`, which the CSP blocks. See spec 044.
- The server sends a strict CSP with `default-src 'self'`, so the build inlines nothing (`assetsInlineLimit: 0`). Radix's modal scroll lock adds a `<style>` element, which the CSP blocks, and `Select` adds a second one, so prefer `Menu` until that's decided. See specs 037 and 040.
- Page images, thumbnails and media are same-origin addresses in `src`, so the cookie and the policy apply, and nothing needs `blob:` or `data:`. Positions go through `element.style`, which the policy allows. See spec 042.
- The console signs in through `POST /api/v1/session`, and sends `x-memex-csrf` on every state-changing request. Page script can't read the session cookie.
- External links are visibly marked and open with `noopener`.
- Trust tier and epistemic status are shown wherever claims appear.

## Quality

- Every screen has loading, empty, error and offline states.
- Everything works by keyboard, with visible focus. Contrast meets WCAG AA.
- URLs hold screen state, so any view can be linked to and reloaded.
- An action on a page asks for confirmation in the page rather than in a modal while the modal's CSP error is undecided. See spec 040.
- Screens can be built against `FakeApi`, but a spec isn't done until it passes against the real API.
- `QueryView` is the one way a screen shows loading, empty, error and offline. `accessTo` in `core` decides what a URL may show, so a direct link gets the same answer as the navigation.
- The API types are generated from the committed `packages/api/openapi.json`. `pnpm api:check` fails on drift from the server's document, and `pnpm typecheck` on drift from the committed one.
- The sign-in route's initial JavaScript stays at or under 200 kB gzip. `pnpm budget` checks it, and anything only a signed-in user needs goes in the lazy `shell/` chunk.

## Packages

Versions are from npm and crates.io on 2026-11-05 and are pinned by the lockfile. A spec may add a package with its licence recorded here. AGPL and
GPL are denied, as in spec 008.

| Package | Version | Licence | Use |
|---|---|---|---|
| `react` | 19.3 | MIT | UI |
| `typescript`, `vite` | 6.0, 8.3 | Apache-2.0, MIT | Build. TypeScript stays on 6.0 because `typescript-eslint` 8.72 requires below 6.1, so 7.0 isn't possible yet. |
| `@tanstack/react-router`, `react-query`, `react-table`, `react-virtual` | 1.170, 5.106, 9.3, 3.14 | MIT | Routes, server state, tables, long lists |
| `tailwindcss`, `@radix-ui/*`, `lucide-react` | 4.3, 1.x, 1.69 | MIT, MIT, ISC | Styling, accessible primitives, icons |
| `@ag-ui/client` | 1.0.4 | MIT | Chat protocol, in `edge-agui` only |
| `@ag-ui/core` | 1.0.4 | MIT | Dev only for now: spec 049's check of the backend's streams against its schemas, in `edge-agui`, with its peer `zod` 3.25.76 (MIT) |
| `@codemirror/*`, `@lezer/highlight` | 6.x, 1.2 | MIT | Page editor |
| `react-markdown`, `rehype-sanitize`, `remark-gfm` | 10.1, 6.0, 4.0 | MIT | Rendering Markdown |
| `openapi-typescript`, `openapi-fetch` | 7.13, 0.17 | MIT | API types and client |
| `vitest`, `@playwright/test`, `msw` | 5.1, 1.64, 3.0 | MIT, Apache-2.0, MIT | Tests |
| `@fontsource-variable/inter`, `@fontsource-variable/jetbrains-mono` | 5.3 | OFL-1.1 | Bundled interface fonts |
| `@axe-core/playwright` | 4.14 | MPL-2.0 | Accessibility tests only |
| `eslint`, `typescript-eslint`, `dependency-cruiser` | 10.14, 8.72, 18.7 | MIT | Lint and layer checks |
| `rust-embed` (Rust) | 8.13 | MIT | Embeds the console in the binary |

## Left out

| Left out | Why |
|---|---|
| Light theme | Not requested. Design tokens leave room for it. |
| Visual page editor | Markdown matches how pages are stored, keeps diffs readable, and needs no lossless conversion. |
| CopilotKit | Its UI model would sit in the way of citation and proposal cards, and it would bypass the `AgentChannel` boundary. |
| pdf.js | Server images already align with anchors and avoid parsing untrusted PDFs in the browser. |
| AG-UI interrupts | The events docs mark that flow as draft. The normal review API does the job. |
| Annotation UI, live co-editing | Not required. |
| SSO | Out of scope in spec 012. |
| Mobile layouts, UI translation | Desktop-first and English only for now. Vault content can still be in any tested language. |
| Runtime editing of settings | Settings are validated at startup, so the console shows them read-only. |

## Work order

Specs 035 and 036 run alongside the backend, since they don't depend on it. Each screen follows its backend spec, as its `Depends on` line
states. Spec 033, the 1M benchmark, doesn't block the frontend.

## Spec map

| Area | Specs |
|---|---|
| Foundation | 035, 036, 040 |
| Live processing | 041 |
| Documents and search | 042, 043 |
| Pages and entities | 044, 045 |
| Governance and agents | 046, 047 |
| Administration | 048 |
| Chat | 050 |
| Acceptance | 051 |
| Backend support | 034, 037, 038, 039, 049 (design in [architecture.md](architecture.md)) |
