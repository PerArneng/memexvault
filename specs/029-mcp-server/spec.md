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
Not started.
