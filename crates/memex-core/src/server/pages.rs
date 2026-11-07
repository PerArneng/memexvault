//! Wiki pages' use cases on the facade. Every write here builds ops and goes through `propose`, so it gets the same validators, risk table and review queue as anything else.
use super::entities::claims_behind;
use super::{Caller, Operation, Proposal, ProposalOutcome, Server, ServerEdges, ServerError};
use crate::page::{self, BlockChange, PageDocument};
use crate::{EntityClaim, Snapshot, Store};
use memex_types::{
    AssertionRow, BlockRow, CLAIM_REF, EdgeDirection, EdgeRow, ObjectId, Op, PageRow, RevisionBlock, RevisionRow, Row, RowKey, Status, Table, VaultId, WIKI_LINK,
    revision_id, slug_of_target, slug_target,
};
use std::collections::{BTreeSet, HashMap};

/// How far a chain of superseded claims is followed. A longer one can only be a corrupt row.
const MAX_SUPERSEDED: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPage {
    pub title: String,
    pub slug: Option<String>,
    pub body: String,
    pub rationale: String,
    pub idempotency_key: String,
}

/// A page's words, as they are or as they were.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageRevisionView {
    pub revision: u32,
    pub title: String,
    pub slug: String,
    pub body: String,
    pub blocks: Vec<RevisionBlock>,
    pub updated_at_ms: i64,
    pub creator: String,
    pub restored_from: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimRefStatus {
    /// The claim it names is current.
    Current,
    /// The claim it names was replaced, and `current` is what replaced it.
    Superseded,
    /// There's no such claim, or the one replacing it has gone too.
    Missing,
}

/// What a `{{claim:<id>}}` shows now. It's read at each request, so a fact never drifts between pages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimReference {
    pub claim_id: ObjectId,
    pub status: ClaimRefStatus,
    pub current: Option<EntityClaim>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageView {
    pub page_id: ObjectId,
    /// The current version, which an edit bases itself on. It differs from `content.revision` on an old revision.
    pub current_revision: u32,
    pub current_slug: String,
    pub redirects: Vec<String>,
    pub content: PageRevisionView,
    pub claims: Vec<ClaimReference>,
}

/// Where a slug leads. `redirected` means it's an old slug.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageRedirect {
    pub slug: String,
    pub page_id: ObjectId,
    pub current_slug: String,
    pub redirected: bool,
}

/// A page linking to this one, and the block that does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BacklinkView {
    pub edge_id: ObjectId,
    pub from_page: ObjectId,
    pub from_title: Option<String>,
    pub from_slug: Option<String>,
    pub block_id: Option<ObjectId>,
    /// The slug as the link wrote it, which may be an old one.
    pub via_slug: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingLink {
    pub edge_id: ObjectId,
    /// `wiki_link` or `claim_ref`.
    pub kind: String,
    pub block_id: Option<ObjectId>,
    /// The slug or claim id as written.
    pub target: String,
    /// The page a wiki link reaches. None if it's broken, and always on a claim reference.
    pub to_page: Option<ObjectId>,
    /// Whether the target exists.
    pub resolved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedLink {
    pub edge_id: ObjectId,
    pub kind: String,
    pub from_page: ObjectId,
    pub block_id: Option<ObjectId>,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageChange {
    pub block_id: ObjectId,
    pub change: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageDiff {
    pub from: u32,
    pub to: u32,
    pub title: Option<(String, String)>,
    pub slug: Option<(String, String)>,
    pub blocks: Vec<PageChange>,
}

/// One link at an object, seen from that object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeighborView {
    pub edge_id: ObjectId,
    pub kind: String,
    /// `out` when the object is where the link starts.
    pub direction: &'static str,
    pub from: ObjectId,
    pub to: ObjectId,
    pub block_id: Option<ObjectId>,
    /// What's at the other end. None is a link to a slug no page holds.
    pub neighbor: Option<ObjectId>,
    /// The other end's title, when it's a page.
    pub title: Option<String>,
}

/// The page an import made or changed, and what became of the proposal.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedPage {
    pub page_id: ObjectId,
    pub created: bool,
    pub outcome: ProposalOutcome,
}

fn key(table: Table, id: &ObjectId) -> RowKey {
    RowKey { table, id: id.clone() }
}

fn page_row<S: Snapshot>(snap: &S, id: &ObjectId) -> Result<PageRow, ServerError> {
    match snap.get(&key(Table::Page, id))? {
        Some(Row::Page(p)) => Ok(p),
        _ => Err(ServerError::NotFound),
    }
}

fn revision_row<S: Snapshot>(snap: &S, id: &ObjectId, revision: u32) -> Result<RevisionRow, ServerError> {
    match snap.get(&key(Table::Revision, &revision_id(id, revision)))? {
        Some(Row::Revision(r)) => Ok(r),
        _ => Err(ServerError::NotFound),
    }
}

fn slugs_of(page: &PageRow) -> Vec<ObjectId> {
    page.redirects
        .iter()
        .chain(std::iter::once(&page.slug))
        .filter(|s| !s.is_empty())
        .map(|s| slug_target(s))
        .collect()
}

fn view_of_revision(r: RevisionRow, creator: String) -> PageRevisionView {
    PageRevisionView {
        revision: r.revision,
        body: page::render_markdown(r.blocks.iter().map(|b| b.text.as_str())),
        title: r.title,
        slug: r.slug,
        blocks: r.blocks,
        updated_at_ms: r.created_at_ms,
        creator,
        restored_from: r.restored_from,
    }
}

/// A page from before spec 022 has no revisions yet, so what it holds now stands in for its current one.
fn view_of_page(p: &PageRow, blocks: Vec<BlockRow>) -> PageRevisionView {
    PageRevisionView {
        revision: p.header.version,
        title: p.title.clone(),
        slug: p.slug.clone(),
        body: p.body.clone(),
        blocks: blocks
            .into_iter()
            .map(|b| RevisionBlock {
                id: b.header.id,
                kind: b.kind,
                text: b.text,
            })
            .collect(),
        updated_at_ms: p.updated_at_ms,
        creator: p.header.creator.clone(),
        restored_from: None,
    }
}

/// Follows each claim to what stands now, at one read per step of the longest chain.
fn claim_references<S: Snapshot>(snap: &S, ids: &[ObjectId]) -> Result<Vec<ClaimReference>, ServerError> {
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let mut seen: HashMap<ObjectId, AssertionRow> = HashMap::new();
    let mut wanted: BTreeSet<ObjectId> = ids.iter().cloned().collect();
    for _ in 0..MAX_SUPERSEDED {
        if wanted.is_empty() {
            break;
        }
        let keys: Vec<RowKey> = wanted.iter().map(|id| key(Table::Assertion, id)).collect();
        let mut next = BTreeSet::new();
        for row in snap.get_many(&keys)?.into_iter().flatten() {
            if let Row::Assertion(a) = row {
                if let Some(by) = &a.superseded_by
                    && !seen.contains_key(by)
                {
                    next.insert(by.clone());
                }
                seen.insert(a.header.id.clone(), a);
            }
        }
        wanted = next;
    }
    let mut ends: Vec<(ObjectId, Option<ObjectId>)> = vec![];
    for id in ids {
        let mut at = id;
        let mut steps = 0;
        let end = loop {
            match seen.get(at) {
                None => break None,
                Some(a) => match &a.superseded_by {
                    // Retired with nothing in its place, so nothing stands for it now.
                    None if a.header.status != Status::Active => break None,
                    None => break Some(a.header.id.clone()),
                    Some(next) if steps < MAX_SUPERSEDED => {
                        at = next;
                        steps += 1;
                    }
                    Some(_) => break None,
                },
            }
        };
        ends.push((id.clone(), end));
    }
    let current: Vec<ObjectId> = ends.iter().filter_map(|e| e.1.clone()).collect::<BTreeSet<_>>().into_iter().collect();
    let claims = claims_behind(snap, &current)?;
    Ok(ends
        .into_iter()
        .map(|(claim_id, end)| {
            let current = end.as_ref().and_then(|e| claims.get(e)).cloned();
            let status = match &end {
                None => ClaimRefStatus::Missing,
                Some(e) if *e == claim_id => ClaimRefStatus::Current,
                Some(_) => ClaimRefStatus::Superseded,
            };
            ClaimReference { claim_id, status, current }
        })
        .collect())
}

fn claims_in(blocks: &[RevisionBlock]) -> Vec<ObjectId> {
    let mut out: Vec<ObjectId> = vec![];
    for b in blocks {
        for c in page::parse_inline(b.kind, &b.text).claims {
            if !out.contains(&c) {
                out.push(c);
            }
        }
    }
    out
}

fn first_op_id(outcome: &ProposalOutcome) -> Option<ObjectId> {
    outcome.changeset.as_ref().and_then(|c| {
        c.ops.iter().find_map(|op| match op {
            Op::CreatePage { id, .. } => Some(id.clone()),
            _ => None,
        })
    })
}

impl<E: ServerEdges> Server<E> {
    fn page_read_access(&self, caller: &Caller, vault: &VaultId) -> Result<(), ServerError> {
        self.allow(caller, Operation::VaultContentRead(vault))?;
        self.vault_exists(vault)
    }

    fn one_op(
        &self,
        caller: &Caller,
        vault: &VaultId,
        op: Op,
        rationale: String,
        idempotency_key: String,
        dry_run: bool,
    ) -> Result<ProposalOutcome, ServerError> {
        self.propose(
            caller,
            vault,
            Proposal {
                ops: vec![op],
                rationale,
                idempotency_key,
            },
            dry_run,
        )
    }

    /// A retry gets the id its first attempt chose. A person's page commits at once, and an agent's waits for review until it has earned auto-commit.
    pub fn propose_new_page(&self, caller: &Caller, vault: &VaultId, new: NewPage, dry_run: bool) -> Result<(ObjectId, ProposalOutcome), ServerError> {
        let id = ObjectId(self.new_id());
        let op = Op::CreatePage {
            id: id.clone(),
            title: new.title,
            body: new.body,
            slug: new.slug,
            redirects: vec![],
        };
        let outcome = self.one_op(caller, vault, op, new.rationale, new.idempotency_key, dry_run)?;
        Ok((first_op_id(&outcome).unwrap_or(id), outcome))
    }

    #[allow(clippy::too_many_arguments)]
    /// A stale `base_version` is a conflict that carries the current row, for the caller to rebase on.
    pub fn propose_page_edit(
        &self,
        caller: &Caller,
        vault: &VaultId,
        id: &ObjectId,
        base_version: u32,
        body: String,
        rationale: String,
        idempotency_key: String,
        dry_run: bool,
    ) -> Result<ProposalOutcome, ServerError> {
        let op = Op::EditPage {
            id: id.clone(),
            base_version,
            body,
        };
        self.one_op(caller, vault, op, rationale, idempotency_key, dry_run)
    }

    #[allow(clippy::too_many_arguments)]
    /// The old slug keeps leading here, so no link needs changing.
    pub fn propose_page_rename(
        &self,
        caller: &Caller,
        vault: &VaultId,
        id: &ObjectId,
        base_version: u32,
        title: String,
        slug: Option<String>,
        rationale: String,
        idempotency_key: String,
    ) -> Result<ProposalOutcome, ServerError> {
        let op = Op::RenamePage {
            id: id.clone(),
            base_version,
            title,
            slug,
        };
        self.one_op(caller, vault, op, rationale, idempotency_key, false)
    }

    /// Always waits for review, for anyone, as every delete does.
    pub fn propose_page_delete(
        &self,
        caller: &Caller,
        vault: &VaultId,
        id: &ObjectId,
        base_version: u32,
        rationale: String,
        idempotency_key: String,
    ) -> Result<ProposalOutcome, ServerError> {
        self.one_op(
            caller,
            vault,
            Op::DeletePage { id: id.clone(), base_version },
            rationale,
            idempotency_key,
            false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    /// Makes an old revision current again, as a new revision, so nothing in the history is lost.
    pub fn propose_page_revert(
        &self,
        caller: &Caller,
        vault: &VaultId,
        id: &ObjectId,
        base_version: u32,
        revision: u32,
        rationale: String,
        idempotency_key: String,
    ) -> Result<ProposalOutcome, ServerError> {
        let op = Op::RevertPage {
            id: id.clone(),
            base_version,
            revision,
        };
        self.one_op(caller, vault, op, rationale, idempotency_key, false)
    }

    /// In id order, without bodies.
    pub fn list_wiki_pages(&self, caller: &Caller, vault: &VaultId, after: Option<&ObjectId>, limit: usize) -> Result<Vec<PageRow>, ServerError> {
        self.page_read_access(caller, vault)?;
        Ok(self.engine.store().snapshot(vault)?.page_summaries(after, limit)?)
    }

    /// The page now, or at a past revision. Claim references show what stands today either way, so an old revision's facts are still marked if they've been replaced.
    pub fn get_wiki_page(&self, caller: &Caller, vault: &VaultId, id: &ObjectId, revision: Option<u32>) -> Result<PageView, ServerError> {
        self.page_read_access(caller, vault)?;
        let snap = self.engine.store().snapshot(vault)?;
        let page = page_row(&snap, id)?;
        let content = match revision {
            Some(n) if n != page.header.version => {
                let r = revision_row(&snap, id, n)?;
                let creator = r.header.creator.clone();
                view_of_revision(r, creator)
            }
            _ => view_of_page(&page, snap.blocks_of(std::slice::from_ref(id), page::MAX_BLOCKS + 1)?),
        };
        let claims = claim_references(&snap, &claims_in(&content.blocks))?;
        Ok(PageView {
            page_id: page.header.id,
            current_revision: page.header.version,
            current_slug: page.slug,
            redirects: page.redirects,
            content,
            claims,
        })
    }

    /// Oldest first, without their blocks. A deleted page's history is still here.
    pub fn list_page_revisions(
        &self,
        caller: &Caller,
        vault: &VaultId,
        id: &ObjectId,
        after: Option<u32>,
        limit: usize,
    ) -> Result<Vec<RevisionRow>, ServerError> {
        self.page_read_access(caller, vault)?;
        let rows = self.engine.store().snapshot(vault)?.revisions_of(id, after, limit)?;
        if rows.is_empty() && after.is_none() {
            return Err(ServerError::NotFound);
        }
        Ok(rows)
    }

    pub fn page_diff(&self, caller: &Caller, vault: &VaultId, id: &ObjectId, from: u32, to: u32) -> Result<PageDiff, ServerError> {
        self.page_read_access(caller, vault)?;
        let snap = self.engine.store().snapshot(vault)?;
        let (a, b) = (revision_row(&snap, id, from)?, revision_row(&snap, id, to)?);
        let pair = |x: &str, y: &str| (x != y).then(|| (x.to_string(), y.to_string()));
        Ok(PageDiff {
            from,
            to,
            title: pair(&a.title, &b.title),
            slug: pair(&a.slug, &b.slug),
            blocks: page::diff_revisions(&a, &b)
                .into_iter()
                .map(|c: BlockChange| PageChange {
                    block_id: c.block_id,
                    change: c.change.to_string(),
                    before: c.before,
                    after: c.after,
                })
                .collect(),
        })
    }

    /// What links here, through any slug the page has ever had, in id order.
    pub fn page_backlinks(
        &self,
        caller: &Caller,
        vault: &VaultId,
        id: &ObjectId,
        after: Option<&ObjectId>,
        limit: usize,
    ) -> Result<Vec<BacklinkView>, ServerError> {
        self.page_read_access(caller, vault)?;
        let snap = self.engine.store().snapshot(vault)?;
        let page = page_row(&snap, id)?;
        let edges = snap.edges_linked(&slugs_of(&page), &[WIKI_LINK], EdgeDirection::To, after, limit)?;
        let keys: Vec<RowKey> = edges
            .iter()
            .map(|e| e.from.clone())
            .collect::<BTreeSet<_>>()
            .iter()
            .map(|p| key(Table::Page, p))
            .collect();
        let mut titles: HashMap<ObjectId, (String, String)> = HashMap::new();
        if !keys.is_empty() {
            for row in snap.get_many(&keys)?.into_iter().flatten() {
                if let Row::Page(p) = row {
                    titles.insert(p.header.id, (p.title, p.slug));
                }
            }
        }
        Ok(edges
            .into_iter()
            .map(|e| {
                let found = titles.get(&e.from);
                BacklinkView {
                    edge_id: e.header.id.clone(),
                    from_title: found.map(|t| t.0.clone()),
                    from_slug: found.map(|t| t.1.clone()),
                    via_slug: slug_of_target(&e.to).unwrap_or_default().to_string(),
                    from_page: e.from,
                    block_id: e.block_id,
                }
            })
            .collect())
    }

    /// What this page links to, each saying whether it leads anywhere. In id order.
    pub fn page_links(
        &self,
        caller: &Caller,
        vault: &VaultId,
        id: &ObjectId,
        after: Option<&ObjectId>,
        limit: usize,
    ) -> Result<Vec<OutgoingLink>, ServerError> {
        self.page_read_access(caller, vault)?;
        let snap = self.engine.store().snapshot(vault)?;
        page_row(&snap, id)?;
        let edges = snap.edges_linked(std::slice::from_ref(id), &[WIKI_LINK, CLAIM_REF], EdgeDirection::From, after, limit)?;
        let mut keys: BTreeSet<RowKey> = BTreeSet::new();
        for e in &edges {
            keys.insert(match slug_of_target(&e.to) {
                Some(s) if e.kind == WIKI_LINK => key(Table::Slug, &ObjectId(s.to_string())),
                _ => key(Table::Assertion, &e.to),
            });
        }
        let keys: Vec<RowKey> = keys.into_iter().collect();
        let (mut holders, mut claims): (HashMap<String, ObjectId>, BTreeSet<ObjectId>) = (HashMap::new(), BTreeSet::new());
        if !keys.is_empty() {
            for row in snap.get_many(&keys)?.into_iter().flatten() {
                match row {
                    Row::Slug(s) => {
                        holders.insert(s.header.id.0, s.page_id);
                    }
                    Row::Assertion(a) => {
                        claims.insert(a.header.id);
                    }
                    _ => {}
                }
            }
        }
        Ok(edges.into_iter().map(|e| outgoing(e, &holders, &claims)).collect())
    }

    /// Every link of these kinds at one object, both ways, one hop, in id order. A page's links through any slug it has ever had count. Two store calls and two more to name the other ends.
    pub fn neighborhood(
        &self,
        caller: &Caller,
        vault: &VaultId,
        id: &ObjectId,
        kinds: &[&str],
        after: Option<&ObjectId>,
        limit: usize,
    ) -> Result<Vec<NeighborView>, ServerError> {
        self.page_read_access(caller, vault)?;
        let snap = self.engine.store().snapshot(vault)?;
        let tables = [Table::Page, Table::Assertion, Table::Entity, Table::Source, Table::Segment];
        let found = snap.get_many(&tables.map(|t| key(t, id)))?.into_iter().flatten().next();
        let Some(found) = found else {
            return Err(ServerError::NotFound);
        };
        let mut ends = vec![id.clone()];
        if let Row::Page(p) = &found {
            ends.extend(slugs_of(p));
        }
        let edges = snap.edges_linked(&ends, kinds, EdgeDirection::Both, after, limit)?;
        let outgoing_of = |e: &EdgeRow| ends.contains(&e.from);
        let mut keys: BTreeSet<RowKey> = BTreeSet::new();
        for e in edges.iter().filter(|e| outgoing_of(e)) {
            if let Some(s) = slug_of_target(&e.to) {
                keys.insert(key(Table::Slug, &ObjectId(s.to_string())));
            }
        }
        let keys: Vec<RowKey> = keys.into_iter().collect();
        let mut holders: HashMap<String, ObjectId> = HashMap::new();
        if !keys.is_empty() {
            for row in snap.get_many(&keys)?.into_iter().flatten() {
                if let Row::Slug(s) = row {
                    holders.insert(s.header.id.0, s.page_id);
                }
            }
        }
        let neighbor = |e: &EdgeRow| -> Option<ObjectId> {
            if !outgoing_of(e) {
                return Some(e.from.clone());
            }
            match slug_of_target(&e.to) {
                Some(s) => holders.get(s).cloned(),
                None => Some(e.to.clone()),
            }
        };
        let keys: Vec<RowKey> = edges
            .iter()
            .filter_map(&neighbor)
            .collect::<BTreeSet<_>>()
            .iter()
            .map(|p| key(Table::Page, p))
            .collect();
        let mut titles: HashMap<ObjectId, String> = HashMap::new();
        if !keys.is_empty() {
            for row in snap.get_many(&keys)?.into_iter().flatten() {
                if let Row::Page(p) = row {
                    titles.insert(p.header.id, p.title);
                }
            }
        }
        Ok(edges
            .iter()
            .map(|e| {
                let other = neighbor(e);
                NeighborView {
                    edge_id: e.header.id.clone(),
                    kind: e.kind.clone(),
                    direction: if outgoing_of(e) { "out" } else { "in" },
                    from: e.from.clone(),
                    to: e.to.clone(),
                    block_id: e.block_id.clone(),
                    title: other.as_ref().and_then(|o| titles.get(o).cloned()),
                    neighbor: other,
                }
            })
            .collect())
    }

    /// Broken links across the vault, in id order. They're listed, and nothing else minds them.
    pub fn unresolved_page_links(&self, caller: &Caller, vault: &VaultId, after: Option<&ObjectId>, limit: usize) -> Result<Vec<UnresolvedLink>, ServerError> {
        self.page_read_access(caller, vault)?;
        let rows = self.engine.store().snapshot(vault)?.unresolved_links(after, limit)?;
        Ok(rows
            .into_iter()
            .map(|e| UnresolvedLink {
                edge_id: e.header.id,
                target: slug_of_target(&e.to).map_or_else(|| e.to.0.clone(), str::to_string),
                kind: e.kind,
                from_page: e.from,
                block_id: e.block_id,
            })
            .collect())
    }

    /// Where a slug leads, old or current.
    pub fn page_redirect(&self, caller: &Caller, vault: &VaultId, slug: &str) -> Result<PageRedirect, ServerError> {
        self.page_read_access(caller, vault)?;
        let snap = self.engine.store().snapshot(vault)?;
        let Some(Row::Slug(s)) = snap.get(&key(Table::Slug, &ObjectId(slug.to_string())))? else {
            return Err(ServerError::NotFound);
        };
        let page = page_row(&snap, &s.page_id)?;
        Ok(PageRedirect {
            slug: slug.to_string(),
            redirected: page.slug != slug,
            page_id: s.page_id,
            current_slug: page.slug,
        })
    }

    /// The page as a Markdown file, at its current revision.
    pub fn export_page(&self, caller: &Caller, vault: &VaultId, id: &ObjectId) -> Result<String, ServerError> {
        self.page_read_access(caller, vault)?;
        let snap = self.engine.store().snapshot(vault)?;
        Ok(page::export_document(&page_row(&snap, id)?))
    }

    /// Reads a page's Markdown file. It makes a new page, or with `base_version` edits the page holding its slug. Links reach their pages by slug, so files can arrive in any order.
    pub fn import_page(
        &self,
        caller: &Caller,
        vault: &VaultId,
        document: &str,
        base_version: Option<u32>,
        rationale: String,
        idempotency_key: String,
    ) -> Result<ImportedPage, ServerError> {
        self.page_read_access(caller, vault)?;
        let doc: PageDocument = page::parse_document(document).map_err(ServerError::Invalid)?;
        let slug = match &doc.slug {
            Some(s) => s.clone(),
            None => page::slugify(&doc.title).ok_or_else(|| ServerError::Invalid(format!("no slug can be made from the title '{}'", doc.title)))?,
        };
        let existing = match self.engine.store().snapshot(vault)?.get(&key(Table::Slug, &ObjectId(slug.clone())))? {
            Some(Row::Slug(s)) => Some(s.page_id),
            _ => None,
        };
        match (existing, base_version) {
            (Some(page_id), Some(base)) => {
                let current = page_row(&self.engine.store().snapshot(vault)?, &page_id)?;
                if current.title != doc.title {
                    return Err(ServerError::Invalid(format!(
                        "this file's title is '{}' and the page's is '{}': an import changes only the body, so rename the page first",
                        doc.title, current.title
                    )));
                }
                let op = Op::EditPage {
                    id: page_id.clone(),
                    base_version: base,
                    body: doc.body,
                };
                let outcome = self.one_op(caller, vault, op, rationale, idempotency_key, false)?;
                Ok(ImportedPage {
                    page_id,
                    created: false,
                    outcome,
                })
            }
            (Some(_), None) => Err(ServerError::Conflict(format!(
                "a page already holds the slug '{slug}': give base_version to edit it"
            ))),
            (None, Some(_)) => Err(ServerError::NotFound),
            (None, None) => {
                let id = ObjectId(self.new_id());
                let op = Op::CreatePage {
                    id: id.clone(),
                    title: doc.title,
                    body: doc.body,
                    slug: Some(slug),
                    redirects: doc.redirects,
                };
                let outcome = self.one_op(caller, vault, op, rationale, idempotency_key, false)?;
                Ok(ImportedPage {
                    page_id: first_op_id(&outcome).unwrap_or(id),
                    created: true,
                    outcome,
                })
            }
        }
    }
}

fn outgoing(e: EdgeRow, holders: &HashMap<String, ObjectId>, claims: &BTreeSet<ObjectId>) -> OutgoingLink {
    let (target, to_page, resolved) = match slug_of_target(&e.to) {
        Some(slug) if e.kind == WIKI_LINK => {
            let page = holders.get(slug).cloned();
            let resolved = page.is_some();
            (slug.to_string(), page, resolved)
        }
        _ => (e.to.0.clone(), None, claims.contains(&e.to)),
    };
    OutgoingLink {
        edge_id: e.header.id,
        kind: e.kind,
        block_id: e.block_id,
        target,
        to_page,
        resolved,
    }
}
