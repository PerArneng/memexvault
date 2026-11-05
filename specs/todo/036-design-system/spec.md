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
Not started.
