---
id: 047
name: audit-agents-ui
description: "Build the audit agent's dashboards with runs, findings, schedule, budgets and cost"
---

# 047 audit-agents-ui

## Goal
A user can see what the random checks found, how much to trust them, and what agents cost, and can start a run.

## Background
**Depends on:** 026, 039, 040. Design: `docs/architecture.md`, section Audit agent.

## Approach
- Run list and detail with seed, sample and budget used. Findings with each check's result, linked to the claim, source and the resulting review item.
- An on-demand run with seed and budget, and a schedule editor.
- Recall report from planted errors, with its sample size stated, and a warning when it's too small to trust.
- Agent activity and token use by agent, model and day. Cost only where a price is configured.
- Charts follow one chart style from the design system, with tables for exact numbers.

## Verification
1. A run started from the UI appears live, finishes, and its findings link to their review items, in Playwright with a scripted LLM.
2. Rerunning with the same seed shows the same sample.
3. Totals on screen match the API for a fixture month, including when no price is set, where no cost appears.
4. A recall figure with too few planted errors shows the warning.
5. A budget that runs out shows as stopped by budget, not as failed.
6. Bad cases: an invalid schedule is refused with a clear message, and a read-scope user can view but not start runs.

## Out of scope
Auto-fixing findings, reputation and external alerting.

## Result
Not started.
