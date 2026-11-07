#!/usr/bin/env bash
# Spec 029, Verification item 2 with the official MCP inspector: connects to the real binary over Streamable HTTP and stdio, and calls every tool and resource.
# It runs from npx's cache, so it needs no system install, and it calls no LLM.
#
# Usage, from the repository root, after `cargo build -p memexvault`:
#   MEMEX_DATABASE_URL=postgres://... MEMEX_MODELS_DIR=<dir with the models> PORT=18029 specs/029-mcp-server/inspector.sh
# The database must be empty and your own: this bootstraps it. The first failure stops the script.
set -euo pipefail

: "${MEMEX_DATABASE_URL:?set MEMEX_DATABASE_URL to an empty database of your own}"
: "${MEMEX_MODELS_DIR:?set MEMEX_MODELS_DIR}"
: "${PORT:=18029}"
BIN=${BIN:-target/debug/memexvault}
INSPECTOR=@modelcontextprotocol/inspector@${INSPECTOR_VERSION:-2.13.0}
export MEMEX_DATABASE_URL MEMEX_MODELS_DIR
export MEMEX_DATA_DIR=${MEMEX_DATA_DIR:-$(mktemp -d)}
export MEMEX_HTTP_BIND=127.0.0.1:$PORT
URL=http://127.0.0.1:$PORT
SERVER=

finish() {
  [ -n "$SERVER" ] && kill -INT "$SERVER" 2>/dev/null && wait "$SERVER" 2>/dev/null
  return 0
}
trap finish EXIT

"$BIN" migrate >/dev/null
TOKEN=$("$BIN" bootstrap inspector)

start_server() {
  "$BIN" serve 2>"$MEMEX_DATA_DIR/serve.log" &
  SERVER=$!
  for _ in $(seq 1 120); do
    curl -fs "$URL/api/v1/health" >/dev/null 2>&1 && return 0
    sleep 0.5
  done
  echo "the server didn't start" >&2
  cat "$MEMEX_DATA_DIR/serve.log" >&2
  return 1
}

api() { curl -fsS -H "authorization: Bearer $TOKEN" -H 'content-type: application/json' "$@"; }

http() { npx -y "$INSPECTOR" --cli "$URL/api/v1/mcp" --transport http --header "Authorization: Bearer $TOKEN" "$@"; }
stdio() { npx -y "$INSPECTOR" --cli -e "MEMEX_MCP_TOKEN=$TOKEN" -e "MEMEX_DATABASE_URL=$MEMEX_DATABASE_URL" -e "MEMEX_MODELS_DIR=$MEMEX_MODELS_DIR" -e "MEMEX_DATA_DIR=$MEMEX_DATA_DIR" "$BIN" mcp "$@"; }

# The text a tool returned, as JSON, and whether it says it failed.
tool_data() { jq -e 'if .isError then error("tool error: \(.content[0].text)") else .content[0].text | fromjson end'; }

tool() {
  local via=$1 name=$2
  shift 2
  local args=()
  for a in "$@"; do args+=(--tool-arg "$a"); done
  "$via" --method tools/call --tool-name "$name" "${args[@]}" | tool_data
}

start_server
echo "inspector: $INSPECTOR"
VAULT=$(api -X POST "$URL/api/v1/vaults" -d '{"name":"inspector"}' | jq -r .id)

check_all() {
  local via=$1 tag=$2
  echo "--- $via: initialize and list"
  "$via" --method tools/list --strict | jq -e '[.tools[].name] == ["search","get","neighborhood","resolve","propose","ingest","changes","provenance"]' >/dev/null
  "$via" --method resources/list | jq -e '.resources[0].uri == "memex://vaults"' >/dev/null
  "$via" --method resources/templates/list | jq -e '.resourceTemplates | length == 2' >/dev/null
  "$via" --method resources/read --uri memex://vaults | jq -e '.contents[0].text | fromjson | .data.items | length == 1' >/dev/null

  echo "--- $via: ingest"
  local up source job segment claim page
  up=$(tool "$via" ingest "vault=$VAULT" "name=inspector-$tag.txt" "text=The $tag pump station opened in 1912 and still runs." trust_tier=untrusted)
  source=$(jq -r .data.source.id <<<"$up")
  job=$(jq -r .data.job_id <<<"$up")
  for _ in $(seq 1 120); do
    [ "$(tool "$via" get "vault=$VAULT" kind=job "id=$job" | jq -r .data.state)" = done ] && break
    sleep 1
  done
  [ "$(tool "$via" get "vault=$VAULT" kind=job "id=$job" | jq -r .data.state)" = done ]
  echo "--- $via: get source and search"
  tool "$via" get "vault=$VAULT" kind=source "id=$source" | jq -e '.trust_tiers == ["untrusted"]' >/dev/null
  for _ in $(seq 1 60); do
    segment=$(tool "$via" search "vault=$VAULT" "q=$tag pump" kind=segment "source_id=$source" | jq -r '.data.items[0].id // empty')
    [ -n "$segment" ] && break
    sleep 1
  done
  [ -n "$segment" ]
  tool "$via" get "vault=$VAULT" kind=segment "id=$segment" | jq -e '.notice != null and .trust_tiers == ["untrusted"]' >/dev/null

  echo "--- $via: propose"
  claim=claim-$tag
  page=page-$tag
  local ops
  ops=$(jq -nc --arg c "$claim" --arg s "$segment" --arg p "$page" --arg tag "$tag" '[
    {op:"add_assertion",id:$c,subject:("pump " + $tag),predicate:"opened",value:"1912",epistemic:"asserted",machine:false,confidence_permille:900,valid_from_ms:null,valid_to_ms:null,evidence:[[($c + "-e"),{segment_id:$s,quote:"opened in 1912",stance:"supports"}]]},
    {op:"create_page",id:$p,title:("Pump " + $tag),body:("About [[Stations]]. " + $c)}]')
  local proposed cs
  proposed=$(tool "$via" propose "vault=$VAULT" "ops=$ops" rationale=seen "idempotency_key=$tag" detail=detailed)
  cs=$(jq -r .data.changeset.id <<<"$proposed")
  if [ "$(jq -r .data.changeset.state <<<"$proposed")" = pending ]; then
    api -X POST "$URL/api/v1/vaults/$VAULT/changesets/$cs/approve" -d '{}' >/dev/null
  fi

  echo "--- $via: the rest"
  tool "$via" get "vault=$VAULT" kind=page "id=$page" | jq -e '.data.title != null' >/dev/null
  tool "$via" get "vault=$VAULT" kind=claim "id=$claim" | jq -e '.trust_tiers == ["untrusted"]' >/dev/null
  tool "$via" get "vault=$VAULT" kind=changeset "id=$cs" | jq -e '.data.id != null' >/dev/null
  tool "$via" neighborhood "vault=$VAULT" "id=$page" | jq -e '.data.items | length == 1' >/dev/null
  tool "$via" resolve "vault=$VAULT" mention=nobody | jq -e '.data != null' >/dev/null
  tool "$via" changes "vault=$VAULT" limit=3 | jq -e '.data.items | length == 3' >/dev/null
  tool "$via" provenance "vault=$VAULT" "id=$claim" | jq -e '.data.evidence[0].trust_tier == "untrusted"' >/dev/null
  "$via" --method resources/read --uri "memex://vaults/$VAULT/pages/$page" | jq -e '.contents[1].text | contains("Stations")' >/dev/null
  "$via" --method resources/read --uri "memex://vaults/$VAULT/sources/$source" | jq -e '.contents[1].text | contains("1912")' >/dev/null
  echo "--- $via: every tool and resource answered"
}

# Over HTTP, against `serve`. stdio can't run beside it, as a vault has one writer, and it says so.
check_all http http
echo "--- stdio beside serve: refused"
if MEMEX_MCP_TOKEN=$TOKEN "$BIN" mcp </dev/null 2>"$MEMEX_DATA_DIR/refused.log"; then
  echo "stdio started beside serve" >&2
  exit 1
fi
grep -q "memexvault serve" "$MEMEX_DATA_DIR/refused.log"
kill -INT "$SERVER"
wait "$SERVER" 2>/dev/null || true
SERVER=

echo "--- stdio: a bad token is refused before anything starts"
if MEMEX_MCP_TOKEN=mxt_wrong "$BIN" mcp </dev/null 2>/dev/null; then
  echo "a bad token started" >&2
  exit 1
fi
check_all stdio stdio
echo "PASS: the official inspector connected over both transports and called every tool and resource"
