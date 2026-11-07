---
id: 029
name: mcp-server
description: Expose the backend to agents over MCP with a small set of semantic tools matching the HTTP API
---

# 029 mcp-server

## Goal
Agents can use the vault through about 8 high-level MCP tools. Each behaves exactly like its HTTP equivalent, and tool
definitions stay small and static.

## Background
**Depends on:** 017, 020, 022, 023. Design: `docs/architecture.md`, sections Search, Prompt injection.

Check the `rmcp` 3.9 protocol version and transports when starting. All tools share the `Engine` facade with HTTP.

## Approach
- `memex-mcp` on `rmcp`, as a thin frontend. No business logic.
- Tools: `search`, `get`, `neighborhood`, `resolve`, `propose`, `ingest`, `changes`, `provenance`.
- Resources for pages and sources by URI.
- Streamable HTTP with the same tokens and scopes, and stdio for local use with a token from settings.
- Concise and detailed response modes, a response size cap and cursors.
- Results mark content from documents as data, with trust tier.
- Tool descriptions are static. A snapshot test pins their hash, and they are never built from vault content.

## Verification
1. Parity tests: for a fixture set, each tool and its HTTP route return equivalent data.
2. The official MCP inspector, or an equivalent client, connects over both transports and calls every tool.
3. The serialized tool list is at most 2,000 tokens, measured with a stated tokenizer.
4. Scope matrix: read tokens can't propose, and other vaults are invisible.
5. A live agent test: an agent answers 10 questions from the golden set with citations through MCP, using only these tools.
6. Bad cases: invalid arguments return useful errors, oversized responses are cut with a cursor, and a document containing tool-like
   instructions has no effect on tool definitions.

## Out of scope
MCP prompts unless a need appears, sampling, tools for vault administration and a client SDK.

## Result

**Built.** `memex-mcp` on `rmcp` =3.9.0: the eight tools, resources at `memex://vaults`, `memex://vaults/{vault}/pages/{id}` and `.../sources/{id}`, Streamable HTTP at `/api/v1/mcp` from `serve`, and stdio from `memexvault mcp` with `MEMEX_MCP_TOKEN`. The design is in `docs/architecture.md`, section MCP. It holds no rules: tools parse, call the `Server` facade and answer with the DTOs HTTP uses.

**`rmcp` check, on starting.** 3.9.0 knows protocol versions 2024-11-05, 2025-03-26, 2025-06-18, 2025-11-25 and 2026-07-28. `LATEST` is 2026-07-28, which has no `initialize` and no sessions, and `LATEST_WITH_INITIALIZE` is 2025-11-25. Both `rmcp`'s client and the inspector settled on 2025-11-25 here, and a test also connects at 2025-03-26. Transports used: stdio, and Streamable HTTP as a tower service, with no session (`legacy_session_mode: false`) and JSON answers. Its defaults that matter: `Host` limited to loopback, and a 4 MiB request limit, raised here to 12 MiB for `ingest`. Its licence is Apache-2.0, and `cargo deny` needed no new entry.

**Verification.**

| Item | Result | Command | Measured |
|---|---|---|---|
| 1 Parity | Passed | `cargo test -p memex-mcp --test mcp` | `detailed` equals the HTTP body byte for byte as JSON in 17 read cases across all read tools, plus `get` claim, a dry-run proposal, and the same changeset and source through both. 7 failures have the same code and message. 9 of the read cases also pass on real Postgres and tantivy in `pg_mcp_every_tool_matches_http_on_real_storage`. |
| 2 Client | Passed, with the official inspector | `specs/029-mcp-server/inspector.sh` | `@modelcontextprotocol/inspector` 2.13.0 --cli, through npx with no system install, against the real binary and my own database: `tools/list --strict` with no error-severity schema problem, every tool, and all resources, over Streamable HTTP and real stdio. `rmcp`'s client does the same in `cargo test` over HTTP and over a pipe with stdio's framing. |
| 3 Size | Passed | `the_tool_list_fits_in_2000_tokens_with_cl100k_base` | 5,569 bytes, 1,238 tokens with `cl100k_base` (`tiktoken-rs`), on the whole `tools/list` result, against ≤2,000. The hash of the tools, instructions and notice is pinned by a test. |
| 4 Scopes | Passed | `the_scope_matrix_holds_on_both_transports` | 5 tokens by 8 tools and a resource on both transports. Read can't `propose` or `ingest`. Another vault's token and a token with no grants get `not_found`, and denials change nothing and name nothing. Also on Postgres for read. Bad, missing and revoked tokens get 401, a cookie gets 403 or 401, and a strange `Host` gets 403. |
| 5 Live agent | **NOT RUN** | See below | No number. It needs an LLM and the user's decision. |
| 6 Bad cases | Passed | `invalid_arguments_get_errors_that_say_what_to_fix`, `oversized_answers_...`, `tool_like_text_...` | 24 invalid-argument cases each name what to fix. At a 2 KiB limit, every page stayed under it and cursors reached every item once on all list tools and search, and a page's resource slices add up to its body. A document with a planted tool definition and instructions left the tool list, templates, instructions and hash unchanged, and came back only inside `data`, with its tier. |

**Item 5, not run.** Built and proven with a scripted endpoint only: `crates/memex-mcp/tests/agent.rs` runs the real tool loop, offering the MCP tools and nothing else, over the OpenAI-compatible wire format, through Streamable HTTP. 10 golden questions finish with citations, an answer citing what no tool returned doesn't count, an unknown tool is refused and recorded, and a model that never answers stops at 6 turns. That script is not a model, so it's no measure of answers. The pass rule, set before any run: all 10 finish citing only sources the tools returned, using only these tools, and at least 8 cite the source holding the answer. To run it, with a server up:

`MEMEX_MCP_URL=... MEMEX_MCP_TOKEN=... MEMEX_TEST_CORPUS=... MEMEX_LLM_BASE_URL=... MEMEX_LLM_MODEL=... MEMEX_EVAL_MAX_TOKENS=150000 MEMEX_EVAL_APPROVED=1 cargo test -p memex-mcp --test agent -- --ignored --nocapture --exact live_agent_answers_ten_golden_questions`

Estimate: about 80,000 tokens, from about 8,000 per question at two turns (1,238 for tools, up to about 5,000 for a search answer). A question using all 6 turns could reach about 30,000, so `MEMEX_EVAL_MAX_TOKENS` stops the run. It sends those passages to the provider, and they're public Wikipedia text from spec 004.

**Other measurements.** One call over MCP on real storage took 3.3-8.1 ms in a debug build on a busy machine, with three other workers running, so it's a sanity figure, not a benchmark. All Postgres tests: 19 passed, on a fresh database with the release command. `pg_child` and `pg_job_child` (helpers), `pg_full_disk` (needs a small disk), `pg_kill_postgres` and `pg_bad_cases` (would disturb the shared server) weren't run. Nothing here touches the store or its writer, which is what they cover.

**Decisions.**
- **Backend changes, kept small.** `Server::neighborhood` and `GET /vaults/{id}/objects/{oid}/neighborhood`, since the tool had no HTTP equivalent. It's one hop both ways, and kinds default to the three built in, because the store reads edges by kind. The mappings HTTP had inline moved to `memex-api/src/convert.rs`, so the two frontends can't drift, which gives `memex-api` a dependency on `memex-core`. `memex_http::router_with` and `serve_with` mount MCP from the composition root, and the routes keep the API's id, log line and security headers. No migrations.
- **Stateless HTTP,** as no tool sends anything unprompted. Cookies are refused, so a browser session can't reach MCP.
- **Stdio is the whole server.** A vault has one writer, so a second process couldn't write, and it refuses to start beside `serve` and says why. A bridge to a running server would need its own design.
- **`vault` is optional** when a token sees exactly one, since no vault tool exists to find it. `memex://vaults` and the error both list what's visible.
- **`get` also takes `changeset` and `job`,** so an agent can follow what `propose` and `ingest` start.
- **Limits.** 64 KiB per answer, and search budgets over 16,384 tokens are clamped, unlike HTTP, and marked `capped`. `ingest` takes 8 MiB. An item too large for a page is replaced by its short fields and `omitted_bytes`, not cut.
- **Found on the way.** A search item larger than any allowed budget would repeat "budget too small" with the same cursor for ever, as HTTP can raise the budget and a capped caller can't. The tool now reads that item alone and passes over it if it's still too large.

**Follow-ups.**
- The user decides on running item 5.
- `changes_since` reads the whole log on each call, for HTTP and MCP alike, before paging.
- Approving isn't a tool, as spec 025 owns review.
- Spec 030 or 062 could bound what MCP adds to `PgStore`'s connections, which are as unbounded as HTTP's.
