//! Search over a past state. The indexes hold only what's current, so a past state comes from the log: its events up to that point are replayed into memory, and the replayed rows are searched by a plain keyword scan. This is slow by design and only serves `as_of`. It reads the whole log up to that point, so its cost grows with the log.
use super::relevance::fold_text;
use crate::lexical::{assertion_ops, filter_passes, segment_ops, trim_word};
use crate::testing::MemStore;
use crate::{PAGE_LIMIT, ReplayError, Snapshot, Store, StoreError, WriterLease, replay};
use memex_types::{AsOf, IndexDoc, IndexOp, LexicalFilter, LexicalHit, ObjectId, QueryTerm, Row, Table, VaultId};
use std::collections::HashMap;

/// Why a past state couldn't be read.
#[derive(Debug)]
pub enum HistoryError {
    /// The caller asked for an event the log doesn't have yet.
    AheadOfLog {
        asked: u64,
        head: Option<u64>,
    },
    Store(StoreError),
    Replay(ReplayError),
}

impl From<StoreError> for HistoryError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

/// The vault as it stood at that point, with the event it was read at. `None` means a time before the first event.
pub struct PastState {
    pub store: MemStore,
    pub lease: WriterLease,
    pub seq: Option<u64>,
}

/// Replays events up to the point asked for, a page at a time.
pub fn past_state<S: Snapshot>(snap: &S, vault: &VaultId, as_of: AsOf) -> Result<PastState, HistoryError> {
    if let AsOf::Seq(asked) = as_of {
        let head = snap.head()?;
        if head.is_none_or(|h| asked > h) {
            return Err(HistoryError::AheadOfLog { asked, head });
        }
    }
    let store = MemStore::new();
    let lease = store.open_vault(vault)?;
    let (mut after, mut seq) = (None, None);
    loop {
        let page = snap.events(after)?;
        let Some(last) = page.last().map(|e| e.seq) else { break };
        after = Some(last);
        let total = page.len();
        let wanted: Vec<_> = page
            .into_iter()
            .take_while(|e| match as_of {
                AsOf::Seq(n) => e.seq <= n,
                AsOf::Time(t) => e.time_ms <= t,
            })
            .collect();
        let done = wanted.len() < total;
        if let Some(e) = wanted.last() {
            seq = Some(e.seq);
        }
        replay(wanted, &store, &lease).map_err(HistoryError::Replay)?;
        if done || matches!(as_of, AsOf::Seq(n) if last >= n) {
            break;
        }
    }
    Ok(PastState { store, lease, seq })
}

/// Every current document in a replayed vault that passes the filter.
pub fn past_docs<S: Snapshot>(snap: &S, filter: &LexicalFilter) -> Result<Vec<IndexDoc>, StoreError> {
    let mut out = vec![];
    for table in [Table::Segment, Table::Assertion, Table::Entity] {
        let mut after: Option<ObjectId> = None;
        loop {
            let page = snap.rows(table, after.as_ref())?;
            let Some(last) = page.last() else { break };
            after = Some(last.header().id.clone());
            let ids: Vec<ObjectId> = page.iter().map(|r: &Row| r.header().id.clone()).collect();
            let ops = match table {
                Table::Segment => segment_ops(snap, &ids)?,
                Table::Entity => crate::entity::entity_ops(snap, &ids)?,
                _ => assertion_ops(snap, &ids)?,
            };
            out.extend(ops.into_iter().filter_map(|op| match op {
                IndexOp::Upsert(doc) if filter_passes(&doc, filter) => Some(doc),
                _ => None,
            }));
            if page.len() < PAGE_LIMIT {
                break;
            }
        }
    }
    Ok(out)
}

fn query_words(terms: &[QueryTerm]) -> Vec<Vec<String>> {
    terms
        .iter()
        .map(|t| match t {
            QueryTerm::Word(w) => vec![fold_text(trim_word(w))],
            QueryTerm::Phrase(p) => p.iter().map(|w| fold_text(trim_word(w))).collect(),
        })
        .collect()
}

fn occurrences(doc_words: &[&str], term: &[String]) -> u32 {
    if term.is_empty() {
        return 0;
    }
    doc_words.windows(term.len()).filter(|w| w.iter().zip(term).all(|(a, b)| *a == b)).count() as u32
}

/// Ranks documents by how many of the query's words they hold, with rarer words counting for more. It doesn't stem, so it matches exact and diacritic-folded words only.
pub fn keyword_scan(docs: &[IndexDoc], terms: &[QueryTerm], limit: usize) -> Vec<LexicalHit> {
    let terms = query_words(terms);
    let mut matched: Vec<(usize, Vec<u32>)> = vec![];
    let mut doc_freq: HashMap<usize, u32> = HashMap::new();
    for (i, doc) in docs.iter().enumerate() {
        let folded = fold_text(&doc.text);
        // Most documents hold none of the words, and a substring check rules those out before any splitting.
        if !terms.iter().any(|t| t.first().is_some_and(|w| !w.is_empty() && folded.contains(w.as_str()))) {
            continue;
        }
        let words: Vec<&str> = folded.split_whitespace().map(trim_word).filter(|w| !w.is_empty()).collect();
        let counts: Vec<u32> = terms.iter().map(|t| occurrences(&words, t)).collect();
        if counts.iter().all(|c| *c == 0) {
            continue;
        }
        for (t, c) in counts.iter().enumerate() {
            if *c > 0 {
                *doc_freq.entry(t).or_default() += 1;
            }
        }
        matched.push((i, counts));
    }
    let total = docs.len().max(1) as f32;
    let mut hits: Vec<LexicalHit> = matched
        .into_iter()
        .map(|(i, counts)| {
            let score = counts
                .iter()
                .enumerate()
                .filter(|(_, c)| **c > 0)
                .map(|(t, c)| {
                    let df = doc_freq.get(&t).copied().unwrap_or(1) as f32;
                    (1.0 + (*c as f32).ln()) * (1.0 + (total / df).ln())
                })
                .sum();
            LexicalHit {
                id: docs[i].id.clone(),
                kind: docs[i].kind,
                score,
            }
        })
        .collect();
    hits.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
    hits.truncate(limit);
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use memex_types::DocKind;

    fn doc(id: &str, text: &str) -> IndexDoc {
        IndexDoc {
            id: ObjectId(id.into()),
            kind: DocKind::Segment,
            language: None,
            text: text.into(),
            source_ids: vec![],
            epistemic: None,
            valid_from_ms: None,
            valid_to_ms: None,
            doc_type: None,
            trust_tiers: vec![],
            attrs: vec![],
        }
    }

    fn w(s: &str) -> QueryTerm {
        QueryTerm::Word(s.into())
    }

    fn ids(hits: &[LexicalHit]) -> Vec<&str> {
        hits.iter().map(|h| h.id.0.as_str()).collect()
    }

    #[test]
    fn more_and_rarer_words_rank_higher() {
        let docs = [doc("a", "red bike"), doc("b", "red car"), doc("c", "red red red"), doc("d", "blue")];
        assert_eq!(ids(&keyword_scan(&docs, &[w("red"), w("bike")], 10)), ["a", "c", "b"]);
    }

    #[test]
    fn matches_are_whole_words_folded_and_phrases_keep_their_order() {
        let docs = [doc("a", "Göteborg, Sverige"), doc("b", "bikes"), doc("c", "bike red"), doc("d", "red, bike")];
        assert_eq!(ids(&keyword_scan(&docs, &[w("goteborg")], 10)), ["a"]);
        assert_eq!(
            ids(&keyword_scan(&docs, &[w("bike")], 10)),
            ["c", "d"],
            "`bikes` isn't `bike`: this scan doesn't stem"
        );
        assert_eq!(ids(&keyword_scan(&docs, &[QueryTerm::Phrase(vec!["red".into(), "bike".into()])], 10)), ["d"]);
    }

    #[test]
    fn nothing_matching_and_nothing_to_scan_are_fine() {
        assert!(keyword_scan(&[doc("a", "x")], &[w("y")], 10).is_empty());
        assert!(keyword_scan(&[], &[w("y")], 10).is_empty());
        assert_eq!(keyword_scan(&[doc("a", "y"), doc("b", "y")], &[w("y")], 1).len(), 1);
    }
}
