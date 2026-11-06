//! The keyword index's trait and its pure policy: parsing a query, deciding when to commit, and turning rows into documents.
use crate::{Snapshot, StateRecorded, StoreError, UpcastError, upcast};
use memex_types::{
    AssertionRow, DocKind, Event, Generation, IndexDoc, IndexHealth, IndexOp, LexicalError, LexicalFilter, LexicalQuery, LexicalResults, Mutation, ObjectId,
    QueryTerm, RenditionRow, Row, RowKey, SegmentRow, Status, Table, VaultId,
};
use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::time::Duration;

/// A keyword index. Only the projector writes to it. A write takes effect at the next commit, and a commit is atomic and durable when it returns.
pub trait LexicalIndex {
    /// `deep` also verifies the live generation's contents, which reads all of it.
    fn health(&self, vault: &VaultId, deep: bool) -> Result<IndexHealth, LexicalError>;
    /// The last event the generation's latest commit covers.
    fn watermark(&self, vault: &VaultId, generation: Generation) -> Result<Option<u64>, LexicalError>;
    /// Starts an empty building generation, discarding any unfinished one.
    fn begin_build(&self, vault: &VaultId) -> Result<(), LexicalError>;
    /// Idempotent per object id.
    fn write(&self, vault: &VaultId, generation: Generation, ops: &[IndexOp]) -> Result<(), LexicalError>;
    fn commit(&self, vault: &VaultId, generation: Generation, watermark: Option<u64>) -> Result<(), LexicalError>;
    /// Makes the building generation live atomically. It needs a commit, and the old one stays for rollback.
    fn flip(&self, vault: &VaultId) -> Result<(), LexicalError>;
    /// Goes back to the generation before the last flip.
    fn rollback(&self, vault: &VaultId) -> Result<(), LexicalError>;
    fn search(&self, vault: &VaultId, query: &LexicalQuery) -> Result<LexicalResults, LexicalError>;
    fn delete_vault(&self, vault: &VaultId) -> Result<(), LexicalError>;
}

pub const MAX_QUERY_BYTES: usize = 4096;
pub const MAX_QUERY_TERMS: usize = 64;
pub const MAX_SEARCH_LIMIT: usize = 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryError {
    Empty,
    TooLong { limit: usize },
    TooManyTerms { limit: usize },
    UnbalancedQuote,
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("the query has no words to search for"),
            Self::TooLong { limit } => write!(f, "the query is over the limit of {limit} bytes"),
            Self::TooManyTerms { limit } => write!(f, "the query is over the limit of {limit} terms"),
            Self::UnbalancedQuote => f.write_str("the query has a quote that isn't closed"),
        }
    }
}

impl std::error::Error for QueryError {}

/// Punctuation around a word isn't part of it, but inside one it is, so `(XK-42/B),` is the word `XK-42/B`.
pub fn trim_word(word: &str) -> &str {
    word.trim_matches(|c: char| !c.is_alphanumeric())
}

/// The whole query language: words, and phrases in double quotes. Nothing else means anything, so there's no syntax to get wrong apart from an open quote.
pub fn parse_query(text: &str) -> Result<Vec<QueryTerm>, QueryError> {
    if text.len() > MAX_QUERY_BYTES {
        return Err(QueryError::TooLong { limit: MAX_QUERY_BYTES });
    }
    let mut terms = vec![];
    for (i, part) in text.split('"').enumerate() {
        let words = part.split_whitespace().map(trim_word).filter(|w| !w.is_empty()).map(String::from);
        if i % 2 == 0 {
            terms.extend(words.map(QueryTerm::Word));
        } else {
            let words: Vec<String> = words.collect();
            match words.len() {
                0 => {}
                1 => terms.extend(words.into_iter().map(QueryTerm::Word)),
                _ => terms.push(QueryTerm::Phrase(words)),
            }
        }
    }
    if text.matches('"').count() % 2 == 1 {
        return Err(QueryError::UnbalancedQuote);
    }
    if terms.is_empty() {
        return Err(QueryError::Empty);
    }
    if terms.len() > MAX_QUERY_TERMS {
        return Err(QueryError::TooManyTerms { limit: MAX_QUERY_TERMS });
    }
    Ok(terms)
}

/// Trades how soon a write is searchable against how much merging frequent commits cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitPolicy {
    /// How long written documents may wait.
    pub interval: Duration,
    /// How long a moved watermark with no documents may wait. Replaying those events costs little, so this is long.
    pub idle_interval: Duration,
    /// Commit anyway after this many writes.
    pub max_pending: usize,
}

impl Default for CommitPolicy {
    fn default() -> Self {
        Self {
            interval: Duration::from_millis(500),
            idle_interval: Duration::from_secs(30),
            max_pending: 50_000,
        }
    }
}

impl CommitPolicy {
    pub fn should_commit(&self, pending_writes: usize, watermark_moved: bool, since_commit: Duration) -> bool {
        if pending_writes >= self.max_pending {
            return true;
        }
        if pending_writes > 0 {
            return since_commit >= self.interval;
        }
        watermark_moved && since_commit >= self.idle_interval
    }
}

/// What an event changed that the index cares about.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Touched {
    pub segments: BTreeSet<ObjectId>,
    pub assertions: BTreeSet<ObjectId>,
    /// Renditions replaced or restored, since that changes whether their segments are current.
    pub renditions: BTreeSet<ObjectId>,
}

impl Touched {
    /// Reads an event's result. It reads no store: the projector looks up the current rows afterwards, which is what makes it idempotent.
    pub fn add_event(&mut self, event: &Event) -> Result<(), UpcastError> {
        let current = upcast(event)?;
        let mutations: Option<Vec<Mutation>> = match current.kind.as_str() {
            "segment.recorded" => {
                if let Some(id) = current.payload.get("id").and_then(|v| v.as_str()) {
                    self.segments.insert(ObjectId(id.into()));
                }
                None
            }
            "state.recorded" | "changeset.recorded" => serde_json::from_value::<StateRecorded>(current.payload).ok().map(|p| p.mutations),
            _ => None,
        };
        for m in mutations.into_iter().flatten() {
            self.add_mutation(m);
        }
        Ok(())
    }

    fn add_mutation(&mut self, mutation: Mutation) {
        match mutation {
            Mutation::Put(Row::Segment(r)) => {
                self.segments.insert(r.header.id);
            }
            Mutation::Put(Row::Assertion(r)) => {
                self.assertions.insert(r.header.id);
            }
            // A claim's document holds its evidence's quotes.
            Mutation::Put(Row::Evidence(r)) => {
                self.assertions.insert(r.assertion_id);
            }
            // A first version brings its segments in the same event, so only later versions need the extra read.
            Mutation::Put(Row::Rendition(r)) if r.header.version > 1 => {
                self.renditions.insert(r.header.id);
            }
            Mutation::Delete(RowKey { table: Table::Segment, id }) => {
                self.segments.insert(id);
            }
            Mutation::Delete(RowKey { table: Table::Assertion, id }) => {
                self.assertions.insert(id);
            }
            _ => {}
        }
    }

    pub fn is_empty(&self) -> bool {
        self.segments.is_empty() && self.assertions.is_empty() && self.renditions.is_empty()
    }
}

fn keys(table: Table, ids: &[ObjectId]) -> Vec<RowKey> {
    ids.iter().map(|id| RowKey { table, id: id.clone() }).collect()
}

fn current_rendition(r: &RenditionRow) -> bool {
    r.header.status == Status::Active && r.superseded_by.is_none()
}

fn segment_doc(s: SegmentRow) -> IndexDoc {
    let text = match s.heading {
        Some(heading) if !heading.is_empty() => format!("{heading}\n{}", s.text),
        _ => s.text,
    };
    IndexDoc {
        id: s.header.id,
        kind: DocKind::Segment,
        language: s.language,
        text,
        source_ids: vec![s.source_id],
        epistemic: None,
        valid_from_ms: None,
        valid_to_ms: None,
    }
}

/// The ids of renditions that are no longer current, out of those these segments belong to. One store call.
fn stale_renditions<S: Snapshot>(snap: &S, segments: &[SegmentRow]) -> Result<BTreeSet<ObjectId>, StoreError> {
    let ids: Vec<ObjectId> = segments
        .iter()
        .filter_map(|s| s.rendition_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if ids.is_empty() {
        return Ok(BTreeSet::new());
    }
    let rows = snap.get_many(&keys(Table::Rendition, &ids))?;
    Ok(ids
        .into_iter()
        .zip(rows)
        .filter(|(_, row)| !matches!(row, Some(Row::Rendition(r)) if current_rendition(r)))
        .map(|(id, _)| id)
        .collect())
}

fn segment_is_current(s: &SegmentRow, stale: &BTreeSet<ObjectId>) -> bool {
    s.header.status == Status::Active && s.rendition_id.as_ref().is_none_or(|r| !stale.contains(r))
}

/// The writes that bring the index up to date for these segments. Store calls don't grow with the number of ids.
pub fn segment_ops<S: Snapshot>(snap: &S, ids: &[ObjectId]) -> Result<Vec<IndexOp>, StoreError> {
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let rows = snap.get_many(&keys(Table::Segment, ids))?;
    let segments: Vec<SegmentRow> = rows
        .into_iter()
        .filter_map(|r| match r {
            Some(Row::Segment(s)) => Some(s),
            _ => None,
        })
        .collect();
    let stale = stale_renditions(snap, &segments)?;
    let mut found: HashMap<ObjectId, IndexOp> = HashMap::with_capacity(segments.len());
    for s in segments {
        let id = s.header.id.clone();
        let op = if segment_is_current(&s, &stale) {
            IndexOp::Upsert(segment_doc(s))
        } else {
            IndexOp::Delete(id.clone())
        };
        found.insert(id, op);
    }
    Ok(ids.iter().map(|id| found.remove(id).unwrap_or_else(|| IndexOp::Delete(id.clone()))).collect())
}

/// Strings as they are, and everything else in the claim's value flattened to its leaves.
fn value_text(value: &serde_json::Value, out: &mut String) {
    use serde_json::Value as V;
    match value {
        V::Null => {}
        V::Bool(b) => out.push_str(if *b { "true " } else { "false " }),
        V::Number(n) => {
            out.push_str(&n.to_string());
            out.push(' ');
        }
        V::String(s) => {
            out.push_str(s);
            out.push(' ');
        }
        V::Array(a) => a.iter().for_each(|v| value_text(v, out)),
        V::Object(o) => o.values().for_each(|v| value_text(v, out)),
    }
}

fn epistemic_name(a: &AssertionRow) -> Option<String> {
    serde_json::to_value(a.epistemic).ok().and_then(|v| v.as_str().map(String::from))
}

/// As `segment_ops`, for claims. A claim takes its sources from its evidence, and the language from its first evidence with one, since claims keep their source's language.
pub fn assertion_ops<S: Snapshot>(snap: &S, ids: &[ObjectId]) -> Result<Vec<IndexOp>, StoreError> {
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let rows = snap.get_many(&keys(Table::Assertion, ids))?;
    let assertions: Vec<AssertionRow> = rows
        .into_iter()
        .filter_map(|r| match r {
            Some(Row::Assertion(a)) if a.header.status == Status::Active && a.superseded_by.is_none() => Some(a),
            _ => None,
        })
        .collect();
    let live: Vec<ObjectId> = assertions.iter().map(|a| a.header.id.clone()).collect();
    let evidence = if live.is_empty() { vec![] } else { snap.evidence_of(&live)? };
    let segment_ids: Vec<ObjectId> = evidence.iter().map(|e| e.segment_id.clone()).collect::<BTreeSet<_>>().into_iter().collect();
    let segments: HashMap<ObjectId, SegmentRow> = if segment_ids.is_empty() {
        HashMap::new()
    } else {
        snap.get_many(&keys(Table::Segment, &segment_ids))?
            .into_iter()
            .filter_map(|r| match r {
                Some(Row::Segment(s)) => Some((s.header.id.clone(), s)),
                _ => None,
            })
            .collect()
    };
    let mut by_assertion: HashMap<&ObjectId, Vec<&memex_types::EvidenceRow>> = HashMap::new();
    for e in &evidence {
        by_assertion.entry(&e.assertion_id).or_default().push(e);
    }
    let mut found: HashMap<ObjectId, IndexOp> = HashMap::with_capacity(assertions.len());
    for a in &assertions {
        let mut text = format!("{} {} ", a.subject, a.predicate);
        value_text(&a.value, &mut text);
        let (mut sources, mut language) = (BTreeSet::new(), None);
        for e in by_assertion.get(&a.header.id).into_iter().flatten() {
            text.push_str(&e.quote);
            text.push('\n');
            if let Some(s) = segments.get(&e.segment_id) {
                sources.insert(s.source_id.clone());
                language = language.or_else(|| s.language.clone());
            }
        }
        found.insert(
            a.header.id.clone(),
            IndexOp::Upsert(IndexDoc {
                id: a.header.id.clone(),
                kind: DocKind::Claim,
                language,
                text,
                source_ids: sources.into_iter().collect(),
                epistemic: epistemic_name(a),
                valid_from_ms: a.valid_from_ms,
                valid_to_ms: a.valid_to_ms,
            }),
        );
    }
    Ok(ids.iter().map(|id| found.remove(id).unwrap_or_else(|| IndexOp::Delete(id.clone()))).collect())
}

/// Drops hits that stopped being current after they were indexed. Two store calls at most, however many hits.
pub fn fence_hits<S: Snapshot>(snap: &S, results: LexicalResults) -> Result<LexicalResults, StoreError> {
    let table = |kind| match kind {
        DocKind::Segment => Table::Segment,
        DocKind::Claim => Table::Assertion,
    };
    let lookups: Vec<RowKey> = results
        .hits
        .iter()
        .map(|h| RowKey {
            table: table(h.kind),
            id: h.id.clone(),
        })
        .collect();
    if lookups.is_empty() {
        return Ok(results);
    }
    let rows = snap.get_many(&lookups)?;
    let segments: Vec<SegmentRow> = rows
        .iter()
        .filter_map(|r| match r {
            Some(Row::Segment(s)) => Some(s.clone()),
            _ => None,
        })
        .collect();
    let stale = stale_renditions(snap, &segments)?;
    let hits = results
        .hits
        .into_iter()
        .zip(rows)
        .filter(|(_, row)| match row {
            Some(Row::Segment(s)) => segment_is_current(s, &stale),
            Some(Row::Assertion(a)) => a.header.status == Status::Active && a.superseded_by.is_none(),
            _ => false,
        })
        .map(|(hit, _)| hit)
        .collect();
    Ok(LexicalResults {
        hits,
        watermark: results.watermark,
    })
}

/// A query from its parts, checking what parsing doesn't.
pub fn build_query(text: &str, filter: LexicalFilter, limit: usize) -> Result<LexicalQuery, QueryError> {
    Ok(LexicalQuery {
        terms: parse_query(text)?,
        filter,
        limit: limit.min(MAX_SEARCH_LIMIT),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use memex_types::{Epistemic, EvidenceRow, Header, Stance, VaultId};
    use serde_json::json;

    fn word(w: &str) -> QueryTerm {
        QueryTerm::Word(w.into())
    }

    #[test]
    fn words_and_phrases_parse() {
        assert_eq!(parse_query("red bike").unwrap(), vec![word("red"), word("bike")]);
        assert_eq!(
            parse_query(r#"a "red bike" c"#).unwrap(),
            vec![word("a"), QueryTerm::Phrase(vec!["red".into(), "bike".into()]), word("c")]
        );
        assert_eq!(parse_query(r#""single""#).unwrap(), vec![word("single")]);
    }

    #[test]
    fn identifiers_stay_whole_and_outer_punctuation_goes() {
        assert_eq!(parse_query("(XK-42/B), ORA-00904:").unwrap(), vec![word("XK-42/B"), word("ORA-00904")]);
    }

    #[test]
    fn engine_syntax_means_nothing() {
        assert_eq!(parse_query("title:foo AND (bar").unwrap(), vec![word("title:foo"), word("AND"), word("bar")]);
    }

    #[test]
    fn bad_queries_say_what_is_wrong() {
        assert_eq!(parse_query(""), Err(QueryError::Empty));
        assert_eq!(parse_query("  ?! -- "), Err(QueryError::Empty));
        assert_eq!(parse_query(r#""""#), Err(QueryError::Empty));
        assert_eq!(parse_query(r#"open "quote"#), Err(QueryError::UnbalancedQuote));
        assert_eq!(
            parse_query(&"a".repeat(MAX_QUERY_BYTES + 1)),
            Err(QueryError::TooLong { limit: MAX_QUERY_BYTES })
        );
        assert_eq!(
            parse_query(&"a ".repeat(MAX_QUERY_TERMS + 1)),
            Err(QueryError::TooManyTerms { limit: MAX_QUERY_TERMS })
        );
    }

    #[test]
    fn a_limit_is_capped() {
        assert_eq!(build_query("x", LexicalFilter::default(), 1_000_000).unwrap().limit, MAX_SEARCH_LIMIT);
    }

    #[test]
    fn commit_policy_decides() {
        let p = CommitPolicy {
            interval: Duration::from_millis(500),
            idle_interval: Duration::from_secs(30),
            max_pending: 10,
        };
        assert!(!p.should_commit(0, false, Duration::from_secs(9999)), "nothing to do");
        assert!(!p.should_commit(1, true, Duration::from_millis(499)), "documents wait for the interval");
        assert!(p.should_commit(1, true, Duration::from_millis(500)));
        assert!(p.should_commit(10, true, Duration::ZERO), "too many pending");
        assert!(!p.should_commit(0, true, Duration::from_secs(29)), "a moved watermark alone waits longer");
        assert!(p.should_commit(0, true, Duration::from_secs(30)));
    }

    fn event(kind: &str, payload: serde_json::Value) -> Event {
        Event {
            seq: 0,
            vault: VaultId("v".into()),
            actor: "t".into(),
            time_ms: 0,
            kind: kind.into(),
            schema_version: 1,
            payload,
        }
    }

    fn header(id: &str, version: u32) -> Header {
        Header {
            id: ObjectId(id.into()),
            creator: "t".into(),
            status: Status::Active,
            version,
            acl_id: None,
        }
    }

    fn evidence(id: &str, assertion: &str) -> EvidenceRow {
        serde_json::from_value(json!({
            "header": header(id, 1), "assertion_id": assertion, "segment_id": "s", "quote": "q", "stance": Stance::Supports
        }))
        .unwrap()
    }

    #[test]
    fn touched_reads_what_events_changed() {
        let mut t = Touched::default();
        t.add_event(&event("segment.recorded", json!({"id": "s1", "source_id": "x", "language": null, "text": "t"})))
            .unwrap();
        t.add_event(&event("page.created", json!({"id": "p", "title": "t", "body": "b"}))).unwrap();
        let mutations = vec![
            Mutation::Put(Row::Evidence(evidence("e", "a1"))),
            Mutation::Delete(RowKey {
                table: Table::Assertion,
                id: ObjectId("a2".into()),
            }),
            Mutation::Delete(RowKey {
                table: Table::Segment,
                id: ObjectId("s2".into()),
            }),
            Mutation::Delete(RowKey {
                table: Table::Page,
                id: ObjectId("p".into()),
            }),
        ];
        t.add_event(&event("state.recorded", serde_json::to_value(StateRecorded { mutations }).unwrap()))
            .unwrap();
        let ids = |s: &BTreeSet<ObjectId>| s.iter().map(|i| i.0.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&t.segments), ["s1", "s2"]);
        assert_eq!(ids(&t.assertions), ["a1", "a2"]);
        assert!(t.renditions.is_empty());
    }

    #[test]
    fn an_event_of_another_kind_touches_nothing() {
        let mut t = Touched::default();
        t.add_event(&event("edge.unlinked", json!({"id": "e"}))).unwrap();
        assert!(t.is_empty());
    }

    #[test]
    fn value_text_flattens_to_leaves() {
        let mut out = String::new();
        value_text(&json!({"a": ["x", 2, true], "b": {"c": null, "d": "y"}}), &mut out);
        assert_eq!(out, "x 2 true y ");
    }

    #[test]
    fn epistemic_names_are_the_wire_names() {
        let a: AssertionRow = serde_json::from_value(json!({
            "header": header("a", 1), "subject": "s", "predicate": "p", "value": "v", "epistemic": Epistemic::Verified, "machine": false,
            "confidence_permille": 0, "valid_from_ms": null, "valid_to_ms": null, "canonical_hash": "h", "changeset_id": "c", "superseded_by": null
        }))
        .unwrap();
        assert_eq!(
            epistemic_name(&a),
            serde_json::to_value(Epistemic::Verified).unwrap().as_str().map(String::from)
        );
    }
}
