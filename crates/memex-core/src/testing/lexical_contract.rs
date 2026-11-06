//! The contract any keyword index must meet.
use crate::LexicalIndex;
use crate::lexical::parse_query;
use memex_types::{DocKind, Generation, IndexDoc, IndexHealth, IndexOp, LexicalError, LexicalFilter, LexicalQuery, ObjectId, VaultId};

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
    }
}

fn up(id: &str, text: &str) -> IndexOp {
    IndexOp::Upsert(doc(id, text))
}

fn query(text: &str) -> LexicalQuery {
    LexicalQuery {
        terms: parse_query(text).expect("query"),
        filter: LexicalFilter::default(),
        limit: 100,
    }
}

fn ids<L: LexicalIndex>(l: &L, v: &VaultId, text: &str) -> Vec<String> {
    let mut out: Vec<String> = l.search(v, &query(text)).expect("search").hits.into_iter().map(|h| h.id.0).collect();
    out.sort();
    out
}

fn filtered<L: LexicalIndex>(l: &L, v: &VaultId, filter: LexicalFilter) -> Vec<String> {
    let q = LexicalQuery { filter, ..query("shared") };
    let mut out: Vec<String> = l.search(v, &q).expect("search").hits.into_iter().map(|h| h.id.0).collect();
    out.sort();
    out
}

/// A live generation holding these writes, committed at this watermark.
fn live<L: LexicalIndex>(l: &L, v: &VaultId, ops: &[IndexOp], watermark: Option<u64>) {
    l.begin_build(v).expect("begin");
    l.write(v, Generation::Building, ops).expect("write");
    l.commit(v, Generation::Building, watermark).expect("commit");
    l.flip(v).expect("flip");
}

/// Runs every check on fresh vault names under `prefix`.
pub fn run_all<L: LexicalIndex>(l: &L, prefix: &str) {
    let v = |n: &str| VaultId(format!("{prefix}{n}"));
    no_generation_yet(l, &v("none"));
    writes_appear_at_commit(l, &v("commit"));
    upsert_and_delete_are_idempotent(l, &v("upsert"));
    watermark_round_trips(l, &v("watermark"));
    identifiers_match_exactly(l, &v("ident"));
    diacritics_fold_and_exact_ranks_first(l, &v("fold"));
    phrases_need_their_order(l, &v("phrase"));
    filters_restrict(l, &v("filter"));
    limit_and_order(l, &v("limit"));
    generations_flip_and_roll_back(l, &v("gen"));
    flip_needs_a_committed_build(l, &v("flip"));
    vaults_are_isolated(l, &v("iso-a"), &v("iso-b"));
    deleting_a_vault_removes_it(l, &v("delete"));
}

fn no_generation_yet<L: LexicalIndex>(l: &L, v: &VaultId) {
    assert_eq!(l.health(v, true).expect("health"), IndexHealth::Missing);
    assert!(matches!(l.search(v, &query("x")), Err(LexicalError::Unavailable(_))));
    assert!(l.write(v, Generation::Live, &[up("a", "x")]).is_err());
}

fn writes_appear_at_commit<L: LexicalIndex>(l: &L, v: &VaultId) {
    live(l, v, &[up("a", "alpha")], Some(1));
    l.write(v, Generation::Live, &[up("b", "alpha")]).expect("write");
    assert_eq!(ids(l, v, "alpha"), ["a"], "not until it's committed");
    l.commit(v, Generation::Live, Some(2)).expect("commit");
    assert_eq!(ids(l, v, "alpha"), ["a", "b"]);
}

fn upsert_and_delete_are_idempotent<L: LexicalIndex>(l: &L, v: &VaultId) {
    live(l, v, &[up("a", "alpha"), up("a", "alpha")], Some(0));
    assert_eq!(ids(l, v, "alpha"), ["a"], "two writes of one id are one document");
    l.write(v, Generation::Live, &[up("a", "beta")]).expect("write");
    l.commit(v, Generation::Live, Some(1)).expect("commit");
    assert!(ids(l, v, "alpha").is_empty(), "the old text is gone");
    assert_eq!(ids(l, v, "beta"), ["a"]);
    let gone = IndexOp::Delete(ObjectId("a".into()));
    l.write(v, Generation::Live, &[gone.clone(), gone, IndexOp::Delete(ObjectId("never-there".into()))])
        .expect("write");
    l.commit(v, Generation::Live, Some(2)).expect("commit");
    assert!(ids(l, v, "beta").is_empty());
}

fn watermark_round_trips<L: LexicalIndex>(l: &L, v: &VaultId) {
    live(l, v, &[], None);
    assert_eq!(l.watermark(v, Generation::Live).expect("watermark"), None);
    assert_eq!(l.health(v, true).expect("health"), IndexHealth::Ready { watermark: None });
    l.commit(v, Generation::Live, Some(7)).expect("commit");
    assert_eq!(l.watermark(v, Generation::Live).expect("watermark"), Some(7));
    assert_eq!(l.search(v, &query("nothing")).expect("search").watermark, Some(7));
}

fn identifiers_match_exactly<L: LexicalIndex>(l: &L, v: &VaultId) {
    live(
        l,
        v,
        &[
            up("ora", "failed with ORA-00904: invalid identifier"),
            up("ora2", "failed with ORA-12345 instead"),
            up("xk", "part (XK-42/B), in stock"),
            up("xk2", "part XK-42/C in stock"),
        ],
        Some(0),
    );
    assert_eq!(ids(l, v, "ORA-00904"), ["ora"]);
    assert_eq!(ids(l, v, "ora-00904"), ["ora"], "case doesn't matter");
    assert_eq!(ids(l, v, "XK-42/B"), ["xk"]);
}

fn diacritics_fold_and_exact_ranks_first<L: LexicalIndex>(l: &L, v: &VaultId) {
    live(
        l,
        v,
        &[up("folded", "resa till Goteborg i maj"), up("exact", "resa till Göteborg i maj")],
        Some(0),
    );
    assert_eq!(ids(l, v, "Goteborg"), ["exact", "folded"]);
    assert_eq!(l.search(v, &query("Göteborg")).expect("search").hits[0].id.0, "exact");
    assert_eq!(l.search(v, &query("Goteborg")).expect("search").hits[0].id.0, "folded");
}

fn phrases_need_their_order<L: LexicalIndex>(l: &L, v: &VaultId) {
    live(l, v, &[up("a", "the red bike is here"), up("b", "the bike is red")], Some(0));
    assert_eq!(ids(l, v, r#""red bike""#), ["a"]);
    assert_eq!(ids(l, v, "red bike"), ["a", "b"]);
}

fn filters_restrict<L: LexicalIndex>(l: &L, v: &VaultId) {
    let claim = IndexDoc {
        kind: DocKind::Claim,
        source_ids: vec![ObjectId("s1".into()), ObjectId("s2".into())],
        epistemic: Some("verified".into()),
        valid_from_ms: Some(100),
        valid_to_ms: Some(200),
        ..doc("claim", "shared")
    };
    let segment = IndexDoc {
        source_ids: vec![ObjectId("s3".into())],
        ..doc("segment", "shared")
    };
    live(l, v, &[IndexOp::Upsert(claim), IndexOp::Upsert(segment)], Some(0));
    let f = LexicalFilter::default;
    assert_eq!(filtered(l, v, f()), ["claim", "segment"]);
    let kinds = |k| LexicalFilter { kinds: vec![k], ..f() };
    assert_eq!(filtered(l, v, kinds(DocKind::Claim)), ["claim"]);
    assert_eq!(filtered(l, v, kinds(DocKind::Segment)), ["segment"]);
    let source = |s: &str| LexicalFilter {
        source_ids: vec![ObjectId(s.into())],
        ..f()
    };
    assert_eq!(filtered(l, v, source("s2")), ["claim"], "any of a claim's sources counts");
    assert_eq!(filtered(l, v, source("s3")), ["segment"]);
    assert!(filtered(l, v, source("nope")).is_empty());
    let epistemic = LexicalFilter {
        epistemic: vec!["verified".into()],
        ..f()
    };
    assert_eq!(filtered(l, v, epistemic), ["claim"]);
    let at = |t| LexicalFilter { valid_at_ms: Some(t), ..f() };
    assert_eq!(filtered(l, v, at(150)), ["claim", "segment"]);
    assert_eq!(filtered(l, v, at(100)), ["claim", "segment"], "the start is included");
    assert_eq!(filtered(l, v, at(200)), ["segment"], "the end isn't, and no bounds means always valid");
    assert_eq!(filtered(l, v, at(99)), ["segment"]);
}

fn limit_and_order<L: LexicalIndex>(l: &L, v: &VaultId) {
    live(l, v, &[up("b", "tie"), up("a", "tie"), up("c", "tie"), up("d", "other words here")], Some(0));
    let mut q = query("tie");
    q.limit = 2;
    let hits = l.search(v, &q).expect("search").hits;
    assert_eq!(hits.len(), 2);
    assert!(hits[0].id < hits[1].id, "ties go in id order");
    assert_eq!(hits[0].kind, DocKind::Segment);
    assert!(hits[0].score > 0.0);
}

fn generations_flip_and_roll_back<L: LexicalIndex>(l: &L, v: &VaultId) {
    live(l, v, &[up("old", "word")], Some(5));
    l.begin_build(v).expect("begin");
    l.write(v, Generation::Building, &[up("new", "word")]).expect("write");
    l.commit(v, Generation::Building, Some(9)).expect("commit");
    assert_eq!(ids(l, v, "word"), ["old"], "the old one serves during a build");
    assert_eq!(l.watermark(v, Generation::Building).expect("watermark"), Some(9));
    assert_eq!(l.watermark(v, Generation::Live).expect("watermark"), Some(5));
    l.write(v, Generation::Live, &[up("old2", "word")])
        .expect("the old one keeps following the log");
    l.commit(v, Generation::Live, Some(6)).expect("commit");
    assert_eq!(ids(l, v, "word"), ["old", "old2"]);
    l.flip(v).expect("flip");
    assert_eq!(ids(l, v, "word"), ["new"]);
    assert_eq!(l.watermark(v, Generation::Live).expect("watermark"), Some(9));
    l.rollback(v).expect("rollback");
    assert_eq!(ids(l, v, "word"), ["old", "old2"]);
    assert_eq!(l.watermark(v, Generation::Live).expect("watermark"), Some(6));
    assert!(l.rollback(v).is_err(), "only one step back");
}

fn flip_needs_a_committed_build<L: LexicalIndex>(l: &L, v: &VaultId) {
    assert!(l.flip(v).is_err(), "nothing was built");
    live(l, v, &[up("a", "word")], Some(1));
    l.begin_build(v).expect("begin");
    l.write(v, Generation::Building, &[up("half", "word")]).expect("write");
    assert!(matches!(l.flip(v), Err(LexicalError::Invalid(_))), "an uncommitted build can't go live");
    assert_eq!(ids(l, v, "word"), ["a"]);
    l.begin_build(v).expect("a new build discards the unfinished one");
    l.commit(v, Generation::Building, Some(2)).expect("commit");
    l.flip(v).expect("flip");
    assert!(ids(l, v, "word").is_empty(), "the unfinished writes are gone");
}

fn vaults_are_isolated<L: LexicalIndex>(l: &L, a: &VaultId, b: &VaultId) {
    live(l, a, &[up("in-a", "word")], Some(1));
    live(l, b, &[up("in-b", "word")], Some(2));
    assert_eq!(ids(l, a, "word"), ["in-a"]);
    assert_eq!(ids(l, b, "word"), ["in-b"]);
}

fn deleting_a_vault_removes_it<L: LexicalIndex>(l: &L, v: &VaultId) {
    live(l, v, &[up("a", "word")], Some(1));
    l.delete_vault(v).expect("delete");
    assert_eq!(l.health(v, true).expect("health"), IndexHealth::Missing);
    assert!(matches!(l.search(v, &query("word")), Err(LexicalError::Unavailable(_))));
    l.delete_vault(v).expect("deleting twice is fine");
}
