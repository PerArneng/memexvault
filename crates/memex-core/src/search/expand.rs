//! k-hop expansion: follows Edges from a search's best candidates to what they link to, for questions that a page's neighbour answers. Spec 022 fixed these numbers before measuring them.
use super::fusion::Fused;
use crate::{Snapshot, StoreError};
use memex_types::{BlockRow, CLAIM_REF, DocKind, EdgeDirection, EdgeRow, ObjectId, Row, RowKey, Table, WIKI_LINK, slug_of_target, slug_target};
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub const HOPS: usize = 1;
pub const SEEDS: usize = 5;
pub const NEIGHBOURS_PER_SEED: usize = 8;
pub const MAX_ADDED: usize = 10;
pub const HOP_DECAY: f32 = 0.5;
/// Edges read per hop, so a page everything links to costs what any other does.
const EDGE_LIMIT: usize = 400;

fn key(table: Table, id: &ObjectId) -> RowKey {
    RowKey { table, id: id.clone() }
}

/// What's reached so far, with the score it carries, best first by its seed.
#[derive(Debug, Clone, PartialEq)]
struct Reached {
    page: ObjectId,
    score: f32,
}

/// A page's neighbours through these edges, in edge order. Slugs resolve through the map, and a broken link leads nowhere.
fn neighbours_of(page: &ObjectId, slugs_of_page: &[String], edges: &[EdgeRow], holders: &HashMap<String, ObjectId>) -> Vec<(ObjectId, Option<ObjectId>)> {
    let mine: BTreeSet<ObjectId> = slugs_of_page.iter().map(|s| slug_target(s)).collect();
    let mut out: Vec<(ObjectId, Option<ObjectId>)> = vec![];
    for e in edges.iter().filter(|e| e.kind == WIKI_LINK) {
        let next = if e.from == *page {
            slug_of_target(&e.to).and_then(|s| holders.get(s)).map(|p| (p.clone(), None))
        } else if mine.contains(&e.to) {
            // Coming the other way, the block holding the link is the one that speaks of this page.
            Some((e.from.clone(), e.block_id.clone()))
        } else {
            None
        };
        if let Some((p, block)) = next
            && p != *page
            && !out.iter().any(|(q, _)| *q == p)
        {
            out.push((p, block));
        }
    }
    out
}

/// Adds what the best candidates link to, at `hops` hops or fewer, and returns the new ranking and how many it added. It reads rows only for pages it reaches, and a set of visited pages means a cycle of links just ends. The fence checks everything added, like any other candidate.
pub fn expand<S: Snapshot>(snap: &S, mut fused: Vec<Fused>, hops: usize) -> Result<(Vec<Fused>, usize), StoreError> {
    let seeds: Vec<(ObjectId, DocKind, f32)> = fused.iter().take(SEEDS).map(|f| (f.id.clone(), f.kind, f.score)).collect();
    let keys: Vec<RowKey> = seeds
        .iter()
        .filter_map(|(id, kind, _)| match kind {
            DocKind::Block => Some(key(Table::Block, id)),
            _ => None,
        })
        .collect();
    let mut block_page: HashMap<ObjectId, ObjectId> = HashMap::new();
    if !keys.is_empty() {
        for row in snap.get_many(&keys)?.into_iter().flatten() {
            if let Row::Block(b) = row {
                block_page.insert(b.header.id, b.page_id);
            }
        }
    }
    let mut frontier: Vec<Reached> = vec![];
    let mut claims: Vec<(ObjectId, f32)> = vec![];
    for (id, kind, score) in &seeds {
        match kind {
            DocKind::Page => frontier.push(Reached {
                page: id.clone(),
                score: *score,
            }),
            DocKind::Block => {
                if let Some(page) = block_page.get(id) {
                    frontier.push(Reached {
                        page: page.clone(),
                        score: *score,
                    });
                }
            }
            DocKind::Claim => claims.push((id.clone(), *score)),
            DocKind::Segment | DocKind::Entity => {}
        }
    }
    if frontier.is_empty() && claims.is_empty() {
        return Ok((fused, 0));
    }
    // (kind, id) to its added score, and the blocks standing in for pages reached.
    let mut gained: BTreeMap<(ObjectId, u8), f32> = BTreeMap::new();
    let mut reached_pages: BTreeMap<ObjectId, (f32, Option<ObjectId>)> = BTreeMap::new();
    let mut visited: BTreeSet<ObjectId> = frontier.iter().map(|r| r.page.clone()).collect();
    if !claims.is_empty() {
        let ids: Vec<ObjectId> = claims.iter().map(|c| c.0.clone()).collect();
        for e in snap.edges_linked(&ids, &[CLAIM_REF], EdgeDirection::To, None, EDGE_LIMIT)? {
            let Some(score) = claims.iter().find(|c| c.0 == e.to).map(|c| c.1) else {
                continue;
            };
            let entry = reached_pages.entry(e.from.clone()).or_insert((0.0, e.block_id.clone()));
            entry.0 = entry.0.max(score * HOP_DECAY);
            visited.insert(e.from);
        }
    }
    let mut decay = HOP_DECAY;
    for _ in 0..hops {
        if frontier.is_empty() {
            break;
        }
        let pages: Vec<ObjectId> = frontier.iter().map(|r| r.page.clone()).collect::<BTreeSet<_>>().into_iter().collect();
        let keys: Vec<RowKey> = pages.iter().map(|p| key(Table::Page, p)).collect();
        let mut slugs: HashMap<ObjectId, Vec<String>> = HashMap::new();
        for row in snap.get_many(&keys)?.into_iter().flatten() {
            if let Row::Page(p) = row {
                let held = p
                    .redirects
                    .iter()
                    .cloned()
                    .chain(std::iter::once(p.slug.clone()))
                    .filter(|s| !s.is_empty())
                    .collect();
                slugs.insert(p.header.id, held);
            }
        }
        let mut ends: Vec<ObjectId> = pages.clone();
        ends.extend(slugs.values().flatten().map(|s| slug_target(s)));
        let edges = snap.edges_linked(&ends, &[WIKI_LINK, CLAIM_REF], EdgeDirection::Both, None, EDGE_LIMIT)?;
        let targets: Vec<RowKey> = edges
            .iter()
            .filter_map(|e| slug_of_target(&e.to).map(|s| key(Table::Slug, &ObjectId(s.to_string()))))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut holders: HashMap<String, ObjectId> = HashMap::new();
        if !targets.is_empty() {
            for row in snap.get_many(&targets)?.into_iter().flatten() {
                if let Row::Slug(s) = row {
                    holders.insert(s.header.id.0, s.page_id);
                }
            }
        }
        let mut next = vec![];
        for r in &frontier {
            let held = slugs.get(&r.page).map_or(&[][..], Vec::as_slice);
            let hop_score = r.score * decay;
            for (page, block) in neighbours_of(&r.page, held, &edges, &holders).into_iter().take(NEIGHBOURS_PER_SEED) {
                let entry = reached_pages.entry(page.clone()).or_insert((0.0, block));
                entry.0 = entry.0.max(hop_score);
                if visited.insert(page.clone()) {
                    next.push(Reached { page, score: r.score });
                }
            }
            for e in edges.iter().filter(|e| e.kind == CLAIM_REF && e.from == r.page) {
                let slot = gained.entry((e.to.clone(), 1)).or_insert(0.0);
                *slot = slot.max(hop_score);
            }
        }
        frontier = next;
        decay *= HOP_DECAY;
    }
    if !reached_pages.is_empty() {
        let pages: Vec<ObjectId> = reached_pages.keys().cloned().collect();
        // Each page's first block with words stands for it, unless a linking block already does.
        let want = pages.len().saturating_mul(4).min(EDGE_LIMIT);
        let blocks: Vec<BlockRow> = snap.blocks_of(&pages, want)?;
        for (page, (score, block)) in &reached_pages {
            let chosen = block.clone().or_else(|| {
                blocks
                    .iter()
                    .find(|b| b.page_id == *page && crate::page::block_doc(b).is_some())
                    .map(|b| b.header.id.clone())
            });
            match chosen {
                Some(b) => {
                    let slot = gained.entry((b, 0)).or_insert(0.0);
                    *slot = slot.max(*score);
                }
                None => {
                    let slot = gained.entry((page.clone(), 2)).or_insert(0.0);
                    *slot = slot.max(*score);
                }
            }
        }
    }
    let mut ranked: Vec<(ObjectId, DocKind, f32)> = gained
        .into_iter()
        .map(|((id, kind), score)| {
            let kind = match kind {
                0 => DocKind::Block,
                1 => DocKind::Claim,
                _ => DocKind::Page,
            };
            (id, kind, score)
        })
        .collect();
    ranked.sort_by(|a, b| b.2.total_cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
    let mut added = 0;
    for (id, kind, score) in ranked {
        if let Some(f) = fused.iter_mut().find(|f| f.id == id && f.kind == kind) {
            f.score += score;
        } else if added < MAX_ADDED {
            added += 1;
            fused.push(Fused {
                id,
                kind,
                score,
                lexical_rank: None,
                lexical_score: None,
                vector_rank: None,
                vector_score: None,
            });
        }
    }
    fused.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
    Ok((fused, added))
}
