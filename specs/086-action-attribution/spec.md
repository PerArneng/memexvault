---
id: 086
name: action-attribution
description: "Show who did what, by name, after a reload: job actions, page edits and proposals"
---

# 086 action-attribution

**Depends on:** 041, 044.

## Goal
The console can say who did something, by name, and still say it after a reload.

## Background
Two console screens found the same gap. Spec 041's "Cancelled by" and "Retried by" lines go on reload, because the event log records the actor (`crates/memex-types/src/event.rs`) but no route shows it. Spec 041 proved attribution only by querying the database. Spec 044's attribution shows a principal's id, not a name, because listing principals needs admin, and each history row reads its changeset for attribution, at most 50 per screen.

## Approach
- A read route, at read scope, that gives a job's or an object's actions with their actor. Only what this needs is added, and what it exposes follows the same safety rules as the activity stream's frames.
- A way for a read-scope user to see a principal's display name and whether it's a person or an agent, without listing all principals or exposing tokens or scopes.
- Spec 041's job screen and spec 044's page history use them. An actor field on `RevisionDto` replaces per-row changeset reads, if that's the simpler way.
- A deleted or unknown principal shows as such.

## Verification
1. A job cancelled and retried from the console shows both actors by name after a reload, in Playwright against a real server, with no SQL.
2. Page history shows names for a person and an agent, using a read-scope token, with the number of requests per screen recorded.
3. A read-scope token gets a name and kind but no token material or scopes, and a token with no access to the vault gets a refusal.
4. A principal that no longer exists shows clearly rather than as an error.
5. A name holding HTML renders as plain text.
6. `e2e:ingest` and `e2e:wiki` pass, and the API types are regenerated.

## Out of scope
A full audit log screen and editing names.

## Result
(Filled in when the spec moves to done or dropped: what was built, decisions, numbers, follow-ups.)
