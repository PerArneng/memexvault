//! The cache edge and its keys. Keys are pure, and each holds everything the value depends on, so a stale value is unreachable rather than invalidated.
//!
//! Every key starts with its vault and the vault's generation, which erasure bumps. Then:
//! - **Query:** the log's head and the index watermarks, or the past event for `as_of`, then the whole request. The head is what stops a package outliving a retraction before the indexes catch up.
//! - **Inference:** the model and a hash of the query or passages.
//! - **Objects:** the row's id and a stamp of its contents. A version alone isn't safe: reverting a ChangeSet restores an old version number, and the next edit uses it again for new content.
//! - **Disk:** the blob's hash, and the DPI, renderer and highlights for a page image.
use memex_types::{CacheTierStats, EvidencePackage, ObjectId, Row, SearchRequest, Table, VaultId};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;

/// What decides whether a cached read is current, in one store call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheMarks {
    pub head: Option<u64>,
    pub generation: u64,
}

/// Holds evidence packages, inference results, rows and files, each tier within its own cap. A tier that's off misses every read and drops every write.
pub trait Caches {
    /// Whether any RAM tier is on. If not, a search makes no extra store calls for caching.
    fn any_ram_tier(&self) -> bool;
    fn query_on(&self) -> bool;
    fn inference_on(&self) -> bool;
    fn objects_on(&self) -> bool;
    fn disk_on(&self) -> bool;
    fn get_package(&self, key: &str) -> Option<EvidencePackage>;
    fn put_package(&self, key: String, value: &EvidencePackage);
    /// Query embeddings and rerank scores.
    fn get_floats(&self, key: &str) -> Option<Vec<f32>>;
    fn put_floats(&self, key: String, value: &[f32]);
    fn get_row(&self, key: &str) -> Option<Row>;
    fn put_row(&self, key: String, value: &Row);
    fn get_file(&self, vault: &VaultId, key: &str) -> Option<Vec<u8>>;
    fn put_file(&self, vault: &VaultId, key: &str, value: &[u8]);
    /// Drops everything of this vault's, for erasure and for deleting the vault.
    fn purge_vault(&self, vault: &VaultId);
    fn stats(&self) -> Vec<CacheTierStats>;
}

/// Every tier off. The default until the composition root provides real ones.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoCaches;

impl Caches for NoCaches {
    fn any_ram_tier(&self) -> bool {
        false
    }
    fn query_on(&self) -> bool {
        false
    }
    fn inference_on(&self) -> bool {
        false
    }
    fn objects_on(&self) -> bool {
        false
    }
    fn disk_on(&self) -> bool {
        false
    }
    fn get_package(&self, _: &str) -> Option<EvidencePackage> {
        None
    }
    fn put_package(&self, _: String, _: &EvidencePackage) {}
    fn get_floats(&self, _: &str) -> Option<Vec<f32>> {
        None
    }
    fn put_floats(&self, _: String, _: &[f32]) {}
    fn get_row(&self, _: &str) -> Option<Row> {
        None
    }
    fn put_row(&self, _: String, _: &Row) {}
    fn get_file(&self, _: &VaultId, _: &str) -> Option<Vec<u8>> {
        None
    }
    fn put_file(&self, _: &VaultId, _: &str, _: &[u8]) {}
    fn purge_vault(&self, _: &VaultId) {}
    fn stats(&self) -> Vec<CacheTierStats> {
        vec![]
    }
}

fn hash_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(64);
    for b in Sha256::digest(bytes) {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// The start of every key of this vault. The length stops one vault's name being read as the start of another's.
pub fn vault_prefix(vault: &VaultId) -> String {
    format!("{}:{}|", vault.0.len(), vault.0)
}

fn prefixed(vault: &VaultId, generation: u64, tier: &str) -> String {
    format!("{}g{generation}|{tier}|", vault_prefix(vault))
}

/// Where the indexes this search reads have got to. A mark isn't there for an index the search doesn't use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IndexMarks {
    pub lexical: Option<Option<u64>>,
    pub vector: Option<Option<u64>>,
}

/// Every field that can change the answer goes in, and the destructuring means a new field can't be forgotten. `min_seq` and `debug` are here for completeness, though such requests aren't cached.
fn request_hash(req: &SearchRequest) -> String {
    let SearchRequest {
        text,
        mode,
        filter,
        limit,
        budget_tokens,
        cursor,
        as_of,
        min_seq,
        rerank,
        debug,
        facets,
        expand,
    } = req;
    // Debug output quotes and escapes its strings, so fields can't run together. It only has to be stable within a run, as these keys live in RAM.
    hash_hex(format!("{text:?}|{mode:?}|{filter:?}|{limit}|{budget_tokens}|{cursor:?}|{as_of:?}|{min_seq:?}|{rerank}|{debug}|{facets}|{expand:?}").as_bytes())
}

/// A search of the present: any event or index progress makes a new key.
pub fn query_key(vault: &VaultId, marks: CacheMarks, index: IndexMarks, req: &SearchRequest) -> String {
    let mut key = prefixed(vault, marks.generation, "q");
    let _ = write!(key, "h{:?}|l{:?}|v{:?}|{}", marks.head, index.lexical, index.vector, request_hash(req));
    key
}

/// A search of a past event, which later events can't change. Only erasure can, through the generation. A time isn't fixed until it has passed, so `as_of` by time isn't given this key.
pub fn past_query_key(vault: &VaultId, generation: u64, seq: u64, req: &SearchRequest) -> String {
    let mut key = prefixed(vault, generation, "p");
    let _ = write!(key, "s{seq}|{}", request_hash(req));
    key
}

pub fn embedding_key(vault: &VaultId, generation: u64, model: &str, text: &str) -> String {
    let mut key = prefixed(vault, generation, "e");
    let _ = write!(key, "{model:?}|{}", hash_hex(text.as_bytes()));
    key
}

/// The whole batch rather than each passage, since a batch is padded to its longest passage and scores could differ with the company a passage keeps.
pub fn rerank_key(vault: &VaultId, generation: u64, model: &str, query: &str, passages: &[&str]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(Sha256::digest(query.as_bytes()));
    for p in passages {
        hasher.update(Sha256::digest(p.as_bytes()));
    }
    let mut key = prefixed(vault, generation, "r");
    let _ = write!(key, "{model:?}|");
    for b in hasher.finalize() {
        let _ = write!(key, "{b:02x}");
    }
    key
}

pub fn object_key(vault: &VaultId, generation: u64, table: Table, id: &ObjectId, stamp: &str) -> String {
    let mut key = prefixed(vault, generation, "o");
    let _ = write!(key, "{table:?}|{}:{}|{stamp}", id.0.len(), id.0);
    key
}

/// The tables whose rows the objects tier holds. Others are small, or rarely read on the search path.
pub fn is_cached_table(table: Table) -> bool {
    matches!(table, Table::Segment | Table::Entity | Table::Page)
}

pub fn page_image_key(vault: &VaultId, generation: u64, blob: &str, page: u32, dpi: u32, renderer: &str, highlights: &str) -> String {
    let mut key = prefixed(vault, generation, "i");
    let _ = write!(key, "{blob}|{page}|{dpi}|{renderer:?}|{}", hash_hex(highlights.as_bytes()));
    key
}

pub fn word_table_key(vault: &VaultId, generation: u64, blob: &str) -> String {
    let mut key = prefixed(vault, generation, "w");
    key.push_str(blob);
    key
}

/// A page image as a cached file: its size, then the PNG.
pub fn encode_page_image(image: &crate::PageImage) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + image.png.len());
    out.extend_from_slice(&image.width.to_le_bytes());
    out.extend_from_slice(&image.height.to_le_bytes());
    out.extend_from_slice(&image.png);
    out
}

/// None for anything too short to be one, which counts as a miss.
pub fn decode_page_image(bytes: &[u8]) -> Option<crate::PageImage> {
    let (width, rest) = bytes.split_at_checked(4)?;
    let (height, png) = rest.split_at_checked(4)?;
    Some(crate::PageImage {
        png: png.to_vec(),
        width: u32::from_le_bytes(width.try_into().ok()?),
        height: u32::from_le_bytes(height.try_into().ok()?),
    })
}

/// A stamp of a row's contents, for a store with nothing cheaper.
pub fn stamp_of(row: &Row) -> String {
    hash_hex(format!("{row:?}").as_bytes())
}

/// Whether a search's answer may be kept. A degraded answer isn't: the next one may be whole. Debug output carries this run's timings, and `min_seq` is a question about right now.
pub fn is_cacheable(req: &SearchRequest) -> bool {
    !req.debug && req.min_seq.is_none()
}

pub fn keeps_package(package: &EvidencePackage) -> bool {
    package.degraded.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use memex_types::{AsOf, DocKind, Expand, LexicalFilter, SearchMode};

    fn vault(name: &str) -> VaultId {
        VaultId(name.into())
    }

    fn marks(head: u64, generation: u64) -> CacheMarks {
        CacheMarks { head: Some(head), generation }
    }

    fn index() -> IndexMarks {
        IndexMarks {
            lexical: Some(Some(1)),
            vector: Some(Some(1)),
        }
    }

    #[test]
    fn the_same_request_at_the_same_marks_has_the_same_key() {
        let a = query_key(&vault("v"), marks(5, 1), index(), &SearchRequest::new("hello"));
        let b = query_key(&vault("v"), marks(5, 1), index(), &SearchRequest::new("hello"));
        assert_eq!(a, b);
    }

    #[test]
    fn every_mark_changes_a_query_key() {
        let req = SearchRequest::new("q");
        let base = query_key(&vault("v"), marks(5, 1), index(), &req);
        assert_ne!(base, query_key(&vault("w"), marks(5, 1), index(), &req), "vault");
        assert_ne!(base, query_key(&vault("v"), marks(6, 1), index(), &req), "head");
        assert_ne!(base, query_key(&vault("v"), marks(5, 2), index(), &req), "generation");
        let lexical = IndexMarks {
            lexical: Some(Some(2)),
            ..index()
        };
        assert_ne!(base, query_key(&vault("v"), marks(5, 1), lexical, &req), "lexical");
        let vector = IndexMarks {
            vector: Some(Some(2)),
            ..index()
        };
        assert_ne!(base, query_key(&vault("v"), marks(5, 1), vector, &req), "vector");
        let not_used = IndexMarks { vector: None, ..index() };
        assert_ne!(base, query_key(&vault("v"), marks(5, 1), not_used, &req), "a used index against an unused one");
        let empty = CacheMarks { head: None, generation: 1 };
        assert_ne!(base, query_key(&vault("v"), empty, index(), &req), "no events");
    }

    type Change = (&'static str, Box<dyn Fn(&mut SearchRequest)>);
    type KeyOf = Box<dyn Fn(&VaultId, u64) -> String>;

    #[test]
    fn every_request_field_changes_a_query_key() {
        let base = SearchRequest::new("q");
        let changes: Vec<Change> = vec![
            ("text", Box::new(|r| r.text = "r".into())),
            ("mode", Box::new(|r| r.mode = SearchMode::Lexical)),
            ("filter kinds", Box::new(|r| r.filter.kinds = vec![DocKind::Claim])),
            ("filter tiers", Box::new(|r| r.filter.trust_tiers = vec!["x".into()])),
            ("limit", Box::new(|r| r.limit = 3)),
            ("budget", Box::new(|r| r.budget_tokens = 3)),
            ("cursor", Box::new(|r| r.cursor = Some("c".into()))),
            ("as_of", Box::new(|r| r.as_of = Some(AsOf::Seq(1)))),
            ("min_seq", Box::new(|r| r.min_seq = Some(1))),
            ("rerank", Box::new(|r| r.rerank = true)),
            ("debug", Box::new(|r| r.debug = true)),
            ("facets", Box::new(|r| r.facets = true)),
            ("expand", Box::new(|r| r.expand = Expand::On)),
        ];
        let key = |r: &SearchRequest| query_key(&vault("v"), marks(1, 0), index(), r);
        for (name, change) in changes {
            let mut r = base.clone();
            change(&mut r);
            assert_ne!(key(&base), key(&r), "{name}");
        }
    }

    #[test]
    fn text_can_not_run_into_the_next_field() {
        let mut a = SearchRequest::new("q\"|Hybrid");
        a.filter = LexicalFilter::default();
        let b = SearchRequest::new("q");
        assert_ne!(request_hash(&a), request_hash(&b));
    }

    #[test]
    fn a_vault_can_not_be_read_as_the_start_of_another() {
        let a = vault_prefix(&vault("ab"));
        let b = vault_prefix(&vault("ab|c"));
        assert!(!b.starts_with(&a));
        assert!(embedding_key(&vault("ab"), 0, "m", "t").starts_with(&a));
        assert!(!embedding_key(&vault("ab|c"), 0, "m", "t").starts_with(&a));
    }

    #[test]
    fn every_key_kind_differs_by_vault_and_generation() {
        let table = Table::Segment;
        let id = ObjectId("o".into());
        let kinds: Vec<KeyOf> = vec![
            Box::new(|v, g| query_key(v, marks(1, g), index(), &SearchRequest::new("q"))),
            Box::new(|v, g| past_query_key(v, g, 1, &SearchRequest::new("q"))),
            Box::new(|v, g| embedding_key(v, g, "m", "t")),
            Box::new(|v, g| rerank_key(v, g, "m", "q", &["p"])),
            Box::new(move |v, g| object_key(v, g, table, &id, "s")),
            Box::new(|v, g| page_image_key(v, g, "b", 1, 72, "r", "[]")),
            Box::new(|v, g| word_table_key(v, g, "b")),
        ];
        for k in &kinds {
            assert_ne!(k(&vault("a"), 1), k(&vault("b"), 1));
            assert_ne!(k(&vault("a"), 1), k(&vault("a"), 2));
        }
    }

    #[test]
    fn each_input_changes_the_other_keys() {
        let v = vault("v");
        assert_ne!(embedding_key(&v, 0, "m", "t"), embedding_key(&v, 0, "n", "t"));
        assert_ne!(embedding_key(&v, 0, "m", "t"), embedding_key(&v, 0, "m", "u"));
        assert_ne!(rerank_key(&v, 0, "m", "q", &["p"]), rerank_key(&v, 0, "n", "q", &["p"]));
        assert_ne!(rerank_key(&v, 0, "m", "q", &["p"]), rerank_key(&v, 0, "m", "r", &["p"]));
        assert_ne!(rerank_key(&v, 0, "m", "q", &["p", "o"]), rerank_key(&v, 0, "m", "q", &["o", "p"]));
        assert_ne!(rerank_key(&v, 0, "m", "qp", &[]), rerank_key(&v, 0, "m", "q", &["p"]));
        let id = ObjectId("o".into());
        assert_ne!(object_key(&v, 0, Table::Page, &id, "s"), object_key(&v, 0, Table::Entity, &id, "s"));
        assert_ne!(object_key(&v, 0, Table::Page, &id, "s"), object_key(&v, 0, Table::Page, &id, "t"));
        let image = page_image_key(&v, 0, "b", 1, 72, "r", "[]");
        assert_ne!(image, page_image_key(&v, 0, "c", 1, 72, "r", "[]"), "blob");
        assert_ne!(image, page_image_key(&v, 0, "b", 2, 72, "r", "[]"), "page");
        assert_ne!(image, page_image_key(&v, 0, "b", 1, 150, "r", "[]"), "dpi");
        assert_ne!(image, page_image_key(&v, 0, "b", 1, 72, "s", "[]"), "renderer");
        assert_ne!(image, page_image_key(&v, 0, "b", 1, 72, "r", "[1]"), "highlights");
    }

    #[test]
    fn debug_and_min_seq_requests_are_not_cached() {
        let mut r = SearchRequest::new("q");
        assert!(is_cacheable(&r));
        r.debug = true;
        assert!(!is_cacheable(&r));
        r.debug = false;
        r.min_seq = Some(1);
        assert!(!is_cacheable(&r));
    }

    #[test]
    fn a_page_image_comes_back_and_a_short_file_is_a_miss() {
        let image = crate::PageImage {
            png: vec![1, 2, 3],
            width: 7,
            height: 9,
        };
        assert_eq!(decode_page_image(&encode_page_image(&image)), Some(image));
        assert_eq!(decode_page_image(&[1, 2, 3]), None);
    }

    #[test]
    fn no_caches_keeps_nothing() {
        let c = NoCaches;
        assert!(!c.any_ram_tier());
        c.put_floats("k".into(), &[1.0]);
        assert_eq!(c.get_floats("k"), None);
        assert!(c.stats().is_empty());
    }
}
