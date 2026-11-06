//! Reciprocal Rank Fusion: a hit's score is the sum of `weight / (k + rank)` over the signals that found it. It reads ranks only, so BM25 and cosine scores never need comparing.
use memex_types::{DocKind, LexicalHit, ObjectId};
use std::collections::HashMap;

pub const RRF_K: f32 = 60.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Fused {
    pub id: ObjectId,
    pub kind: DocKind,
    pub score: f32,
    /// 1 is best.
    pub lexical_rank: Option<usize>,
    pub lexical_score: Option<f32>,
    pub vector_rank: Option<usize>,
    pub vector_score: Option<f32>,
}

/// Best first, with ties in id order, so a ranking repeats exactly.
pub fn fuse(lexical: &[LexicalHit], vector: &[LexicalHit], lexical_weight: f32, vector_weight: f32, k: f32) -> Vec<Fused> {
    let mut by_id: HashMap<&ObjectId, Fused> = HashMap::with_capacity(lexical.len() + vector.len());
    for (i, hit) in lexical.iter().enumerate() {
        let f = by_id.entry(&hit.id).or_insert_with(|| empty(hit));
        f.score += lexical_weight / (k + (i + 1) as f32);
        f.lexical_rank = Some(i + 1);
        f.lexical_score = Some(hit.score);
    }
    for (i, hit) in vector.iter().enumerate() {
        let f = by_id.entry(&hit.id).or_insert_with(|| empty(hit));
        f.score += vector_weight / (k + (i + 1) as f32);
        f.vector_rank = Some(i + 1);
        f.vector_score = Some(hit.score);
    }
    let mut out: Vec<Fused> = by_id.into_values().collect();
    out.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
    out
}

fn empty(hit: &LexicalHit) -> Fused {
    Fused {
        id: hit.id.clone(),
        kind: hit.kind,
        score: 0.0,
        lexical_rank: None,
        lexical_score: None,
        vector_rank: None,
        vector_score: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hits(ids: &[&str]) -> Vec<LexicalHit> {
        ids.iter()
            .enumerate()
            .map(|(i, id)| LexicalHit {
                id: ObjectId((*id).into()),
                kind: DocKind::Segment,
                score: 100.0 - i as f32,
            })
            .collect()
    }

    fn order(f: &[Fused]) -> Vec<&str> {
        f.iter().map(|x| x.id.0.as_str()).collect()
    }

    #[test]
    fn found_by_both_beats_first_in_one() {
        let f = fuse(&hits(&["a", "b"]), &hits(&["c", "b"]), 1.0, 1.0, RRF_K);
        assert_eq!(order(&f), ["b", "a", "c"]);
        let b = &f[0];
        assert_eq!((b.lexical_rank, b.vector_rank), (Some(2), Some(2)));
        assert!((b.score - 2.0 / 62.0).abs() < 1e-6);
    }

    #[test]
    fn scores_are_the_rrf_formula_and_ignore_signal_scale() {
        let mut lexical = hits(&["a"]);
        lexical[0].score = 1e9;
        let f = fuse(&lexical, &hits(&["b"]), 1.0, 1.0, RRF_K);
        assert!((f[0].score - f[1].score).abs() < 1e-9, "only ranks count");
        assert_eq!(order(&f), ["a", "b"], "a tie goes in id order");
    }

    #[test]
    fn a_weight_favours_its_signal() {
        let f = fuse(&hits(&["a"]), &hits(&["b"]), 2.0, 1.0, RRF_K);
        assert_eq!(order(&f), ["a", "b"]);
        let f = fuse(&hits(&["z"]), &hits(&["b"]), 2.0, 1.0, RRF_K);
        assert_eq!(order(&f), ["z", "b"]);
        let f = fuse(&hits(&["z"]), &hits(&["b"]), 1.0, 2.0, RRF_K);
        assert_eq!(order(&f), ["b", "z"]);
    }

    #[test]
    fn one_signal_keeps_its_order_and_none_gives_nothing() {
        assert_eq!(order(&fuse(&hits(&["c", "a", "b"]), &[], 1.0, 1.0, RRF_K)), ["c", "a", "b"]);
        assert_eq!(order(&fuse(&[], &hits(&["c", "a"]), 1.0, 1.0, RRF_K)), ["c", "a"]);
        assert!(fuse(&[], &[], 1.0, 1.0, RRF_K).is_empty());
    }

    #[test]
    fn signal_details_survive() {
        let f = fuse(&hits(&["a"]), &[], 1.0, 1.0, RRF_K);
        assert_eq!((f[0].lexical_score, f[0].vector_score, f[0].vector_rank), (Some(100.0), None, None));
    }
}
