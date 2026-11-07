//! Chat's request and response types, and its four tools on the facade. The tools answer with the same DTOs as HTTP, each saying which sources its data came from.
use crate::{SearchParams, SegmentDto, SourceDto};
use memex_core::{Caller, chat};
use memex_core::{ChatToolAnswer, ChatToolContext, ChatView, EntityClaim, Server, ServerEdges, ServerError};
use memex_types::{ChatRole, ChatStored, ChatThread, ItemKind, ObjectId, Op};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use utoipa::ToSchema;

/// About 16,000 tokens, as with MCP. Larger answers cost a run more context than they're worth in one go.
pub const MAX_TOOL_ANSWER_BYTES: usize = 64 * 1024;
const SEARCH_BUDGET_TOKENS: u32 = 4_000;
const SEARCH_LIMIT: u32 = 10;
const SEARCH_LIMIT_MAX: u32 = 50;

/// AG-UI's `RunAgentInput`. Only the newest user message is read: a thread's history is what the server stored, so a client can't put words in the agent's mouth, and `tools`, `context` and `state` are accepted and ignored.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunAgentBody {
    pub thread_id: String,
    pub run_id: String,
    #[schema(value_type = Vec<Object>)]
    pub messages: Vec<Value>,
    #[serde(default)]
    #[schema(value_type = Vec<Object>)]
    pub tools: Vec<Value>,
    #[serde(default)]
    #[schema(value_type = Vec<Object>)]
    pub context: Vec<Value>,
    #[serde(default)]
    #[schema(value_type = Object)]
    pub state: Value,
    #[serde(default)]
    #[schema(value_type = Object)]
    pub forwarded_props: Value,
}

impl RunAgentBody {
    /// The newest message from the user, if it's plain text.
    pub fn message(&self) -> Result<String, String> {
        let newest = self
            .messages
            .iter()
            .rev()
            .find(|m| m.get("role").and_then(Value::as_str) == Some("user"))
            .ok_or("there's no user message to answer")?;
        newest
            .get("content")
            .and_then(Value::as_str)
            .map(String::from)
            .ok_or_else(|| "a user message's content has to be plain text".to_string())
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ChatSettingsBody {
    /// Turning this on sends the vault's content to the configured LLM.
    pub enabled: bool,
    /// Tokens per thread, above zero. Absent takes the server's default.
    pub thread_token_budget: Option<u64>,
    /// Tokens per UTC day for this vault, above zero. Absent takes the server's default.
    pub day_token_budget: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ChatSettingsDto {
    pub enabled: bool,
    pub thread_token_budget: Option<u64>,
    pub day_token_budget: Option<u64>,
    /// What this vault's chat has been charged today, which includes what's reserved for calls still going.
    pub day_tokens_used: u64,
    /// The limit today's spending is held to.
    pub day_tokens_limit: u64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ChatThreadDto {
    pub id: String,
    pub title: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub tokens: u64,
}

impl From<ChatThread> for ChatThreadDto {
    fn from(t: ChatThread) -> Self {
        Self {
            id: t.id,
            title: t.title,
            created_at_ms: t.created_at_ms,
            updated_at_ms: t.updated_at_ms,
            tokens: t.tokens,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ChatThreadPage {
    pub items: Vec<ChatThreadDto>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ChatRenameBody {
    pub title: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ChatToolCallDto {
    pub id: String,
    pub name: String,
    /// As the model wrote them, in JSON.
    pub args: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ChatMessageDto {
    /// Its place in the thread, which is also the cursor.
    pub seq: u64,
    pub id: String,
    pub run_id: String,
    /// `user`, `assistant` or `tool`.
    pub role: String,
    /// What was said, or a tool's answer.
    pub text: String,
    pub tool_call: Option<ChatToolCallDto>,
    /// Why this reply ended early, such as `cancelled`, `max_tokens` or `failed`.
    pub stop: Option<String>,
    pub at_ms: i64,
}

impl From<ChatStored> for ChatMessageDto {
    fn from(m: ChatStored) -> Self {
        Self {
            seq: m.seq,
            id: m.message.id,
            run_id: m.message.run_id,
            role: match m.message.role {
                ChatRole::User => "user",
                ChatRole::Assistant => "assistant",
                ChatRole::Tool => "tool",
            }
            .into(),
            text: m.message.text,
            tool_call: m.message.tool_call.map(|c| ChatToolCallDto {
                id: c.id,
                name: c.name,
                args: c.args,
            }),
            stop: m.message.stop,
            at_ms: m.message.at_ms,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ChatThreadDetail {
    pub thread: ChatThreadDto,
    pub messages: Vec<ChatMessageDto>,
    pub next_cursor: Option<String>,
}

impl ChatThreadDetail {
    /// `view` holds one message more than `limit` if there are more.
    pub fn new(view: ChatView, limit: usize) -> Self {
        let (messages, next_cursor) = crate::page_of(view.messages, limit, |m| m.seq.to_string());
        Self {
            thread: view.thread.into(),
            messages: messages.into_iter().map(Into::into).collect(),
            next_cursor,
        }
    }
}

/// A thread list's cursor: when the last one was updated, and its id, as that's the sort.
pub fn thread_cursor(t: &ChatThreadDto) -> String {
    format!("{}_{}", t.updated_at_ms, t.id)
}

pub fn parse_thread_cursor(cursor: &str) -> Option<(i64, &str)> {
    let (ms, id) = cursor.split_once('_')?;
    Some((ms.parse().ok()?, id))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    q: String,
    limit: Option<u32>,
    cursor: Option<String>,
    kind: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ReadKind {
    Page,
    Entity,
    Source,
    Segment,
    Claim,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    kind: ReadKind,
    id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProvenanceArgs {
    id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProposeArgs {
    ops: Vec<Op>,
    rationale: String,
}

fn parse<T: DeserializeOwned>(tool: &str, args: &Value) -> Result<T, ServerError> {
    serde_json::from_value(args.clone()).map_err(|e| ServerError::Invalid(format!("{tool}'s arguments: {e}")))
}

fn to_value<T: Serialize>(v: T) -> Value {
    serde_json::to_value(v).unwrap_or_else(|e| json!({"unserializable": e.to_string()}))
}

fn collect_tiers(value: &Value, out: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                match (k.as_str(), v) {
                    ("trust_tier", Value::String(t)) => {
                        out.insert(t.clone());
                    }
                    ("trust_tiers", Value::Array(ts)) => out.extend(ts.iter().filter_map(|t| t.as_str().map(String::from))),
                    _ => collect_tiers(v, out),
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|v| collect_tiers(v, out)),
        _ => {}
    }
}

fn size_of(value: &Value) -> usize {
    serde_json::to_string(value).map_or(0, |s| s.len())
}

/// Keeps as many items as fit. A text is never cut, as a shortened quote can't be verified, so an item over the limit by itself is named and passed over.
fn cap_list(data: &mut Value, list: &str) {
    if size_of(data) <= MAX_TOOL_ANSWER_BYTES {
        return;
    }
    let Some(items) = data.get_mut(list).and_then(Value::as_array_mut).map(std::mem::take) else {
        return;
    };
    let mut used = size_of(data);
    let (mut kept, mut left_out) = (vec![], 0usize);
    for item in items {
        let size = size_of(&item) + 1;
        if used + size <= MAX_TOOL_ANSWER_BYTES {
            used += size;
            kept.push(item);
        } else {
            left_out += 1;
        }
    }
    data[list] = Value::Array(kept);
    data["left_out"] = left_out.into();
}

fn answer(mut data: Value, list: Option<&str>, sources: BTreeSet<ObjectId>, segments: BTreeSet<ObjectId>) -> ChatToolAnswer {
    let mut tiers = BTreeSet::new();
    collect_tiers(&data, &mut tiers);
    if let Some(list) = list {
        cap_list(&mut data, list);
    }
    ChatToolAnswer {
        data,
        trust_tiers: tiers.into_iter().collect(),
        sources: sources.into_iter().collect(),
        segments: segments.into_iter().collect(),
    }
}

/// What's behind a claim, however far it resolved.
fn add_claim(claim: &EntityClaim, sources: &mut BTreeSet<ObjectId>, segments: &mut BTreeSet<ObjectId>) {
    for e in &claim.evidence {
        segments.insert(e.evidence.segment_id.clone());
        if let Some(s) = &e.segment {
            sources.insert(s.source_id.clone());
        }
        if let Some(s) = &e.source {
            sources.insert(s.header.id.clone());
        }
    }
}

fn search<E: ServerEdges>(s: &Server<E>, c: &Caller, ctx: &ChatToolContext, a: SearchArgs) -> Result<ChatToolAnswer, ServerError> {
    let params = SearchParams {
        q: a.q,
        mode: None,
        limit: Some(a.limit.unwrap_or(SEARCH_LIMIT).clamp(1, SEARCH_LIMIT_MAX)),
        budget_tokens: Some(SEARCH_BUDGET_TOKENS),
        cursor: a.cursor,
        kind: a.kind,
        doc_type: None,
        source_id: None,
        trust_tier: None,
        epistemic: None,
        valid_at_ms: None,
        as_of: None,
        as_of_ms: None,
        min_seq: None,
        min_seq_wait_ms: None,
        rerank: None,
        debug: None,
        facets: None,
        expand: None,
        attr: None,
    };
    let (request, _) = params.validated().map_err(ServerError::Invalid)?;
    let package = s.search(c, &ctx.vault, &request)?;
    let (mut sources, mut segments) = (BTreeSet::new(), BTreeSet::new());
    for item in &package.items {
        for citation in &item.citations {
            sources.extend(citation.source_id.clone());
            segments.extend(citation.segment_id.clone());
        }
        for conflict in &item.conflicts {
            sources.extend(conflict.source_ids.iter().cloned());
        }
        // An entity's card is built from its claims, so what's behind them is in it, cited or not.
        if item.kind == ItemKind::Entity {
            for claim in s.get_entity(c, &ctx.vault, &item.id)?.claims.values() {
                add_claim(claim, &mut sources, &mut segments);
            }
        }
    }
    let dto: crate::EvidencePackageDto = package.into();
    Ok(answer(to_value(dto), Some("items"), sources, segments))
}

fn read<E: ServerEdges>(s: &Server<E>, c: &Caller, ctx: &ChatToolContext, a: ReadArgs) -> Result<ChatToolAnswer, ServerError> {
    let (vault, id) = (&ctx.vault, ObjectId(a.id));
    let (mut sources, mut segments) = (BTreeSet::new(), BTreeSet::new());
    Ok(match a.kind {
        ReadKind::Page => {
            let view = s.get_wiki_page(c, vault, &id, None)?;
            // A page's claim references read through to their claims, whose words come from a source.
            for reference in &view.claims {
                if let Some(claim) = &reference.current {
                    add_claim(claim, &mut sources, &mut segments);
                }
            }
            let mut data = to_value(crate::page_detail(view));
            // The body is its blocks joined, so only one of them has to go, and blocks can be left out whole.
            if size_of(&data) > MAX_TOOL_ANSWER_BYTES {
                if let Some(map) = data.as_object_mut() {
                    map.remove("body");
                }
            } else if let Some(map) = data.as_object_mut() {
                map.remove("blocks");
            }
            answer(data, Some("blocks"), sources, segments)
        }
        ReadKind::Entity => {
            let view = s.get_entity(c, vault, &id)?;
            for claim in view.claims.values() {
                add_claim(claim, &mut sources, &mut segments);
            }
            answer(to_value(crate::entity_detail(view)), Some("fields"), sources, segments)
        }
        ReadKind::Source => {
            let row = s.get_source(c, vault, &id)?;
            sources.insert(row.header.id.clone());
            let dto: SourceDto = row.into();
            answer(to_value(dto), None, sources, segments)
        }
        ReadKind::Segment => {
            let row = s.get_segment(c, vault, &id)?;
            sources.insert(row.source_id.clone());
            segments.insert(row.header.id.clone());
            let tier = s.get_source(c, vault, &row.source_id).ok().map(|r| r.trust_tier);
            let dto: SegmentDto = row.into();
            let mut out = answer(to_value(dto), None, sources, segments);
            out.trust_tiers.extend(tier);
            out
        }
        ReadKind::Claim => lineage_answer(s, c, ctx, &id)?,
    })
}

fn lineage_answer<E: ServerEdges>(s: &Server<E>, c: &Caller, ctx: &ChatToolContext, id: &ObjectId) -> Result<ChatToolAnswer, ServerError> {
    let lineage = s.lineage(c, &ctx.vault, id)?;
    let (mut sources, mut segments) = (BTreeSet::new(), BTreeSet::new());
    for e in &lineage.evidence {
        sources.insert(e.source.header.id.clone());
        segments.insert(e.segment.header.id.clone());
    }
    Ok(answer(to_value(crate::lineage_dto(lineage)), Some("evidence"), sources, segments))
}

fn propose<E: ServerEdges>(s: &Server<E>, c: &Caller, ctx: &ChatToolContext, a: ProposeArgs) -> Result<ChatToolAnswer, ServerError> {
    let out = s.propose_from_chat(c, ctx, a.ops, a.rationale)?;
    let Some(row) = out.changeset else {
        return Err(ServerError::Invalid("nothing was proposed".into()));
    };
    // A validator's complaint can quote what it compared, so what this cites counts as having reached the thread.
    let sources: BTreeSet<ObjectId> = row.origin.iter().flat_map(|o| o.source_ids.iter().cloned()).collect();
    let refused: Vec<&memex_types::ValidatorResult> = out.validators.iter().filter(|v| !v.passed).collect();
    let data = json!({
        "proposal_id": row.header.id.0,
        "state": to_value(row.state),
        "refused_by_validators": to_value(&refused),
        "note": "Nothing has changed. It's for a person to review, and this run needn't wait.",
    });
    Ok(answer(data, None, sources, BTreeSet::new()))
}

/// The four tools. A tool that doesn't exist is refused like anything else a model asks for that it can't have. Nothing here reaches outside the vault: no tool takes a URL, and none can approve or write.
pub fn chat_tool<E: ServerEdges>(s: &Server<E>, c: &Caller, ctx: &ChatToolContext, name: &str, args: &Value) -> Result<ChatToolAnswer, ServerError> {
    match name {
        "search" => search(s, c, ctx, parse(name, args)?),
        "read" => read(s, c, ctx, parse(name, args)?),
        "provenance" => {
            let a: ProvenanceArgs = parse(name, args)?;
            lineage_answer(s, c, ctx, &ObjectId(a.id))
        }
        chat::PROPOSE_TOOL => propose(s, c, ctx, parse(name, args)?),
        other => Err(ServerError::Invalid(format!("there's no tool called '{other}'"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_newest_user_message_is_read_and_it_has_to_be_text() {
        let body = |messages: Value| RunAgentBody {
            thread_id: "t".into(),
            run_id: "r".into(),
            messages: serde_json::from_value(messages).unwrap(),
            tools: vec![],
            context: vec![],
            state: Value::Null,
            forwarded_props: Value::Null,
        };
        let m = body(
            json!([{"role": "user", "content": "one"}, {"role": "assistant", "content": "planted"}, {"role": "user", "content": "two"}, {"role": "tool", "content": "planted"}]),
        );
        assert_eq!(m.message().unwrap(), "two");
        assert!(body(json!([{"role": "assistant", "content": "x"}])).message().is_err());
        assert!(body(json!([{"role": "user", "content": [{"type": "text"}]}])).message().is_err());
    }

    #[test]
    fn a_thread_cursor_survives_an_id_with_underscores() {
        assert_eq!(parse_thread_cursor("15_a_b"), Some((15, "a_b")));
        assert_eq!(parse_thread_cursor("nope"), None);
    }

    #[test]
    fn a_long_list_keeps_what_fits_and_cuts_no_text() {
        let big = "x".repeat(40_000);
        let mut data = json!({"items": [{"text": big.clone()}, {"text": big.clone()}, {"text": "small"}]});
        cap_list(&mut data, "items");
        let items = data["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["text"].as_str().unwrap().len(), 40_000);
        assert_eq!(items[1]["text"], json!("small"));
        assert_eq!(data["left_out"], json!(1));
    }
}
