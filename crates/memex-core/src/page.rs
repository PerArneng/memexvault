//! Wiki pages' pure rules: the Markdown parser, slugs, planning page ops into rows, block identity across edits, and what a page's search documents say.
use crate::governance::PlanError;
use crate::rendition::detect_language;
use crate::search::relevance::fold_text;
use crate::{Snapshot, StoreError};
use memex_types::{
    BlockKind, BlockRow, CLAIM_REF, DocKind, EdgeDirection, EdgeRow, Header, IndexDoc, IndexOp, Mutation, ObjectId, Op, PageRow, RevisionBlock, RevisionRow,
    Row, RowKey, SlugRow, Status, Table, WIKI_LINK, revision_id, slug_target,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// A ChangeSet's event holds a page about four times over: the op, the rows, the inverse and the revision. So this also bounds the event.
pub const MAX_PAGE_BYTES: usize = 256 * 1024;
/// Matching blocks across an edit takes time proportional to the square of this at worst.
pub const MAX_BLOCKS: usize = 2000;
pub const MAX_LINKS: usize = 2000;
pub const MAX_TITLE_BYTES: usize = 500;
pub const MAX_SLUG_BYTES: usize = 200;
pub const MAX_REDIRECTS: usize = 100;
const MAX_CLAIM_ID_BYTES: usize = 64;

/// A `[[target|label]]`. The label is what shows, and the target is what it reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WikiLink {
    pub target: String,
    pub slug: String,
    pub label: Option<String>,
}

/// One block of parsed Markdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedBlock {
    pub kind: BlockKind,
    pub text: String,
    pub heading: Option<String>,
}

/// What a block's text refers to, and its words with the link syntax gone.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Inline {
    pub links: Vec<WikiLink>,
    pub claims: Vec<ObjectId>,
    pub plain: String,
}

/// A slug from any title: folded, lower case, with runs of anything but letters and digits as one dash. Letters outside Latin stay. None if nothing is left.
pub fn slugify(text: &str) -> Option<String> {
    let folded = fold_text(text).to_lowercase();
    let mut out = String::with_capacity(folded.len());
    for c in folded.chars() {
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-');
    let mut end = out.len().min(MAX_SLUG_BYTES);
    while !out.is_char_boundary(end) {
        end -= 1;
    }
    let out = out[..end].trim_end_matches('-');
    (!out.is_empty()).then(|| out.to_string())
}

/// Whether a slug is one `slugify` could have made.
pub fn slug_is_valid(slug: &str) -> bool {
    slugify(slug).as_deref() == Some(slug)
}

fn fence_of(line: &str) -> Option<(char, usize)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let c = rest.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let run = rest.chars().take_while(|x| *x == c).count();
    if run < 3 || (c == '`' && rest[run..].contains('`')) {
        return None;
    }
    Some((c, run))
}

fn closes(line: &str, open: (char, usize)) -> bool {
    let text = line.trim();
    text.chars().count() >= open.1 && text.chars().all(|c| c == open.0) && line.len() - line.trim_start_matches(' ').len() <= 3
}

fn is_heading(line: &str) -> bool {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') && !line[hashes..].trim().is_empty()
}

fn is_rule(line: &str) -> bool {
    let text: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    text.len() >= 3 && ['-', '*', '_'].iter().any(|m| text.chars().all(|c| c == *m))
}

fn kind_of(first_line: &str) -> BlockKind {
    let text = first_line.trim_start();
    let numbered = text.chars().take_while(char::is_ascii_digit).count();
    if text.starts_with('>') {
        BlockKind::Quote
    } else if text.starts_with("- ") || text.starts_with("* ") || text.starts_with("+ ") || (numbered > 0 && text[numbered..].starts_with(". ")) {
        BlockKind::List
    } else if text.starts_with('|') {
        BlockKind::Table
    } else {
        BlockKind::Paragraph
    }
}

fn heading_text(block: &str) -> String {
    block.trim_start_matches('#').trim().trim_end_matches('#').trim().to_string()
}

/// Splits Markdown into blocks. Blocks end at a blank line, and a heading, a rule or a code fence is always its own block. Lines keep their words, losing only trailing space outside code, so rendering and parsing again gives the same blocks.
pub fn parse_markdown(body: &str) -> Vec<ParsedBlock> {
    let normal = body.replace("\r\n", "\n").replace('\r', "\n");
    let mut out: Vec<(BlockKind, String)> = vec![];
    let mut para: Vec<&str> = vec![];
    let flush = |para: &mut Vec<&str>, out: &mut Vec<(BlockKind, String)>| {
        if let Some(first) = para.first() {
            out.push((kind_of(first), para.join("\n")));
            para.clear();
        }
    };
    let mut lines = normal.split('\n').peekable();
    while let Some(raw) = lines.next() {
        let line = raw.trim_end();
        if line.is_empty() {
            flush(&mut para, &mut out);
        } else if let Some(open) = fence_of(line) {
            flush(&mut para, &mut out);
            let mut code = vec![line.to_string()];
            let mut closed = false;
            for inner in lines.by_ref() {
                if closes(inner, open) {
                    code.push(inner.trim_end().to_string());
                    closed = true;
                    break;
                }
                code.push(inner.to_string());
            }
            if !closed {
                // A fence left open runs to the end, and blank lines there aren't part of anything.
                while code.last().is_some_and(|l| l.trim().is_empty()) {
                    code.pop();
                }
            }
            out.push((BlockKind::Code, code.join("\n")));
        } else if is_heading(line) {
            flush(&mut para, &mut out);
            out.push((BlockKind::Heading, line.to_string()));
        } else if is_rule(line) {
            flush(&mut para, &mut out);
            out.push((BlockKind::Rule, line.to_string()));
        } else {
            para.push(line);
        }
    }
    flush(&mut para, &mut out);
    let mut heading = None;
    out.into_iter()
        .map(|(kind, text)| {
            let block = ParsedBlock {
                kind,
                heading: heading.clone(),
                text,
            };
            if kind == BlockKind::Heading {
                heading = Some(heading_text(&block.text));
            }
            block
        })
        .collect()
}

/// The canonical Markdown of some blocks. Every stored body is this, so a body is exactly its blocks.
pub fn render_markdown<'a>(blocks: impl IntoIterator<Item = &'a str>) -> String {
    let mut out = String::new();
    for text in blocks {
        out.push_str(text);
        out.push_str("\n\n");
    }
    out.truncate(out.trim_end_matches('\n').len());
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

fn claim_id_is_valid(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_CLAIM_ID_BYTES && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

/// Finds links and claim references in a block's text. Text that isn't well-formed stays as text, as code and backslash-escaped openers do. Pure, and never fails.
pub fn parse_inline(kind: BlockKind, text: &str) -> Inline {
    let mut out = Inline::default();
    if kind == BlockKind::Code {
        out.plain = text.lines().filter(|l| fence_of(l).is_none()).collect::<Vec<_>>().join("\n");
        return out;
    }
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if c == '\\' && (rest[1..].starts_with("[[") || rest[1..].starts_with("{{")) {
            out.plain.push_str(&rest[1..3]);
            rest = &rest[3..];
        } else if c == '`' {
            // An inline code span runs to the next backtick. Without one, it's a plain backtick.
            let inner = rest[1..].find('`');
            let end = inner.map_or(1, |i| i + 2);
            out.plain.push_str(&rest[..end]);
            rest = &rest[end..];
        } else if rest.starts_with("[[") {
            match parse_link(&rest[2..]) {
                Some((link, used)) => {
                    out.plain.push_str(link.label.as_deref().unwrap_or(&link.target));
                    out.links.push(link);
                    rest = &rest[2 + used..];
                }
                None => {
                    out.plain.push_str("[[");
                    rest = &rest[2..];
                }
            }
        } else if let Some(after) = rest.strip_prefix("{{claim:") {
            match after.find("}}").filter(|end| claim_id_is_valid(&after[..*end])) {
                Some(end) => {
                    out.claims.push(ObjectId(after[..end].to_string()));
                    rest = &after[end + 2..];
                }
                None => {
                    out.plain.push_str("{{");
                    rest = &rest[2..];
                }
            }
        } else {
            out.plain.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

fn parse_link(after_open: &str) -> Option<(WikiLink, usize)> {
    let end = after_open.find("]]")?;
    let inner = &after_open[..end];
    if inner.contains('\n') || inner.contains('[') {
        return None;
    }
    let (target, label) = match inner.split_once('|') {
        Some((t, l)) => (t.trim(), Some(l.trim().to_string()).filter(|l| !l.is_empty())),
        None => (inner.trim(), None),
    };
    let slug = slugify(target)?;
    Some((
        WikiLink {
            target: target.to_string(),
            slug,
            label,
        },
        end + 2,
    ))
}

/// A page as a Markdown file, and what one says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageDocument {
    pub title: String,
    pub slug: Option<String>,
    pub redirects: Vec<String>,
    pub body: String,
}

/// Values are JSON, so any title survives. Old slugs travel with it, because links in other pages may still use them.
pub fn export_document(page: &PageRow) -> String {
    let text = |t: &str| serde_json::Value::from(t).to_string();
    let mut out = format!("---\ntitle: {}\nslug: {}\n", text(&page.title), text(&page.slug));
    if !page.redirects.is_empty() {
        out.push_str(&format!("redirects: {}\n", serde_json::json!(page.redirects)));
    }
    out.push_str("---\n\n");
    out.push_str(&page.body);
    out
}

/// Reads a page's Markdown file. Without a front matter block, its first heading is the title and stays in the body.
pub fn parse_document(document: &str) -> Result<PageDocument, String> {
    let normal = document.replace("\r\n", "\n");
    let Some(after) = normal.strip_prefix("---\n") else {
        let title = parse_markdown(&normal)
            .iter()
            .find(|b| b.kind == BlockKind::Heading)
            .map(|b| heading_text(&b.text))
            .ok_or("a page's file needs a front matter block with a title, or a heading")?;
        return Ok(PageDocument {
            title,
            slug: None,
            redirects: vec![],
            body: normal,
        });
    };
    let end = after.find("\n---\n").ok_or("the front matter block isn't closed")?;
    let (mut title, mut slug, mut redirects) = (None, None, vec![]);
    for line in after[..end].lines() {
        let (key, value) = line.split_once(':').ok_or_else(|| format!("'{line}' isn't a front matter line"))?;
        let bad = |e: serde_json::Error| format!("the front matter value of {key} isn't valid: {e}");
        match key.trim() {
            "title" => title = Some(serde_json::from_str::<String>(value.trim()).map_err(bad)?),
            "slug" => slug = Some(serde_json::from_str::<String>(value.trim()).map_err(bad)?),
            "redirects" => redirects = serde_json::from_str::<Vec<String>>(value.trim()).map_err(bad)?,
            other => return Err(format!("unknown front matter key '{other}'")),
        }
    }
    Ok(PageDocument {
        title: title.ok_or("the front matter block has no title")?,
        slug,
        redirects,
        body: after[end + 5..].trim_start_matches('\n').to_string(),
    })
}

/// Which old block each new block continues, by position. An unchanged block keeps its id even if it moved, and an edited one keeps the id of the old block in its place.
fn match_blocks(old: &[&str], new: &[&str]) -> Vec<Option<usize>> {
    let mut matched: Vec<Option<usize>> = vec![None; new.len()];
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..].iter().rev().zip(new[prefix..].iter().rev()).take_while(|(a, b)| a == b).count();
    for (i, m) in matched.iter_mut().enumerate().take(prefix) {
        *m = Some(i);
    }
    for i in 0..suffix {
        matched[new.len() - 1 - i] = Some(old.len() - 1 - i);
    }
    let (a, b) = (&old[prefix..old.len() - suffix], &new[prefix..new.len() - suffix]);
    // The longest common subsequence of what's left.
    let mut table = vec![0u16; (a.len() + 1) * (b.len() + 1)];
    let width = b.len() + 1;
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            table[i * width + j] = if a[i] == b[j] {
                table[(i + 1) * width + j + 1] + 1
            } else {
                table[(i + 1) * width + j].max(table[i * width + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            matched[prefix + j] = Some(prefix + i);
            i += 1;
            j += 1;
        } else if table[(i + 1) * width + j] >= table[i * width + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    let mut taken: BTreeSet<usize> = matched.iter().flatten().copied().collect();
    // A block with the same text elsewhere moved.
    for (j, m) in matched.iter_mut().enumerate() {
        if m.is_none()
            && let Some(i) = (0..old.len()).find(|i| !taken.contains(i) && old[*i] == new[j])
        {
            *m = Some(i);
            taken.insert(i);
        }
    }
    // Between two kept blocks, what's left was edited in place, in order.
    let mut free = (0..old.len()).filter(|i| !taken.contains(i));
    let mut previous: Option<usize> = None;
    let mut pending: Option<usize> = free.next();
    for m in &mut matched {
        match m {
            Some(i) => previous = Some(*i),
            None => {
                while pending.is_some_and(|p| previous.is_some_and(|q| p < q)) {
                    pending = free.next();
                }
                if let Some(p) = pending {
                    *m = Some(p);
                    previous = Some(p);
                    pending = free.next();
                }
            }
        }
    }
    matched
}

/// One change between two revisions, by block id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockChange {
    pub block_id: ObjectId,
    /// `added`, `removed`, `changed` or `moved`.
    pub change: &'static str,
    pub before: Option<String>,
    pub after: Option<String>,
}

/// What differs between two revisions. Pure.
pub fn diff_revisions(from: &RevisionRow, to: &RevisionRow) -> Vec<BlockChange> {
    let before: HashMap<&ObjectId, (usize, &RevisionBlock)> = from.blocks.iter().enumerate().map(|(i, b)| (&b.id, (i, b))).collect();
    let after_ids: BTreeSet<&ObjectId> = to.blocks.iter().map(|b| &b.id).collect();
    let mut out = vec![];
    // Blocks kept in both, in the old order, show which of them moved: those out of line with the rest.
    let kept: Vec<usize> = to.blocks.iter().filter_map(|b| before.get(&b.id).map(|(i, _)| *i)).collect();
    let in_order = longest_increasing(&kept);
    for b in &from.blocks {
        if !after_ids.contains(&b.id) {
            out.push(BlockChange {
                block_id: b.id.clone(),
                change: "removed",
                before: Some(b.text.clone()),
                after: None,
            });
        }
    }
    for b in &to.blocks {
        match before.get(&b.id) {
            None => out.push(BlockChange {
                block_id: b.id.clone(),
                change: "added",
                before: None,
                after: Some(b.text.clone()),
            }),
            Some((i, old)) if old.text != b.text || !in_order.contains(i) => out.push(BlockChange {
                block_id: b.id.clone(),
                change: if old.text != b.text { "changed" } else { "moved" },
                before: Some(old.text.clone()),
                after: Some(b.text.clone()),
            }),
            Some(_) => {}
        }
    }
    out
}

fn longest_increasing(values: &[usize]) -> BTreeSet<usize> {
    let mut tails: Vec<usize> = vec![];
    let mut links: Vec<Option<usize>> = vec![None; values.len()];
    for (i, v) in values.iter().enumerate() {
        let at = tails.partition_point(|t| values[*t] < *v);
        links[i] = at.checked_sub(1).map(|p| tails[p]);
        if at == tails.len() {
            tails.push(i);
        } else {
            tails[at] = i;
        }
    }
    let mut out = BTreeSet::new();
    let mut at = tails.last().copied();
    while let Some(i) = at {
        out.insert(values[i]);
        at = links[i];
    }
    out
}

fn key(table: Table, id: &ObjectId) -> RowKey {
    RowKey { table, id: id.clone() }
}

fn header(id: ObjectId, creator: &str) -> Header {
    Header {
        id,
        creator: creator.into(),
        status: Status::Active,
        version: 1,
        acl_id: None,
    }
}

/// Whether a ChangeSet has any page in it, so other proposals pay nothing for these checks.
pub fn touches_pages(ops: &[Op]) -> bool {
    ops.iter().any(|op| {
        matches!(
            op,
            Op::CreatePage { .. } | Op::EditPage { .. } | Op::DeletePage { .. } | Op::RenamePage { .. } | Op::RevertPage { .. }
        )
    })
}

/// Refuses a page that's too large before anything else looks at it, so its bytes never reach the log, not even in a rejected ChangeSet.
pub fn check_sizes(ops: &[Op]) -> Result<(), String> {
    for op in ops {
        if let Op::CreatePage { body, .. } | Op::EditPage { body, .. } = op
            && body.len() > MAX_PAGE_BYTES
        {
            return Err(format!("the page is {} bytes, over the limit of {MAX_PAGE_BYTES}", body.len()));
        }
        if let Op::CreatePage { title, .. } | Op::RenamePage { title, .. } = op
            && title.len() > MAX_TITLE_BYTES
        {
            return Err(format!("a title can have at most {MAX_TITLE_BYTES} bytes"));
        }
    }
    Ok(())
}

fn check_title(title: &str) -> Result<(), PlanError> {
    if title.trim().is_empty() || title.contains('\n') {
        return Err(PlanError::Invalid("a page needs a title of one line".into()));
    }
    Ok(())
}

fn parsed(body: &str) -> Result<Vec<ParsedBlock>, PlanError> {
    if body.len() > MAX_PAGE_BYTES {
        return Err(PlanError::Invalid(format!(
            "the page is {} bytes, over the limit of {MAX_PAGE_BYTES}",
            body.len()
        )));
    }
    let blocks = parse_markdown(body);
    if blocks.len() > MAX_BLOCKS {
        return Err(PlanError::Invalid(format!(
            "the page has {} blocks, over the limit of {MAX_BLOCKS}",
            blocks.len()
        )));
    }
    Ok(blocks)
}

/// What a page's edges should be, as (kind, target, block).
fn wanted_edges(blocks: &[BlockRow]) -> Result<BTreeSet<(String, ObjectId, ObjectId)>, PlanError> {
    let mut out = BTreeSet::new();
    for b in blocks {
        let inline = parse_inline(b.kind, &b.text);
        for link in inline.links {
            out.insert((WIKI_LINK.to_string(), slug_target(&link.slug), b.header.id.clone()));
        }
        for claim in inline.claims {
            out.insert((CLAIM_REF.to_string(), claim, b.header.id.clone()));
        }
    }
    if out.len() > MAX_LINKS {
        return Err(PlanError::Invalid(format!("the page has {} links, over the limit of {MAX_LINKS}", out.len())));
    }
    Ok(out)
}

fn stale(row: PageRow, base: u32) -> PlanError {
    PlanError::Stale {
        key: key(Table::Page, &row.header.id),
        current: Box::new(Row::Page(row)),
        base,
    }
}

/// Plans a ChangeSet's page ops against the current state. Slugs taken earlier in the same ChangeSet count.
struct Work<'a, S: Snapshot> {
    snap: &'a S,
    changeset: &'a ObjectId,
    creator: &'a str,
    now_ms: i64,
    new_id: &'a mut dyn FnMut() -> ObjectId,
    /// The page each slug now belongs to, for slugs this ChangeSet touched. None means it's been let go.
    slugs: BTreeMap<String, Option<ObjectId>>,
    done: BTreeSet<ObjectId>,
    out: Vec<Mutation>,
}

impl<S: Snapshot> Work<'_, S> {
    fn holder(&mut self, slug: &str) -> Result<Option<ObjectId>, PlanError> {
        if let Some(known) = self.slugs.get(slug) {
            return Ok(known.clone());
        }
        Ok(match self.snap.get(&key(Table::Slug, &ObjectId(slug.to_string())))? {
            Some(Row::Slug(s)) => Some(s.page_id),
            _ => None,
        })
    }

    /// Takes a slug for this page, unless another page holds it. A link can't change meaning under anyone.
    fn claim_slug(&mut self, slug: &str, page: &ObjectId) -> Result<(), PlanError> {
        if !slug_is_valid(slug) {
            return Err(PlanError::Invalid(format!("'{slug}' isn't a valid slug")));
        }
        match self.holder(slug)? {
            Some(other) if other == *page => return Ok(()),
            Some(_) => return Err(PlanError::Exists(key(Table::Slug, &ObjectId(slug.to_string())))),
            None => {}
        }
        self.slugs.insert(slug.to_string(), Some(page.clone()));
        self.out.push(Mutation::Put(Row::Slug(SlugRow {
            header: header(ObjectId(slug.to_string()), self.creator),
            page_id: page.clone(),
        })));
        Ok(())
    }

    fn once_per_changeset(&mut self, id: &ObjectId) -> Result<(), PlanError> {
        if self.done.insert(id.clone()) {
            Ok(())
        } else {
            Err(PlanError::Invalid(format!("page {id} appears twice in this ChangeSet")))
        }
    }

    fn at_version(&mut self, id: &ObjectId, base: u32) -> Result<PageRow, PlanError> {
        self.once_per_changeset(id)?;
        let k = key(Table::Page, id);
        let Some(Row::Page(page)) = self.snap.get(&k)? else {
            return Err(PlanError::Missing(k));
        };
        if page.header.version != base {
            return Err(stale(page, base));
        }
        Ok(page)
    }

    fn current_blocks(&self, id: &ObjectId) -> Result<Vec<BlockRow>, PlanError> {
        Ok(self.snap.blocks_of(std::slice::from_ref(id), MAX_BLOCKS + 1)?)
    }

    fn record_revision(&mut self, page: &PageRow, blocks: &[BlockRow], restored_from: Option<u32>) {
        self.out.push(Mutation::Put(Row::Revision(RevisionRow {
            header: header(revision_id(&page.header.id, page.header.version), self.creator),
            page_id: page.header.id.clone(),
            revision: page.header.version,
            title: page.title.clone(),
            slug: page.slug.clone(),
            blocks: blocks
                .iter()
                .map(|b| RevisionBlock {
                    id: b.header.id.clone(),
                    kind: b.kind,
                    text: b.text.clone(),
                })
                .collect(),
            changeset_id: self.changeset.clone(),
            created_at_ms: self.now_ms,
            restored_from,
        })));
    }

    /// Writes what differs between the blocks and edges the page has and the ones it should have.
    fn write_blocks(&mut self, id: &ObjectId, old: &[BlockRow], new: &[BlockRow]) -> Result<(), PlanError> {
        let before: HashMap<&ObjectId, &BlockRow> = old.iter().map(|b| (&b.header.id, b)).collect();
        for b in new {
            let mut row = b.clone();
            // A block keeps who first wrote it, so an untouched one is exactly what it was, and isn't written.
            if let Some(o) = before.get(&b.header.id) {
                row.header.creator.clone_from(&o.header.creator);
                if **o == row {
                    continue;
                }
            }
            self.out.push(Mutation::Put(Row::Block(row)));
        }
        let kept: BTreeSet<&ObjectId> = new.iter().map(|b| &b.header.id).collect();
        for b in old.iter().filter(|b| !kept.contains(&b.header.id)) {
            self.out.push(Mutation::Delete(key(Table::Block, &b.header.id)));
        }
        let mut wanted = wanted_edges(new)?;
        let edges = self
            .snap
            .edges_linked(std::slice::from_ref(id), &[WIKI_LINK, CLAIM_REF], EdgeDirection::From, None, MAX_LINKS + 1)?;
        for e in edges {
            let Some(block) = e.block_id.clone() else { continue };
            if !wanted.remove(&(e.kind.clone(), e.to.clone(), block)) {
                self.out.push(Mutation::Delete(key(Table::Edge, &e.header.id)));
            }
        }
        for (kind, to, block) in wanted {
            self.out.push(Mutation::Put(Row::Edge(EdgeRow {
                header: header((self.new_id)(), self.creator),
                from: id.clone(),
                to,
                kind,
                block_id: Some(block),
            })));
        }
        Ok(())
    }

    fn block_row(&self, id: ObjectId, version: u32, page: &ObjectId, position: usize, b: ParsedBlock) -> BlockRow {
        let mut h = header(id, self.creator);
        h.version = version;
        BlockRow {
            header: h,
            page_id: page.clone(),
            position: position as u32,
            kind: b.kind,
            text: b.text,
            heading: b.heading,
        }
    }

    fn create(&mut self, id: &ObjectId, title: &str, body: &str, slug: Option<&str>, redirects: &[String]) -> Result<(), PlanError> {
        self.once_per_changeset(id)?;
        let k = key(Table::Page, id);
        if self.snap.get(&k)?.is_some() {
            return Err(PlanError::Exists(k));
        }
        check_title(title)?;
        let blocks = parsed(body)?;
        let slug = match slug {
            Some(s) => s.to_string(),
            None => slugify(title).ok_or_else(|| PlanError::Invalid(format!("no slug can be made from the title '{title}'")))?,
        };
        if redirects.len() > MAX_REDIRECTS {
            return Err(PlanError::Invalid(format!("a page can have at most {MAX_REDIRECTS} old slugs")));
        }
        self.claim_slug(&slug, id)?;
        for r in redirects {
            self.claim_slug(r, id)?;
        }
        let rows: Vec<BlockRow> = blocks
            .into_iter()
            .enumerate()
            .map(|(i, b)| {
                let block_id = (self.new_id)();
                self.block_row(block_id, 1, id, i, b)
            })
            .collect();
        let page = PageRow {
            header: header(id.clone(), self.creator),
            title: title.to_string(),
            body: render_markdown(rows.iter().map(|b| b.text.as_str())),
            updated_at_ms: self.now_ms,
            slug,
            redirects: redirects.to_vec(),
        };
        self.write_blocks(id, &[], &rows)?;
        self.record_revision(&page, &rows, None);
        self.out.push(Mutation::Put(Row::Page(page)));
        Ok(())
    }

    /// A page from before pages had slugs gets one on its first change.
    fn ensure_slug(&mut self, page: &mut PageRow) -> Result<(), PlanError> {
        if page.slug.is_empty() {
            let slug = slugify(&page.title).ok_or_else(|| PlanError::Invalid(format!("no slug can be made from the title '{}'", page.title)))?;
            self.claim_slug(&slug, &page.header.id)?;
            page.slug = slug;
        }
        Ok(())
    }

    fn edit(&mut self, id: &ObjectId, base: u32, body: &str) -> Result<(), PlanError> {
        let mut page = self.at_version(id, base)?;
        let blocks = parsed(body)?;
        self.ensure_slug(&mut page)?;
        let old = self.current_blocks(id)?;
        let old_text: Vec<&str> = old.iter().map(|b| b.text.as_str()).collect();
        let new_text: Vec<String> = blocks.iter().map(|b| b.text.clone()).collect();
        let new_refs: Vec<&str> = new_text.iter().map(String::as_str).collect();
        let matched = match_blocks(&old_text, &new_refs);
        let rows: Vec<BlockRow> = blocks
            .into_iter()
            .zip(matched)
            .enumerate()
            .map(|(i, (b, m))| match m {
                Some(o) => {
                    let version = if old[o].text == b.text {
                        old[o].header.version
                    } else {
                        old[o].header.version + 1
                    };
                    self.block_row(old[o].header.id.clone(), version, id, i, b)
                }
                None => {
                    let block_id = (self.new_id)();
                    self.block_row(block_id, 1, id, i, b)
                }
            })
            .collect();
        page.body = render_markdown(rows.iter().map(|b| b.text.as_str()));
        page.header.version += 1;
        page.updated_at_ms = self.now_ms;
        self.write_blocks(id, &old, &rows)?;
        self.record_revision(&page, &rows, None);
        self.out.push(Mutation::Put(Row::Page(page)));
        Ok(())
    }

    fn change_slug(&mut self, page: &mut PageRow, slug: &str) -> Result<(), PlanError> {
        if page.slug == slug {
            return Ok(());
        }
        self.claim_slug(slug, &page.header.id)?;
        let old = std::mem::replace(&mut page.slug, slug.to_string());
        page.redirects.retain(|r| r != slug);
        if !old.is_empty() {
            page.redirects.push(old);
        }
        if page.redirects.len() > MAX_REDIRECTS {
            return Err(PlanError::Invalid(format!("a page can have at most {MAX_REDIRECTS} old slugs")));
        }
        Ok(())
    }

    fn rename(&mut self, id: &ObjectId, base: u32, title: &str, slug: Option<&str>) -> Result<(), PlanError> {
        let mut page = self.at_version(id, base)?;
        check_title(title)?;
        let slug = match slug {
            Some(s) => s.to_string(),
            None => slugify(title).ok_or_else(|| PlanError::Invalid(format!("no slug can be made from the title '{title}'")))?,
        };
        self.change_slug(&mut page, &slug)?;
        page.title = title.to_string();
        page.header.version += 1;
        page.updated_at_ms = self.now_ms;
        let blocks = self.current_blocks(id)?;
        self.record_revision(&page, &blocks, None);
        self.out.push(Mutation::Put(Row::Page(page)));
        Ok(())
    }

    fn revert(&mut self, id: &ObjectId, base: u32, revision: u32) -> Result<(), PlanError> {
        let mut page = self.at_version(id, base)?;
        let k = key(Table::Revision, &revision_id(id, revision));
        let Some(Row::Revision(old)) = self.snap.get(&k)? else {
            return Err(PlanError::Invalid(format!("page {id} has no revision {revision}")));
        };
        let current = self.current_blocks(id)?;
        let versions: HashMap<&ObjectId, u32> = current.iter().map(|b| (&b.header.id, b.header.version)).collect();
        let texts: HashMap<&ObjectId, &str> = current.iter().map(|b| (&b.header.id, b.text.as_str())).collect();
        let mut heading = None;
        let mut rows = Vec::with_capacity(old.blocks.len());
        for (i, b) in old.blocks.iter().enumerate() {
            // A block that comes back with other text is a new version of it. Without changes, its row stays untouched.
            let version = match (versions.get(&b.id), texts.get(&b.id)) {
                (Some(v), Some(t)) if *t == b.text => *v,
                (Some(v), _) => v + 1,
                _ => 1,
            };
            rows.push(self.block_row(
                b.id.clone(),
                version,
                id,
                i,
                ParsedBlock {
                    kind: b.kind,
                    text: b.text.clone(),
                    heading: heading.clone(),
                },
            ));
            if b.kind == BlockKind::Heading {
                heading = Some(heading_text(&b.text));
            }
        }
        self.change_slug(&mut page, &old.slug)?;
        page.title.clone_from(&old.title);
        page.body = render_markdown(rows.iter().map(|b| b.text.as_str()));
        page.header.version += 1;
        page.updated_at_ms = self.now_ms;
        self.write_blocks(id, &current, &rows)?;
        self.record_revision(&page, &rows, Some(revision));
        self.out.push(Mutation::Put(Row::Page(page)));
        Ok(())
    }

    /// The page, its blocks, its own edges and its slugs go. Links to it stay, and read as broken until a page takes the slug. Its revisions stay too, as history outlives it.
    fn delete(&mut self, id: &ObjectId, base: u32) -> Result<(), PlanError> {
        let page = self.at_version(id, base)?;
        for b in self.current_blocks(id)? {
            self.out.push(Mutation::Delete(key(Table::Block, &b.header.id)));
        }
        for e in self
            .snap
            .edges_linked(std::slice::from_ref(id), &[WIKI_LINK, CLAIM_REF], EdgeDirection::From, None, MAX_LINKS + 1)?
        {
            self.out.push(Mutation::Delete(key(Table::Edge, &e.header.id)));
        }
        for slug in page.redirects.iter().chain(std::iter::once(&page.slug)).filter(|s| !s.is_empty()) {
            self.slugs.insert(slug.clone(), None);
            self.out.push(Mutation::Delete(key(Table::Slug, &ObjectId(slug.clone()))));
        }
        self.out.push(Mutation::Delete(key(Table::Page, id)));
        Ok(())
    }
}

/// The rows a ChangeSet's page ops lead to. Base versions are checked again here, at commit.
pub fn plan_pages<S: Snapshot>(
    snap: &S,
    ops: &[Op],
    creator: &str,
    changeset: &ObjectId,
    now_ms: i64,
    new_id: &mut dyn FnMut() -> ObjectId,
) -> Result<Vec<Mutation>, PlanError> {
    if !touches_pages(ops) {
        return Ok(vec![]);
    }
    let mut work = Work {
        snap,
        changeset,
        creator,
        now_ms,
        new_id,
        slugs: BTreeMap::new(),
        done: BTreeSet::new(),
        out: vec![],
    };
    for op in ops {
        match op {
            Op::CreatePage {
                id,
                title,
                body,
                slug,
                redirects,
            } => work.create(id, title, body, slug.as_deref(), redirects)?,
            Op::EditPage { id, base_version, body } => work.edit(id, *base_version, body)?,
            Op::RenamePage { id, base_version, title, slug } => work.rename(id, *base_version, title, slug.as_deref())?,
            Op::RevertPage { id, base_version, revision } => work.revert(id, *base_version, *revision)?,
            Op::DeletePage { id, base_version } => work.delete(id, *base_version)?,
            _ => {}
        }
    }
    Ok(work.out)
}

/// The search document for a page: its title and headings, which is what a block's document doesn't have. A page from before spec 022 has no blocks, so its body goes here too.
pub(crate) fn page_doc(page: &PageRow, headings: &[&BlockRow]) -> IndexDoc {
    let mut text = page.title.clone();
    for h in headings {
        text.push('\n');
        text.push_str(&parse_inline(h.kind, &heading_text(&h.text)).plain);
    }
    if page.slug.is_empty() {
        text.push('\n');
        text.push_str(&page.body);
    }
    IndexDoc {
        id: page.header.id.clone(),
        kind: DocKind::Page,
        language: detect_language(&text),
        text,
        source_ids: vec![],
        epistemic: None,
        valid_from_ms: None,
        valid_to_ms: None,
        doc_type: None,
        trust_tiers: vec![],
        attrs: vec![],
    }
}

/// The search document for a block, or none if it holds no words of its own: a heading is part of its page's document, and a block that's only a claim reference has its claim indexed already.
pub(crate) fn block_doc(block: &BlockRow) -> Option<IndexDoc> {
    if matches!(block.kind, BlockKind::Heading | BlockKind::Rule) {
        return None;
    }
    let plain = parse_inline(block.kind, &block.text).plain;
    if plain.trim().is_empty() {
        return None;
    }
    let language = detect_language(&plain);
    let text = match &block.heading {
        Some(h) if !h.is_empty() => format!("{}\n{plain}", parse_inline(BlockKind::Heading, h).plain),
        _ => plain,
    };
    Some(IndexDoc {
        id: block.header.id.clone(),
        kind: DocKind::Block,
        language,
        text,
        source_ids: vec![],
        epistemic: None,
        valid_from_ms: None,
        valid_to_ms: None,
        doc_type: None,
        trust_tiers: vec![],
        attrs: vec![],
    })
}

/// As `segment_ops`, for pages. Store calls don't grow with the number of ids.
pub fn page_ops<S: Snapshot>(snap: &S, ids: &[ObjectId]) -> Result<Vec<IndexOp>, StoreError> {
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let keys: Vec<RowKey> = ids.iter().map(|id| key(Table::Page, id)).collect();
    let pages: Vec<PageRow> = snap
        .get_many(&keys)?
        .into_iter()
        .filter_map(|r| match r {
            Some(Row::Page(p)) => Some(p),
            _ => None,
        })
        .collect();
    let found_ids: Vec<ObjectId> = pages.iter().map(|p| p.header.id.clone()).collect();
    let blocks = if found_ids.is_empty() { vec![] } else { snap.headings_of(&found_ids)? };
    let mut by_page: HashMap<&ObjectId, Vec<&BlockRow>> = HashMap::new();
    for b in &blocks {
        by_page.entry(&b.page_id).or_default().push(b);
    }
    let mut found: HashMap<ObjectId, IndexOp> = HashMap::with_capacity(pages.len());
    for p in &pages {
        let headings = by_page.get(&p.header.id).map_or(&[][..], Vec::as_slice);
        found.insert(p.header.id.clone(), IndexOp::Upsert(page_doc(p, headings)));
    }
    Ok(ids.iter().map(|id| found.remove(id).unwrap_or_else(|| IndexOp::Delete(id.clone()))).collect())
}

/// As `segment_ops`, for blocks. One store call.
pub fn block_ops<S: Snapshot>(snap: &S, ids: &[ObjectId]) -> Result<Vec<IndexOp>, StoreError> {
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let keys: Vec<RowKey> = ids.iter().map(|id| key(Table::Block, id)).collect();
    let rows = snap.get_many(&keys)?;
    Ok(ids
        .iter()
        .zip(rows)
        .map(|(id, row)| match row {
            Some(Row::Block(b)) => block_doc(&b).map_or_else(|| IndexOp::Delete(id.clone()), IndexOp::Upsert),
            _ => IndexOp::Delete(id.clone()),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(blocks: &[ParsedBlock]) -> Vec<&str> {
        blocks.iter().map(|b| b.text.as_str()).collect()
    }

    fn roundtrip(markdown: &str) {
        let once = parse_markdown(markdown);
        let rendered = render_markdown(texts(&once));
        let twice = parse_markdown(&rendered);
        assert_eq!(once, twice, "{markdown:?}");
        assert_eq!(rendered, render_markdown(texts(&twice)), "{markdown:?}");
    }

    #[test]
    fn markdown_splits_into_blocks_of_the_right_kinds() {
        let md = "# Title\nFirst para\nstill first\n\n- a\n- b\n\n> quote\n\n```rust\nlet x = 1;\n\nlet y = 2;\n```\n\n| a |\n\n---\n\n## Next\n1. one\n";
        let blocks = parse_markdown(md);
        let kinds: Vec<BlockKind> = blocks.iter().map(|b| b.kind).collect();
        assert_eq!(
            kinds,
            [
                BlockKind::Heading,
                BlockKind::Paragraph,
                BlockKind::List,
                BlockKind::Quote,
                BlockKind::Code,
                BlockKind::Table,
                BlockKind::Rule,
                BlockKind::Heading,
                BlockKind::List
            ]
        );
        assert_eq!(blocks[1].text, "First para\nstill first");
        assert_eq!(blocks[4].text, "```rust\nlet x = 1;\n\nlet y = 2;\n```", "a blank line in code doesn't end it");
        assert_eq!(blocks[1].heading.as_deref(), Some("Title"));
        assert_eq!(blocks[8].heading.as_deref(), Some("Next"));
    }

    #[test]
    fn fixtures_round_trip_to_the_same_blocks() {
        for md in [
            "",
            "\n\n\n",
            "plain",
            "a\r\nb\r\n\r\nc\r",
            "# only heading",
            "#not a heading\n\n####### seven",
            "para\n# heading right after\nmore\n---\nend",
            "trailing spaces   \n  indented\t\n\n\n\nlast  ",
            "```\nnever closed\n\n  \n",
            "~~~~\ninner ~~~\n~~~\nstill inner\n~~~~\nafter",
            "```\ncode with trailing space   \n   \n```\n",
            "text\n```\ncode\n```\ntext",
            "    ```\nindented four is no fence",
            "[[Link|label]] and {{claim:abc}} and `[[code]]` and \\[[escaped]]",
            "Göteborg ß ñ 日本語\n\n* item\n* [[Åsa Öberg]]",
            "---\n---\n---",
            "> q\n> q2\n\n1. x\n   y\n\n   z",
            "| a | b |\n|---|---|\n| 1 | 2 |",
        ] {
            roundtrip(md);
        }
    }

    #[test]
    fn random_markdown_round_trips() {
        let parts = [
            "# h",
            "para",
            "",
            "",
            "- l",
            "> q",
            "```",
            "~~~",
            "---",
            "  x  ",
            "| t |",
            "[[a]]",
            "{{claim:1}}",
            "`",
            "\t",
            "## ",
            "1. n",
            "***",
            "é",
            "````",
        ];
        let mut state = 22u64;
        for _ in 0..3000 {
            let mut md = String::new();
            state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            for _ in 0..(state >> 60) {
                state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                md.push_str(parts[(state >> 33) as usize % parts.len()]);
                md.push('\n');
            }
            roundtrip(&md);
        }
    }

    #[test]
    fn links_and_claims_parse_and_bad_ones_stay_text() {
        let i = parse_inline(BlockKind::Paragraph, "See [[Acme Pump|the pump]], {{claim:0192ab}} and [[Göta Älv]].");
        assert_eq!(
            i.links,
            vec![
                WikiLink {
                    target: "Acme Pump".into(),
                    slug: "acme-pump".into(),
                    label: Some("the pump".into())
                },
                WikiLink {
                    target: "Göta Älv".into(),
                    slug: "gota-alv".into(),
                    label: None
                }
            ]
        );
        assert_eq!(i.claims, vec![ObjectId("0192ab".into())]);
        assert_eq!(i.plain, "See the pump,  and Göta Älv.");
        for bad in [
            "[[]]",
            "[[ ]]",
            "[[?!]]",
            "[[open",
            "[[a[b]]",
            "{{claim:}}",
            "{{claim:a b}}",
            "{{claim:open",
            "\\[[esc]]",
            "\\{{claim:a}}",
            "`[[code]]`",
            "`{{claim:a}}`",
        ] {
            let i = parse_inline(BlockKind::Paragraph, bad);
            assert!(i.links.is_empty() && i.claims.is_empty(), "{bad}");
        }
        assert!(parse_inline(BlockKind::Code, "```\n[[not]] {{claim:x}}\n```").links.is_empty());
        assert_eq!(parse_inline(BlockKind::Code, "```\ncode\n```").plain, "code");
    }

    #[test]
    fn slugs_are_stable_and_refuse_nothing_but_nothing() {
        assert_eq!(slugify("Hello,  World!").as_deref(), Some("hello-world"));
        assert_eq!(slugify("Göteborg").as_deref(), Some("goteborg"));
        assert_eq!(slugify("日本語 ページ").as_deref(), Some("日本語-ページ"));
        assert_eq!(slugify("?!"), None);
        assert_eq!(slugify("a").as_deref().map(slug_is_valid), Some(true));
        assert!(!slug_is_valid("Has Space"));
        let long = "ä".repeat(500);
        assert!(slugify(&long).unwrap().len() <= MAX_SLUG_BYTES);
    }

    fn matched(old: &[&str], new: &[&str]) -> Vec<Option<usize>> {
        match_blocks(old, new)
    }

    #[test]
    fn blocks_keep_their_identity_across_edits() {
        assert_eq!(matched(&["a", "b", "c"], &["a", "b", "c"]), [Some(0), Some(1), Some(2)]);
        assert_eq!(
            matched(&["a", "b", "c"], &["a", "x", "c"]),
            [Some(0), Some(1), Some(2)],
            "an edit in place keeps the id"
        );
        assert_eq!(matched(&["a", "c"], &["a", "b", "c"]), [Some(0), None, Some(1)], "an insert gets a new id");
        assert_eq!(matched(&["a", "b", "c"], &["a", "c"]), [Some(0), Some(2)], "a delete loses one");
        assert_eq!(matched(&["a", "b", "c"], &["c", "a", "b"]), [Some(2), Some(0), Some(1)], "a move keeps the id");
        assert_eq!(matched(&[], &["a"]), [None]);
        assert!(matched(&["a"], &[]).is_empty());
        assert_eq!(matched(&["x", "x"], &["x", "x", "x"]), [Some(0), Some(1), None], "duplicates pair off once");
    }

    #[test]
    fn a_document_survives_export_and_import() {
        let page = PageRow {
            header: header(ObjectId("p".into()), "t"),
            title: "A \"quoted\": title".into(),
            body: "# Title\n\n---\n\nbody [[x]]\n".into(),
            updated_at_ms: 0,
            slug: "a-quoted-title".into(),
            redirects: vec!["old".into()],
        };
        let d = parse_document(&export_document(&page)).unwrap();
        assert_eq!(d.title, page.title);
        assert_eq!(d.slug.as_deref(), Some("a-quoted-title"));
        assert_eq!(d.redirects, ["old"]);
        assert_eq!(d.body, page.body);
    }

    #[test]
    fn a_document_without_front_matter_uses_its_heading_and_bad_ones_say_why() {
        assert_eq!(parse_document("# Hello\n\nbody").unwrap().title, "Hello");
        for bad in [
            "no heading",
            "---\ntitle: \"x\"\n",
            "---\nnope\n---\n",
            "---\ntitle: unquoted\n---\n",
            "---\nslug: \"s\"\n---\n",
            "---\nwat: 1\n---\n",
        ] {
            assert!(parse_document(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_block_of_only_a_claim_reference_has_no_document() {
        let b = |kind, text: &str| BlockRow {
            header: header(ObjectId("b".into()), "t"),
            page_id: ObjectId("p".into()),
            position: 0,
            kind,
            text: text.into(),
            heading: Some("H".into()),
        };
        assert!(block_doc(&b(BlockKind::Paragraph, "{{claim:a}}")).is_none());
        assert!(block_doc(&b(BlockKind::Heading, "# H")).is_none());
        assert_eq!(block_doc(&b(BlockKind::Paragraph, "see [[Thing|it]]")).unwrap().text, "H\nsee it");
    }

    #[test]
    fn a_diff_says_what_moved_changed_came_and_went() {
        let rev = |n, blocks: &[(&str, &str)]| RevisionRow {
            header: header(revision_id(&ObjectId("p".into()), n), "t"),
            page_id: ObjectId("p".into()),
            revision: n,
            title: "t".into(),
            slug: "t".into(),
            blocks: blocks
                .iter()
                .map(|(id, text)| RevisionBlock {
                    id: ObjectId((*id).into()),
                    kind: BlockKind::Paragraph,
                    text: (*text).into(),
                })
                .collect(),
            changeset_id: ObjectId("c".into()),
            created_at_ms: 0,
            restored_from: None,
        };
        let a = rev(1, &[("1", "one"), ("2", "two"), ("3", "three"), ("4", "four")]);
        let b = rev(2, &[("4", "four"), ("1", "one"), ("2", "TWO"), ("5", "five")]);
        let changes: Vec<(String, &str)> = diff_revisions(&a, &b).into_iter().map(|c| (c.block_id.0, c.change)).collect();
        assert_eq!(
            changes,
            [
                ("3".to_string(), "removed"),
                ("4".to_string(), "moved"),
                ("2".to_string(), "changed"),
                ("5".to_string(), "added")
            ]
        );
    }
}
