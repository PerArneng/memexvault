---
id: 036
name: design-system
description: Build the dark professional design system with tokens and accessible components
---

# 036 design-system

## Goal
A dark, professional visual language in `packages/ui`, with every colour pair meeting WCAG AA by a script, so screens are consistent without one-off
styling.

## Background
**Depends on:** 035. Design: `docs/frontend-architecture.md`, sections Decisions, Quality.

The console is dense and data-heavy, so it should read as a professional tool: restrained colour, clear hierarchy, tabular numbers, and colour used
for meaning such as trust tier, status and conflict.

## Approach
- Design tokens for colour, type, spacing, radius and elevation, in Tailwind. Semantic names only (`surface`, `danger`), never raw colours in views.
- Components on Radix primitives: button, input, select, dialog, menu, tabs, tooltip, toast, badge, data table with virtualization, split pane,
  skeleton, empty state, error state, command palette, and timeline.
- Domain badges: trust tier, epistemic status, job stage, conflict.
- A gallery route showing every component and state, used for review and tests.
- Interface fonts bundled with the app, so nothing loads from a remote host.

## Verification
1. A script checks every declared text and surface token pair: all meet 4.5:1, or 3:1 for large text and icons.
2. axe finds 0 violations on the gallery.
3. Keyboard test: every interactive component can be reached and used, with visible focus.
4. A lint rule fails on raw colour values outside the token file. A temporary raw colour fails, recorded.
5. The data table scrolls 100,000 rows at ≥50 fps on the reference machine, measured in Playwright with a trace.
6. Screenshots of the gallery at 1024 and 1920 px wide are recorded in the Result and reviewed by the user. Whether it looks professional is the
   user's call.

## Out of scope
Light theme, mobile layouts and screens.

## Result
Built the dark design system in `packages/ui`: tokens in `src/tokens.css`, all the listed components, domain badges with an icon and a word so colour is never the only signal, and a gallery at `/gallery`. Tailwind's default palette is removed, so only semantic names exist. Inter and JetBrains Mono are bundled, and the test confirms no request leaves the origin. Production build 407 kB of JS and 24 kB of CSS, before gzip.

**Verification.**
1. `pnpm run contrast`: 53 pairs, 0 failed, tightest margin 1.05× of its requirement. It first caught `border-strong` on `surface-selected` at 2.92:1, so that token was lightened.
2. axe: 0 violations on the gallery closed, and with the dialog, palette, menu and toast open. Page-structure rules are off for the dialog and palette, because a modal correctly hides the page. `region` is off for the menu, a transient portalled popover.
3. Keyboard test passes for dialog, menu, select, tabs, tooltip, palette, split pane, table and toast, including visible focus. It found one real bug, now fixed: controlled dialogs didn't return focus on close. Menus became non-modal, because Radix's modal menu leaves focusable content under `aria-hidden`.
4. Lint fails on a temporary `bg-[#ff0000]` and on `rgb(1,2,3)`: "Use a design token". CSS colours can only be in `tokens.css`, since no other palette exists, though the lint itself doesn't scan `.css` files.
5. The table scrolled 100,000 rows at 60 fps, frame gap p95 16.7 ms, with 36 rows in the DOM, in headless Chromium through `requestAnimationFrame` timing in a Playwright test. That's a substitute for the trace the spec named, and it covers only the scrolling page's frame callback, not GPU compositing.
6. Screenshots are in `screens/`. **The user's review is outstanding**, tracked by spec 056, which assumes the look is acceptable for now.

**Left out.** `@tanstack/react-table`: the table needs only virtualization so far, and sorting is server-side by the API conventions. Spec 040 can add it if a screen needs it. Playwright is pinned to 1.64 to match the browsers already installed here, with `playwright-core` listed so that axe resolves the same version.
