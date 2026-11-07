//! Edge: the keyword index on tantivy. One folder per generation, under `<root>/vaults/<vault>/indexes/lexical/`.
// Reading and renaming the pointer file and listing generations is this crate's job.
#![allow(clippy::disallowed_methods)]
mod analysis;

use analysis::{EXACT, FOLDED, STEMMED, normalize, tokens};
use memex_types::{
    AttrScope, DocKind, Generation, IndexDoc, IndexHealth, IndexOp, LexicalError, LexicalFilter, LexicalHit, LexicalQuery, LexicalResults, ObjectId, QueryTerm,
    VaultId,
};
use serde_json::json;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::ops::Bound;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, ConstScoreQuery, Occur, PhraseQuery, Query, RangeQuery, TermQuery};
use tantivy::schema::{FAST, Field, INDEXED, IndexRecordOption, JsonObjectOptions, OwnedValue, STRING, Schema, TextFieldIndexing, TextOptions};
use tantivy::tokenizer::TextAnalyzer;
use tantivy::{DocAddress, Index, IndexReader, IndexWriter, ReloadPolicy, Score, Searcher, TantivyDocument, TantivyError, Term};

/// Bump this when the schema or analysis changes. An older generation is then rebuilt, not misread.
const SCHEMA_VERSION: u64 = 4;

#[derive(Debug, Clone, Copy)]
pub struct WriterBudget {
    pub threads: usize,
    pub heap_bytes: usize,
}

/// Following the log needs little. A rebuild is rare and worth more memory.
pub const FOLLOW_BUDGET: WriterBudget = WriterBudget {
    threads: 1,
    heap_bytes: 64 << 20,
};
pub const BUILD_BUDGET: WriterBudget = WriterBudget {
    threads: 4,
    heap_bytes: 512 << 20,
};

#[derive(Clone, Copy)]
struct Fields {
    id: Field,
    kind: Field,
    source: Field,
    epistemic: Field,
    doc_type: Field,
    trust_tier: Field,
    valid_from: Field,
    valid_to: Field,
    exact: Field,
    folded: Field,
    /// An entity's searchable fields, as `<type>.<field>`, with folded words. A new type adds paths, and needs no change to the schema.
    attrs: Field,
    stems: [Field; STEMMED.len()],
}

fn analyzed(name: &str) -> TextOptions {
    TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer(name)
            .set_index_option(IndexRecordOption::WithFreqsAndPositions),
    )
}

fn schema() -> (Schema, Fields) {
    let mut b = Schema::builder();
    let fields = Fields {
        id: b.add_text_field("id", STRING | FAST),
        kind: b.add_text_field("kind", STRING | FAST),
        source: b.add_text_field("source", STRING),
        epistemic: b.add_text_field("epistemic", STRING),
        doc_type: b.add_text_field("doc_type", STRING),
        trust_tier: b.add_text_field("trust_tier", STRING),
        valid_from: b.add_i64_field("valid_from", INDEXED | FAST),
        valid_to: b.add_i64_field("valid_to", INDEXED | FAST),
        exact: b.add_text_field(EXACT, analyzed(EXACT)),
        folded: b.add_text_field(FOLDED, analyzed(FOLDED)),
        attrs: b.add_json_field(
            "attrs",
            JsonObjectOptions::default().set_indexing_options(
                TextFieldIndexing::default()
                    .set_tokenizer(FOLDED)
                    .set_index_option(IndexRecordOption::WithFreqsAndPositions),
            ),
        ),
        stems: STEMMED.map(|(_, name, _)| b.add_text_field(name, analyzed(name))),
    };
    (b.build(), fields)
}

fn corrupt_or_backend(e: TantivyError) -> LexicalError {
    match e {
        TantivyError::DataCorruption(_)
        | TantivyError::OpenReadError(_)
        | TantivyError::OpenDirectoryError(_)
        | TantivyError::IoError(_)
        | TantivyError::SchemaError(_) => LexicalError::Corrupt(e.to_string()),
        other => LexicalError::Backend(other.to_string()),
    }
}

fn backend(e: impl std::fmt::Display) -> LexicalError {
    LexicalError::Backend(e.to_string())
}

/// What a commit records beside the documents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Payload {
    schema: u64,
    watermark: Option<u64>,
}

fn read_payload(index: &Index) -> Result<Option<Payload>, LexicalError> {
    let metas = index.load_metas().map_err(corrupt_or_backend)?;
    let Some(text) = metas.payload else { return Ok(None) };
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| LexicalError::Corrupt(e.to_string()))?;
    Ok(Some(Payload {
        schema: v.get("schema").and_then(|s| s.as_u64()).unwrap_or(0),
        watermark: v.get("watermark").and_then(|s| s.as_u64()),
    }))
}

/// One generation: an index folder, its reader and, while it's written to, its writer.
struct Gen {
    number: u64,
    index: Index,
    reader: IndexReader,
    fields: Fields,
    budget: WriterBudget,
    writer: Mutex<Option<IndexWriter>>,
    /// What the reader sees. Nothing until the first commit.
    payload: Mutex<Option<Payload>>,
    /// Set when reading it failed, so the projector finds out.
    suspect: Mutex<Option<String>>,
}

impl Gen {
    fn create(dir: &Path, number: u64, budget: WriterBudget) -> Result<Self, LexicalError> {
        fs::create_dir_all(dir).map_err(backend)?;
        let (schema, _) = schema();
        let index = Index::create_in_dir(dir, schema).map_err(backend)?;
        Self::wrap(index, number, budget, None)
    }

    fn open(dir: &Path, number: u64, budget: WriterBudget) -> Result<Self, LexicalError> {
        let index = Index::open_in_dir(dir).map_err(corrupt_or_backend)?;
        let payload = read_payload(&index)?;
        Self::wrap(index, number, budget, payload)
    }

    fn wrap(index: Index, number: u64, budget: WriterBudget, payload: Option<Payload>) -> Result<Self, LexicalError> {
        analysis::register(index.tokenizers());
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .map_err(corrupt_or_backend)?;
        let (_, fields) = schema();
        Ok(Self {
            number,
            index,
            reader,
            fields,
            budget,
            writer: Mutex::new(None),
            payload: Mutex::new(payload),
            suspect: Mutex::new(None),
        })
    }

    fn with_writer<T>(&self, f: impl FnOnce(&mut IndexWriter) -> Result<T, LexicalError>) -> Result<T, LexicalError> {
        let mut slot = self.writer.lock().expect("lock");
        if slot.is_none() {
            *slot = Some(
                self.index
                    .writer_with_num_threads(self.budget.threads, self.budget.heap_bytes)
                    .map_err(backend)?,
            );
        }
        f(slot.as_mut().expect("just set"))
    }

    fn write(&self, ops: &[IndexOp]) -> Result<(), LexicalError> {
        self.with_writer(|writer| {
            for op in ops {
                match op {
                    IndexOp::Upsert(doc) => {
                        writer.delete_term(Term::from_field_text(self.fields.id, &doc.id.0));
                        writer.add_document(to_document(self.fields, doc)).map_err(backend)?;
                    }
                    IndexOp::Delete(id) => {
                        writer.delete_term(Term::from_field_text(self.fields.id, &id.0));
                    }
                }
            }
            Ok(())
        })
    }

    fn commit(&self, watermark: Option<u64>) -> Result<(), LexicalError> {
        self.with_writer(|writer| {
            let payload = json!({"schema": SCHEMA_VERSION, "watermark": watermark}).to_string();
            let mut prepared = writer.prepare_commit().map_err(backend)?;
            prepared.set_payload(&payload);
            prepared.commit().map_err(backend)?;
            Ok(())
        })?;
        self.reader.reload().map_err(corrupt_or_backend)?;
        *self.payload.lock().expect("lock") = Some(Payload {
            schema: SCHEMA_VERSION,
            watermark,
        });
        Ok(())
    }

    fn close_writer(&self) {
        *self.writer.lock().expect("lock") = None;
    }

    fn committed(&self) -> bool {
        self.payload.lock().expect("lock").is_some()
    }

    fn watermark(&self) -> Option<u64> {
        self.payload.lock().expect("lock").and_then(|p| p.watermark)
    }

    fn search(&self, query: &LexicalQuery, keep_common_terms: bool) -> Result<LexicalResults, LexicalError> {
        // Read before the search, so the answer reflects at least this much.
        let watermark = self.watermark();
        let searcher = self.reader.searcher();
        let built = build_query(&self.index, &searcher, self.fields, query, keep_common_terms)?;
        let top: Vec<(Score, DocAddress)> = searcher
            .search(&built, &TopDocs::with_limit(query.limit.max(1)).order_by_score())
            .map_err(corrupt_or_backend)?;
        let mut columns = HashMap::new();
        let mut hits = Vec::with_capacity(top.len());
        let (mut id, mut kind) = (String::new(), String::new());
        for (score, address) in top.into_iter().take(query.limit) {
            if let std::collections::hash_map::Entry::Vacant(slot) = columns.entry(address.segment_ord) {
                let fast = searcher.segment_reader(address.segment_ord).fast_fields();
                let missing = || LexicalError::Corrupt("a fast field is missing".into());
                let ids = fast.str("id").map_err(corrupt_or_backend)?.ok_or_else(missing)?;
                let kinds = fast.str("kind").map_err(corrupt_or_backend)?.ok_or_else(missing)?;
                slot.insert((ids, kinds));
            }
            let (ids, kinds) = &columns[&address.segment_ord];
            read_str(ids, address.doc_id, &mut id)?;
            read_str(kinds, address.doc_id, &mut kind)?;
            let kind = DocKind::parse(&kind).ok_or_else(|| LexicalError::Corrupt(format!("unknown kind {kind}")))?;
            hits.push(LexicalHit {
                id: ObjectId(id.clone()),
                kind,
                score,
            });
        }
        hits.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
        Ok(LexicalResults { hits, watermark })
    }
}

fn read_str(column: &tantivy::columnar::StrColumn, doc: u32, out: &mut String) -> Result<(), LexicalError> {
    let ord = column
        .term_ords(doc)
        .next()
        .ok_or_else(|| LexicalError::Corrupt("a document has no id".into()))?;
    out.clear();
    column.ord_to_str(ord, out).map_err(|e| LexicalError::Corrupt(e.to_string()))?;
    Ok(())
}

fn to_document(fields: Fields, doc: &IndexDoc) -> TantivyDocument {
    let text = normalize(&doc.text);
    let mut out = TantivyDocument::new();
    out.add_text(fields.id, &doc.id.0);
    out.add_text(fields.kind, doc.kind.as_str());
    for source in &doc.source_ids {
        out.add_text(fields.source, &source.0);
    }
    if let Some(e) = &doc.epistemic {
        out.add_text(fields.epistemic, e);
    }
    if let Some(t) = &doc.doc_type {
        out.add_text(fields.doc_type, t);
    }
    for tier in &doc.trust_tiers {
        out.add_text(fields.trust_tier, tier);
    }
    out.add_i64(fields.valid_from, doc.valid_from_ms.unwrap_or(i64::MIN));
    out.add_i64(fields.valid_to, doc.valid_to_ms.unwrap_or(i64::MAX));
    out.add_text(fields.exact, &text);
    out.add_text(fields.folded, &text);
    if let Some(language) = &doc.language
        && let Some(i) = STEMMED.iter().position(|(code, _, _)| code == language)
    {
        out.add_text(fields.stems[i], &text);
    }
    if !doc.attrs.is_empty() {
        let mut by_type: BTreeMap<String, BTreeMap<String, OwnedValue>> = BTreeMap::new();
        for a in &doc.attrs {
            by_type
                .entry(a.type_name.clone())
                .or_default()
                .insert(a.field.clone(), OwnedValue::Str(normalize(&a.text)));
        }
        out.add_object(
            fields.attrs,
            by_type.into_iter().map(|(t, f)| (t, OwnedValue::Object(f.into_iter().collect()))).collect(),
        );
    }
    out
}

fn analyzer(index: &Index, name: &str) -> Result<TextAnalyzer, LexicalError> {
    index.tokenizers().get(name).ok_or_else(|| LexicalError::Backend(format!("no analyzer {name}")))
}

/// One word or phrase on one field: a term for one token, a phrase for several, and nothing for none.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Clause {
    One(Term),
    Phrase(Vec<Term>),
}

fn field_clause(field: Field, words: &[String]) -> Option<Clause> {
    let mut terms: Vec<Term> = words.iter().map(|t| Term::from_field_text(field, t)).collect();
    match terms.len() {
        0 => None,
        1 => Some(Clause::One(terms.remove(0))),
        _ => Some(Clause::Phrase(terms)),
    }
}

/// One word or phrase within one entity field. Type and field names have no dots, so the path can't be misread.
fn attr_clause(field: Field, scope: &AttrScope, words: &[String]) -> Option<Clause> {
    let path = format!("{}.{}", scope.type_name, scope.field);
    let mut terms: Vec<Term> = words
        .iter()
        .map(|w| {
            let mut term = Term::from_field_json_path(field, &path, false);
            term.append_type_and_str(w);
            term
        })
        .collect();
    match terms.len() {
        0 => None,
        1 => Some(Clause::One(terms.remove(0))),
        _ => Some(Clause::Phrase(terms)),
    }
}

impl Clause {
    fn into_query(self) -> Box<dyn Query> {
        match self {
            Self::One(term) => Box::new(TermQuery::new(term, IndexRecordOption::WithFreqs)),
            Self::Phrase(terms) => Box::new(PhraseQuery::new(terms)),
        }
    }
}

/// Words in more than this share of documents say little about what's being asked for, and reading their long lists is most of a long query's cost.
const COMMON_TERM_SHARE: f64 = 0.04;
/// Below this many documents speed doesn't matter, and every word counts.
const COMMON_TERM_MIN_DOCS: u64 = 10_000;

/// Keeps the words that carry the query. If every word is common, as in `to be or not to be`, all of them stay.
fn without_common_terms(clauses: Vec<Clause>, searcher: &Searcher) -> Result<Vec<Clause>, LexicalError> {
    let docs = searcher.num_docs();
    if docs < COMMON_TERM_MIN_DOCS {
        return Ok(clauses);
    }
    let limit = (docs as f64 * COMMON_TERM_SHARE) as u64;
    let mut kept = Vec::with_capacity(clauses.len());
    for clause in &clauses {
        let common = match clause {
            Clause::One(term) => searcher.doc_freq(term).map_err(corrupt_or_backend)? > limit,
            Clause::Phrase(_) => false,
        };
        if !common {
            kept.push(clause.clone());
        }
    }
    Ok(if kept.is_empty() { clauses } else { kept })
}

fn filter_clauses(fields: Fields, filter: &LexicalFilter) -> Vec<(Occur, Box<dyn Query>)> {
    fn any_of(field: Field, values: impl Iterator<Item = String>) -> Box<dyn Query> {
        let clauses: Vec<(Occur, Box<dyn Query>)> = values
            .map(|v| {
                let term: Box<dyn Query> = Box::new(TermQuery::new(Term::from_field_text(field, &v), IndexRecordOption::Basic));
                (Occur::Should, term)
            })
            .collect();
        Box::new(BooleanQuery::new(clauses))
    }
    let mut out: Vec<Box<dyn Query>> = vec![];
    if !filter.kinds.is_empty() {
        out.push(any_of(fields.kind, filter.kinds.iter().map(|k| k.as_str().to_string())));
    }
    if !filter.source_ids.is_empty() {
        out.push(any_of(fields.source, filter.source_ids.iter().map(|s| s.0.clone())));
    }
    if !filter.epistemic.is_empty() {
        out.push(any_of(fields.epistemic, filter.epistemic.iter().cloned()));
    }
    if !filter.types.is_empty() {
        out.push(any_of(fields.doc_type, filter.types.iter().cloned()));
    }
    if !filter.trust_tiers.is_empty() {
        out.push(any_of(fields.trust_tier, filter.trust_tiers.iter().cloned()));
    }
    if let Some(t) = filter.valid_at_ms {
        out.push(Box::new(RangeQuery::new(
            Bound::Unbounded,
            Bound::Included(Term::from_field_i64(fields.valid_from, t)),
        )));
        out.push(Box::new(RangeQuery::new(
            Bound::Excluded(Term::from_field_i64(fields.valid_to, t)),
            Bound::Unbounded,
        )));
    }
    out.into_iter()
        .map(|q| (Occur::Must, Box::new(ConstScoreQuery::new(q, 0.0)) as Box<dyn Query>))
        .collect()
}

/// A query runs against every field, because its language can't be reliably detected.
fn build_query(index: &Index, searcher: &Searcher, fields: Fields, query: &LexicalQuery, keep_common_terms: bool) -> Result<Box<dyn Query>, LexicalError> {
    let mut exact = analyzer(index, EXACT)?;
    let mut folded = analyzer(index, FOLDED)?;
    let mut stems = vec![];
    for (i, (_, name, _)) in STEMMED.iter().enumerate() {
        stems.push((fields.stems[i], analyzer(index, name)?));
    }
    let mut clauses: Vec<Clause> = vec![];
    let mut seen = HashSet::new();
    let mut add = |clause: Option<Clause>| {
        if let Some(c) = clause
            && seen.insert(c.clone())
        {
            clauses.push(c);
        }
    };
    for term in &query.terms {
        let (text, is_phrase) = match term {
            QueryTerm::Word(w) => (normalize(w), false),
            QueryTerm::Phrase(words) => (normalize(&words.join(" ")), true),
        };
        if let Some(scope) = &query.filter.attr {
            add(attr_clause(fields.attrs, scope, &tokens(&mut folded, &text)));
            continue;
        }
        // A document spelled as asked matches here and in the folded field, so it beats one matching only after folding, with no boost needed.
        add(field_clause(fields.exact, &tokens(&mut exact, &text)));
        let folded_tokens = tokens(&mut folded, &text);
        // A word that splits into parts is an identifier, and stemming its parts would only add noise.
        let stemmable = is_phrase || folded_tokens.len() == 1;
        add(field_clause(fields.folded, &folded_tokens));
        if stemmable {
            for (field, analyzer) in &mut stems {
                add(field_clause(*field, &tokens(analyzer, &text)));
            }
        }
    }
    // A plain union of terms also lets the engine skip documents that can't reach the top.
    let clauses = if keep_common_terms {
        clauses
    } else {
        without_common_terms(clauses, searcher)?
    };
    let should: Vec<(Occur, Box<dyn Query>)> = clauses.into_iter().map(|c| (Occur::Should, c.into_query())).collect();
    let matching: Box<dyn Query> = Box::new(BooleanQuery::new(should));
    let filters = filter_clauses(fields, &query.filter);
    if filters.is_empty() {
        return Ok(matching);
    }
    let mut all = vec![(Occur::Must, matching)];
    all.extend(filters);
    Ok(Box::new(BooleanQuery::new(all)))
}

#[derive(Default)]
struct VaultIndex {
    /// Whether leftovers have been cleared and the pointer read.
    opened: bool,
    live: Option<Arc<Gen>>,
    /// Why the live generation couldn't be opened.
    open_failure: Option<String>,
    previous: Option<u64>,
    building: Option<Arc<Gen>>,
}

/// Tantivy behind the shape the core trait wants. The binary crate adapts it.
pub struct TantivyLexical {
    root: PathBuf,
    vaults: Mutex<HashMap<VaultId, VaultIndex>>,
    follow: WriterBudget,
    build: WriterBudget,
    keep_common_terms: bool,
}

fn safe_vault(vault: &VaultId) -> Result<(), LexicalError> {
    let ok = !vault.0.is_empty() && vault.0.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(LexicalError::Invalid(format!("the vault id {} can't name a folder", vault.0)))
    }
}

fn gen_dir(base: &Path, number: u64) -> PathBuf {
    base.join(format!("gen-{number}"))
}

fn gen_numbers(base: &Path) -> Result<Vec<u64>, LexicalError> {
    let mut out = vec![];
    let entries = match fs::read_dir(base) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(backend(e)),
    };
    for entry in entries {
        let name = entry.map_err(backend)?.file_name();
        if let Some(n) = name.to_str().and_then(|n| n.strip_prefix("gen-")).and_then(|n| n.parse().ok()) {
            out.push(n);
        }
    }
    Ok(out)
}

/// Replaces the pointer atomically. It's the one thing that decides which generation is live.
fn write_pointer(base: &Path, live: u64, previous: Option<u64>) -> Result<(), LexicalError> {
    let tmp = base.join("CURRENT.tmp");
    let mut file = fs::File::create(&tmp).map_err(backend)?;
    file.write_all(json!({"live": live, "previous": previous}).to_string().as_bytes())
        .map_err(backend)?;
    file.sync_all().map_err(backend)?;
    drop(file);
    fs::rename(&tmp, base.join("CURRENT")).map_err(backend)?;
    fs::File::open(base).and_then(|d| d.sync_all()).map_err(backend)
}

fn read_pointer(base: &Path) -> Result<Option<(u64, Option<u64>)>, LexicalError> {
    let text = match fs::read_to_string(base.join("CURRENT")) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(backend(e)),
    };
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| LexicalError::Corrupt(format!("the pointer file: {e}")))?;
    let live = v
        .get("live")
        .and_then(|l| l.as_u64())
        .ok_or_else(|| LexicalError::Corrupt("the pointer file names no generation".into()))?;
    Ok(Some((live, v.get("previous").and_then(|p| p.as_u64()))))
}

impl TantivyLexical {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::with_budgets(root, FOLLOW_BUDGET, BUILD_BUDGET)
    }

    pub fn with_budgets(root: impl Into<PathBuf>, follow: WriterBudget, build: WriterBudget) -> Self {
        Self {
            root: root.into(),
            vaults: Mutex::new(HashMap::new()),
            follow,
            build,
            keep_common_terms: false,
        }
    }

    /// Switches off the common-term rule, to measure what the rule costs in recall. Production never does this.
    pub fn keeping_common_terms(mut self) -> Self {
        self.keep_common_terms = true;
        self
    }

    /// The folder holding a vault's generations.
    pub fn vault_dir(&self, vault: &VaultId) -> PathBuf {
        self.root.join("vaults").join(&vault.0).join("indexes").join("lexical")
    }

    /// Runs `f` on the vault's state, opening it the first time.
    fn with_vault<T>(&self, vault: &VaultId, f: impl FnOnce(&mut VaultIndex, &Path) -> Result<T, LexicalError>) -> Result<T, LexicalError> {
        safe_vault(vault)?;
        let base = self.vault_dir(vault);
        let mut vaults = self.vaults.lock().expect("lock");
        let entry = vaults.entry(vault.clone()).or_default();
        if !entry.opened {
            self.open_vault(entry, &base)?;
        }
        f(entry, &base)
    }

    /// Reads the pointer and clears what a crash left behind.
    fn open_vault(&self, entry: &mut VaultIndex, base: &Path) -> Result<(), LexicalError> {
        let mut keep = vec![];
        match read_pointer(base) {
            Ok(None) => {}
            Ok(Some((live, previous))) => {
                keep.push(live);
                keep.extend(previous);
                entry.previous = previous;
                match Gen::open(&gen_dir(base, live), live, self.follow) {
                    Ok(g) => entry.live = Some(Arc::new(g)),
                    Err(e) => entry.open_failure = Some(e.to_string()),
                }
            }
            Err(LexicalError::Corrupt(m)) => entry.open_failure = Some(m),
            Err(e) => return Err(e),
        }
        // With a damaged pointer nothing is certain, so nothing gets deleted until a rebuild replaces it.
        if entry.open_failure.is_none() {
            for n in gen_numbers(base)? {
                if !keep.contains(&n) {
                    fs::remove_dir_all(gen_dir(base, n)).map_err(backend)?;
                }
            }
        }
        entry.opened = true;
        Ok(())
    }

    fn generation(&self, vault: &VaultId, generation: Generation) -> Result<Arc<Gen>, LexicalError> {
        self.with_vault(vault, |v, _| {
            match generation {
                Generation::Live => v.live.clone(),
                Generation::Building => v.building.clone(),
            }
            .ok_or_else(|| LexicalError::Invalid(format!("no {generation:?} generation")))
        })
    }

    pub fn health(&self, vault: &VaultId, deep: bool) -> Result<IndexHealth, LexicalError> {
        let (live, failure) = self.with_vault(vault, |v, _| Ok((v.live.clone(), v.open_failure.clone())))?;
        if let Some(detail) = failure {
            return Ok(IndexHealth::Corrupt(detail));
        }
        let Some(live) = live else { return Ok(IndexHealth::Missing) };
        if let Some(detail) = live.suspect.lock().expect("lock").clone() {
            return Ok(IndexHealth::Corrupt(detail));
        }
        let payload = *live.payload.lock().expect("lock");
        let Some(payload) = payload else {
            return Ok(IndexHealth::Corrupt("the live generation has no commit".into()));
        };
        if payload.schema != SCHEMA_VERSION {
            return Ok(IndexHealth::Outdated);
        }
        if deep {
            match live.index.validate_checksum() {
                Ok(damaged) if damaged.is_empty() => {}
                Ok(damaged) => return Ok(IndexHealth::Corrupt(format!("{} files fail their checksum", damaged.len()))),
                Err(e) => return Ok(IndexHealth::Corrupt(e.to_string())),
            }
        }
        Ok(IndexHealth::Ready { watermark: payload.watermark })
    }

    pub fn watermark(&self, vault: &VaultId, generation: Generation) -> Result<Option<u64>, LexicalError> {
        Ok(self.generation(vault, generation)?.watermark())
    }

    pub fn begin_build(&self, vault: &VaultId) -> Result<(), LexicalError> {
        let budget = self.build;
        self.with_vault(vault, |v, base| {
            if let Some(old) = v.building.take() {
                old.close_writer();
                fs::remove_dir_all(gen_dir(base, old.number)).map_err(backend)?;
            }
            let number = gen_numbers(base)?.into_iter().max().map_or(1, |n| n + 1);
            v.building = Some(Arc::new(Gen::create(&gen_dir(base, number), number, budget)?));
            Ok(())
        })
    }

    pub fn write(&self, vault: &VaultId, generation: Generation, ops: &[IndexOp]) -> Result<(), LexicalError> {
        self.generation(vault, generation)?.write(ops)
    }

    pub fn commit(&self, vault: &VaultId, generation: Generation, watermark: Option<u64>) -> Result<(), LexicalError> {
        self.generation(vault, generation)?.commit(watermark)
    }

    pub fn flip(&self, vault: &VaultId) -> Result<(), LexicalError> {
        let follow = self.follow;
        self.with_vault(vault, |v, base| {
            let Some(built) = v.building.clone().filter(|b| b.committed()) else {
                return Err(LexicalError::Invalid("no committed generation to flip to".into()));
            };
            let old = v.live.as_ref().map(|g| g.number);
            built.close_writer();
            write_pointer(base, built.number, old)?;
            // Its writer had the build's budget, so the next write opens one with the follower's.
            let reopened = Gen::open(&gen_dir(base, built.number), built.number, follow)?;
            if let Some(g) = v.live.take() {
                g.close_writer();
            }
            for n in gen_numbers(base)? {
                if n != built.number && Some(n) != old {
                    let _ = fs::remove_dir_all(gen_dir(base, n));
                }
            }
            v.live = Some(Arc::new(reopened));
            v.previous = old;
            v.building = None;
            v.open_failure = None;
            Ok(())
        })
    }

    pub fn rollback(&self, vault: &VaultId) -> Result<(), LexicalError> {
        let follow = self.follow;
        self.with_vault(vault, |v, base| {
            let Some(previous) = v.previous else {
                return Err(LexicalError::Invalid("no earlier generation to go back to".into()));
            };
            let restored = Gen::open(&gen_dir(base, previous), previous, follow)?;
            write_pointer(base, previous, None)?;
            if let Some(g) = v.live.take() {
                g.close_writer();
                let _ = fs::remove_dir_all(gen_dir(base, g.number));
            }
            v.live = Some(Arc::new(restored));
            v.previous = None;
            v.open_failure = None;
            Ok(())
        })
    }

    pub fn search(&self, vault: &VaultId, query: &LexicalQuery) -> Result<LexicalResults, LexicalError> {
        let (live, failure) = self.with_vault(vault, |v, _| Ok((v.live.clone(), v.open_failure.clone())))?;
        if let Some(detail) = failure {
            return Err(LexicalError::Corrupt(detail));
        }
        let live = live.ok_or_else(|| LexicalError::Unavailable("it hasn't been built yet".into()))?;
        // Damaged files can make the engine panic as well as return an error. Either way it's this generation's problem, not the server's.
        let result = catch_unwind(AssertUnwindSafe(|| live.search(query, self.keep_common_terms)))
            .unwrap_or_else(|_| Err(LexicalError::Corrupt("reading the index panicked".into())));
        if let Err(LexicalError::Corrupt(detail)) = &result {
            *live.suspect.lock().expect("lock") = Some(detail.clone());
        }
        result
    }

    pub fn delete_vault(&self, vault: &VaultId) -> Result<(), LexicalError> {
        safe_vault(vault)?;
        if let Some(v) = self.vaults.lock().expect("lock").remove(vault) {
            for g in v.live.iter().chain(v.building.iter()) {
                g.close_writer();
            }
        }
        match fs::remove_dir_all(self.vault_dir(vault)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(backend(e)),
        }
    }
}
