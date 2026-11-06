//! What every VectorIndex must do. `prefix` keeps vault names apart on a shared backend.
use crate::VectorIndex;
use memex_types::{DocKind, Generation, LexicalFilter, ModelInfo, ObjectId, VaultId, VectorDoc, VectorError, VectorHealth, VectorOp, VectorQuery};

fn model(name: &str, dimension: u32) -> ModelInfo {
    ModelInfo { name: name.into(), dimension }
}

fn unit(i: usize, dimension: usize) -> Vec<f32> {
    let mut v = vec![0.0; dimension];
    v[i % dimension] = 1.0;
    v
}

fn doc(id: &str, vector: Vec<f32>) -> VectorDoc {
    VectorDoc {
        id: ObjectId(id.into()),
        kind: DocKind::Segment,
        source_ids: vec![ObjectId("src".into())],
        epistemic: None,
        valid_from_ms: None,
        valid_to_ms: None,
        doc_type: None,
        trust_tiers: vec![],
        vector,
    }
}

fn put(d: VectorDoc) -> VectorOp {
    VectorOp::Upsert(d)
}

fn query(name: &str, vector: Vec<f32>) -> VectorQuery {
    VectorQuery {
        model: name.into(),
        vector,
        filter: LexicalFilter::default(),
        limit: 10,
    }
}

fn ids<V: VectorIndex>(index: &V, vault: &VaultId, q: &VectorQuery) -> Vec<String> {
    index.search(vault, q).expect("search").hits.into_iter().map(|h| h.id.0).collect()
}

/// A live generation of model `m` holding `docs`, at watermark `seq`.
fn live<V: VectorIndex>(index: &V, vault: &VaultId, m: &ModelInfo, docs: Vec<VectorDoc>, seq: u64) {
    index.begin_build(vault, m).expect("begin");
    let ops: Vec<VectorOp> = docs.into_iter().map(put).collect();
    index.apply(vault, Generation::Building, &ops, Some(seq)).expect("apply");
    index.flip(vault).expect("flip");
}

pub fn a_new_vault_has_nothing<V: VectorIndex>(index: &V, vault: &VaultId) {
    assert_eq!(index.health(vault).expect("health"), VectorHealth::Missing);
    assert_eq!(index.watermark(vault, Generation::Live).expect("watermark"), None);
    assert!(matches!(index.search(vault, &query("m", vec![1.0, 0.0])), Err(VectorError::Unavailable(_))));
}

pub fn building_is_invisible_until_the_flip<V: VectorIndex>(index: &V, vault: &VaultId) {
    let m = model("m", 3);
    index.begin_build(vault, &m).expect("begin");
    index.apply(vault, Generation::Building, &[put(doc("a", unit(0, 3)))], Some(4)).expect("apply");
    assert_eq!(index.health(vault).expect("health"), VectorHealth::Missing);
    assert_eq!(index.watermark(vault, Generation::Building).expect("watermark"), Some(4));
    index.flip(vault).expect("flip");
    assert_eq!(index.health(vault).expect("health"), VectorHealth::Ready { model: m, watermark: Some(4) });
    assert_eq!(ids(index, vault, &query("m", unit(0, 3))), ["a"]);
}

pub fn nearest_comes_first_and_ties_go_by_id<V: VectorIndex>(index: &V, vault: &VaultId) {
    let m = model("m", 3);
    live(
        index,
        vault,
        &m,
        vec![
            doc("far", vec![0.0, 1.0, 0.0]),
            doc("near", vec![1.0, 0.0, 0.0]),
            doc("mid", vec![0.8, 0.6, 0.0]),
        ],
        1,
    );
    assert_eq!(ids(index, vault, &query("m", vec![1.0, 0.0, 0.0])), ["near", "mid", "far"]);
    let hits = index.search(vault, &query("m", vec![1.0, 0.0, 0.0])).expect("search").hits;
    assert!((hits[0].score - 1.0).abs() < 0.01, "a score is the inner product, and got {}", hits[0].score);
    assert!(hits[0].score > hits[1].score);
    let mut limited = query("m", vec![1.0, 0.0, 0.0]);
    limited.limit = 2;
    assert_eq!(ids(index, vault, &limited), ["near", "mid"]);

    index
        .apply(
            vault,
            Generation::Live,
            &[put(doc("t2", vec![0.0, 0.0, 1.0])), put(doc("t1", vec![0.0, 0.0, 1.0]))],
            Some(2),
        )
        .expect("apply");
    let mut ties = query("m", vec![0.0, 0.0, 1.0]);
    ties.limit = 2;
    assert_eq!(ids(index, vault, &ties), ["t1", "t2"]);
}

pub fn writes_are_idempotent_and_deletes_are_quiet<V: VectorIndex>(index: &V, vault: &VaultId) {
    let m = model("m", 2);
    live(index, vault, &m, vec![doc("a", vec![1.0, 0.0])], 1);
    let again = [put(doc("a", vec![0.0, 1.0])), put(doc("b", vec![1.0, 0.0]))];
    index.apply(vault, Generation::Live, &again, Some(2)).expect("apply");
    index.apply(vault, Generation::Live, &again, Some(2)).expect("apply again");
    assert_eq!(
        ids(index, vault, &query("m", vec![1.0, 0.0])),
        ["b", "a"],
        "the second version of a replaced the first"
    );
    let gone = [VectorOp::Delete(ObjectId("a".into())), VectorOp::Delete(ObjectId("never".into()))];
    index.apply(vault, Generation::Live, &gone, Some(3)).expect("delete");
    assert_eq!(ids(index, vault, &query("m", vec![1.0, 0.0])), ["b"]);
    assert_eq!(index.watermark(vault, Generation::Live).expect("watermark"), Some(3));
}

pub fn a_failed_apply_changes_nothing<V: VectorIndex>(index: &V, vault: &VaultId) {
    let m = model("m", 2);
    live(index, vault, &m, vec![doc("a", vec![1.0, 0.0])], 1);
    let bad = [put(doc("b", vec![1.0, 0.0])), put(doc("c", vec![1.0, 0.0, 0.0]))];
    assert_eq!(
        index.apply(vault, Generation::Live, &bad, Some(9)),
        Err(VectorError::WrongDimension { expected: 2, got: 3 })
    );
    assert_eq!(ids(index, vault, &query("m", vec![1.0, 0.0])), ["a"]);
    assert_eq!(
        index.watermark(vault, Generation::Live).expect("watermark"),
        Some(1),
        "the watermark moves with the vectors or not at all"
    );
}

pub fn filters_mean_what_they_say<V: VectorIndex>(index: &V, vault: &VaultId) {
    let m = model("m", 2);
    let mut claim = doc("claim", vec![1.0, 0.0]);
    claim.kind = DocKind::Claim;
    claim.source_ids = vec![ObjectId("s1".into()), ObjectId("s2".into())];
    claim.epistemic = Some("verified".into());
    claim.valid_from_ms = Some(100);
    claim.valid_to_ms = Some(200);
    claim.doc_type = Some("weighs".into());
    claim.trust_tiers = vec!["trusted".into(), "reviewed".into()];
    let mut segment = doc("segment", vec![0.9, 0.1]);
    segment.source_ids = vec![ObjectId("s3".into())];
    live(index, vault, &m, vec![claim, segment], 1);
    let search = |filter: LexicalFilter| {
        let mut q = query("m", vec![1.0, 0.0]);
        q.filter = filter;
        ids(index, vault, &q)
    };
    assert_eq!(search(LexicalFilter::default()), ["claim", "segment"]);
    let by_kind = LexicalFilter {
        kinds: vec![DocKind::Segment],
        ..Default::default()
    };
    assert_eq!(search(by_kind), ["segment"]);
    let by_source = LexicalFilter {
        source_ids: vec![ObjectId("s2".into()), ObjectId("nope".into())],
        ..Default::default()
    };
    assert_eq!(search(by_source), ["claim"]);
    let hostile = LexicalFilter {
        source_ids: vec![ObjectId("x'] ) OR true; --".into())],
        ..Default::default()
    };
    assert!(search(hostile).is_empty(), "a quote in a filter is only text");
    let by_epistemic = LexicalFilter {
        epistemic: vec!["verified".into()],
        ..Default::default()
    };
    assert_eq!(search(by_epistemic), ["claim"]);
    let by_type = LexicalFilter {
        types: vec!["weighs".into()],
        ..Default::default()
    };
    assert_eq!(search(by_type), ["claim"]);
    let by_tier = LexicalFilter {
        trust_tiers: vec!["reviewed".into(), "nope".into()],
        ..Default::default()
    };
    assert_eq!(search(by_tier), ["claim"], "any of a claim's tiers counts");
    let at = |t| LexicalFilter {
        valid_at_ms: Some(t),
        ..Default::default()
    };
    assert_eq!(search(at(99)), ["segment"], "before it's valid");
    assert_eq!(search(at(100)), ["claim", "segment"], "the start is included");
    assert_eq!(search(at(200)), ["segment"], "the end isn't");
}

pub fn a_query_can_t_mix_models<V: VectorIndex>(index: &V, vault: &VaultId) {
    live(index, vault, &model("old", 2), vec![doc("a", vec![1.0, 0.0])], 1);
    assert_eq!(
        index.search(vault, &query("new", vec![1.0, 0.0])),
        Err(VectorError::ModelMismatch {
            live: "old".into(),
            asked: "new".into()
        })
    );
    assert_eq!(
        index.search(vault, &query("old", vec![1.0, 0.0, 0.0])),
        Err(VectorError::WrongDimension { expected: 2, got: 3 })
    );
    // A new model can have another dimension, and the old one answers until the flip.
    let new = model("new", 3);
    index.begin_build(vault, &new).expect("begin");
    index
        .apply(vault, Generation::Building, &[put(doc("b", vec![1.0, 0.0, 0.0]))], Some(2))
        .expect("apply");
    assert_eq!(ids(index, vault, &query("old", vec![1.0, 0.0])), ["a"]);
    index
        .apply(vault, Generation::Live, &[put(doc("a2", vec![1.0, 0.0]))], Some(2))
        .expect("old still follows");
    index.flip(vault).expect("flip");
    assert_eq!(ids(index, vault, &query("new", vec![1.0, 0.0, 0.0])), ["b"]);
    assert!(matches!(
        index.search(vault, &query("old", vec![1.0, 0.0])),
        Err(VectorError::ModelMismatch { .. })
    ));
}

pub fn rollback_returns_to_the_last_generation<V: VectorIndex>(index: &V, vault: &VaultId) {
    assert!(matches!(index.rollback(vault), Err(VectorError::Invalid(_))), "nothing to go back to");
    assert!(matches!(index.flip(vault), Err(VectorError::Invalid(_))), "nothing to flip to");
    let m = model("m", 2);
    live(index, vault, &m, vec![doc("first", vec![1.0, 0.0])], 1);
    live(index, vault, &m, vec![doc("second", vec![1.0, 0.0])], 2);
    assert_eq!(ids(index, vault, &query("m", vec![1.0, 0.0])), ["second"]);
    index.rollback(vault).expect("rollback");
    assert_eq!(ids(index, vault, &query("m", vec![1.0, 0.0])), ["first"]);
    assert_eq!(index.watermark(vault, Generation::Live).expect("watermark"), Some(1));
}

pub fn an_unfinished_build_is_discarded<V: VectorIndex>(index: &V, vault: &VaultId) {
    let m = model("m", 2);
    index.begin_build(vault, &m).expect("begin");
    index
        .apply(vault, Generation::Building, &[put(doc("stale", vec![1.0, 0.0]))], Some(1))
        .expect("apply");
    index.begin_build(vault, &m).expect("begin again");
    assert_eq!(index.watermark(vault, Generation::Building).expect("watermark"), None);
    index.flip(vault).expect("flip");
    assert!(ids(index, vault, &query("m", vec![1.0, 0.0])).is_empty());
    assert!(matches!(index.apply(vault, Generation::Building, &[], Some(1)), Err(VectorError::Invalid(_))));
}

pub fn embeddings_are_stored_per_model<V: VectorIndex>(index: &V, vault: &VaultId) {
    assert!(index.embeddings(vault, "m", &["h1".into()]).expect("read").is_empty());
    let rows = [("h1".to_string(), vec![0.5, 0.25]), ("h2".to_string(), vec![0.25, 0.5])];
    index.store_embeddings(vault, "m", &rows).expect("store");
    index.store_embeddings(vault, "m", &rows).expect("store again");
    let got = index.embeddings(vault, "m", &["h1".into(), "h2".into(), "h3".into()]).expect("read");
    assert_eq!(got.len(), 2);
    assert_eq!(got["h1"], vec![0.5, 0.25]);
    assert!(index.embeddings(vault, "other", &["h1".into()]).expect("read").is_empty());
}

pub fn vaults_are_separate<V: VectorIndex>(index: &V, a: &VaultId, b: &VaultId) {
    let m = model("m", 2);
    live(index, a, &m, vec![doc("in-a", vec![1.0, 0.0])], 1);
    index.store_embeddings(a, "m", &[("h".to_string(), vec![1.0, 0.0])]).expect("store");
    assert_eq!(index.health(b).expect("health"), VectorHealth::Missing);
    assert!(index.embeddings(b, "m", &["h".into()]).expect("read").is_empty());
    live(index, b, &m, vec![doc("in-b", vec![1.0, 0.0])], 1);
    assert_eq!(ids(index, a, &query("m", vec![1.0, 0.0])), ["in-a"]);
    assert_eq!(ids(index, b, &query("m", vec![1.0, 0.0])), ["in-b"]);
}

/// Runs every case, each in its own vault. `open` prepares a vault, which a store-backed index needs.
pub fn run_all<V: VectorIndex>(index: &V, prefix: &str, open: &dyn Fn(&VaultId)) {
    let vault = |name: &str| {
        let v = VaultId(format!("{prefix}{name}"));
        open(&v);
        v
    };
    a_new_vault_has_nothing(index, &vault("new"));
    building_is_invisible_until_the_flip(index, &vault("building"));
    nearest_comes_first_and_ties_go_by_id(index, &vault("nearest"));
    writes_are_idempotent_and_deletes_are_quiet(index, &vault("idempotent"));
    a_failed_apply_changes_nothing(index, &vault("atomic"));
    filters_mean_what_they_say(index, &vault("filters"));
    a_query_can_t_mix_models(index, &vault("models"));
    rollback_returns_to_the_last_generation(index, &vault("rollback"));
    an_unfinished_build_is_discarded(index, &vault("unfinished"));
    embeddings_are_stored_per_model(index, &vault("embeddings"));
    vaults_are_separate(index, &vault("a"), &vault("b"));
}
