//! The keyword index on real tantivy files.
// These tests make and damage files.
#![allow(clippy::disallowed_methods)]
use memex_core::testing::{ManualClock, MemStore, lexical_contract};
use memex_core::{Engine, LexicalIndex, LexicalProjector, StateRecorded, StepOutcome, Store};
use memex_lexical::{TantivyLexical, WriterBudget};
use memex_types::{
    DocKind, EventDraft, Generation, Header, IndexDoc, IndexHealth, IndexOp, LexicalError, LexicalFilter, LexicalQuery, Mutation, ObjectId, QueryTerm, Row,
    SegmentRow, Status, VaultId,
};
use memexvault::edges::TantivyIndex;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const SMALL: WriterBudget = WriterBudget {
    threads: 1,
    heap_bytes: 20 << 20,
};

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("memex-lexical-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn index(dir: &Path) -> TantivyIndex {
    TantivyIndex(TantivyLexical::with_budgets(dir, SMALL, SMALL))
}

fn v() -> VaultId {
    VaultId("v".into())
}

fn doc(id: &str, language: Option<&str>, text: &str) -> IndexOp {
    IndexOp::Upsert(IndexDoc {
        id: ObjectId(id.into()),
        kind: DocKind::Segment,
        language: language.map(String::from),
        text: text.into(),
        source_ids: vec![],
        epistemic: None,
        valid_from_ms: None,
        valid_to_ms: None,
        doc_type: None,
        trust_tiers: vec![],
        attrs: vec![],
    })
}

fn built(l: &TantivyIndex, ops: &[IndexOp], watermark: Option<u64>) {
    l.begin_build(&v()).unwrap();
    l.write(&v(), Generation::Building, ops).unwrap();
    l.commit(&v(), Generation::Building, watermark).unwrap();
    l.flip(&v()).unwrap();
}

fn search(l: &TantivyIndex, words: &[&str]) -> Vec<String> {
    let query = LexicalQuery {
        terms: words.iter().map(|w| QueryTerm::Word((*w).into())).collect(),
        filter: LexicalFilter::default(),
        limit: 100,
    };
    l.search(&v(), &query).unwrap().hits.into_iter().map(|h| h.id.0).collect()
}

fn sorted(mut ids: Vec<String>) -> Vec<String> {
    ids.sort();
    ids
}

#[test]
fn tantivy_meets_the_contract() {
    let dir = temp("contract");
    lexical_contract::run_all(&index(&dir), "c-");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn an_inflected_form_finds_its_base_form_in_each_language() {
    let dir = temp("stems");
    let l = index(&dir);
    built(
        &l,
        &[
            doc("sv", Some("sv"), "Vi köpte en bil i Lund"),
            doc("de", Some("de"), "Das kleine Haus steht am Fluss"),
            doc("es", Some("es"), "El niño juega en el parque"),
            doc("en", Some("en"), "The engine was running hot"),
            doc("none", None, "En annan bilar utan språk"),
        ],
        Some(0),
    );
    assert_eq!(search(&l, &["bilarna"]), ["sv"], "Swedish, and a document of unknown language isn't stemmed");
    assert_eq!(search(&l, &["Hauses"]), ["de"], "German");
    assert_eq!(search(&l, &["niños"]), ["es"], "Spanish");
    assert_eq!(search(&l, &["run"]), ["en"], "English");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn accents_are_forgiven_and_the_exact_spelling_ranks_first() {
    let dir = temp("fold");
    let l = index(&dir);
    built(
        &l,
        &[
            doc("exact", Some("sv"), "Resan gick till Göteborg i maj"),
            doc("plain", Some("sv"), "Resan gick till Goteborg i maj"),
            doc("strasse", Some("de"), "Die Straße ist lang"),
            doc("nino", Some("es"), "Un niño pequeño"),
        ],
        Some(0),
    );
    assert_eq!(sorted(search(&l, &["Goteborg"])), ["exact", "plain"]);
    assert_eq!(search(&l, &["Göteborg"])[0], "exact");
    assert_eq!(search(&l, &["Goteborg"])[0], "plain");
    assert_eq!(search(&l, &["strasse"]), ["strasse"]);
    assert_eq!(search(&l, &["nino"]), ["nino"]);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn text_decomposed_or_with_invisible_characters_still_matches() {
    let dir = temp("nfc");
    let l = index(&dir);
    built(
        &l,
        &[
            doc("nfd", None, "Staden Go\u{308}teborg"),
            doc("soft", None, "Infor\u{ad}mation om\u{200b}falling"),
        ],
        Some(0),
    );
    assert_eq!(search(&l, &["Göteborg"]), ["nfd"], "a decomposed document, a composed query");
    assert_eq!(search(&l, &["Go\u{308}teborg"]), ["nfd"], "and a decomposed query");
    assert_eq!(search(&l, &["Information"]), ["soft"]);
    assert_eq!(search(&l, &["omfalling"]), ["soft"]);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_part_of_an_identifier_can_find_it_but_a_sibling_cannot() {
    let dir = temp("ident");
    let l = index(&dir);
    built(
        &l,
        &[
            doc("b", None, "part XK-42/B here"),
            doc("c", None, "part XK-42/C here"),
            doc("n", None, "part 42 is here"),
        ],
        Some(0),
    );
    assert_eq!(search(&l, &["XK-42/B"]), ["b"]);
    assert_eq!(sorted(search(&l, &["XK-42"])), ["b", "c"], "a prefix of the parts matches both, in order");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn it_survives_a_restart() {
    let dir = temp("restart");
    {
        let l = index(&dir);
        built(&l, &[doc("a", None, "word")], Some(3));
        l.write(&v(), Generation::Live, &[doc("b", None, "word")]).unwrap();
        l.commit(&v(), Generation::Live, Some(4)).unwrap();
        l.write(&v(), Generation::Live, &[doc("uncommitted", None, "word")]).unwrap();
    }
    let l = index(&dir);
    assert_eq!(l.health(&v(), true).unwrap(), IndexHealth::Ready { watermark: Some(4) });
    assert_eq!(
        sorted(search(&l, &["word"])),
        ["a", "b"],
        "an uncommitted write isn't there, and the log has it"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn an_unfinished_build_is_cleared_at_startup() {
    let dir = temp("leftover");
    let base = {
        let l = index(&dir);
        built(&l, &[doc("a", None, "word")], Some(1));
        l.begin_build(&v()).unwrap();
        l.write(&v(), Generation::Building, &[doc("half", None, "word")]).unwrap();
        l.0.vault_dir(&v())
    };
    let generations = || {
        fs::read_dir(&base)
            .unwrap()
            .filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().starts_with("gen-"))
            .count()
    };
    assert_eq!(generations(), 2);
    let l = index(&dir);
    assert_eq!(search(&l, &["word"]), ["a"]);
    assert_eq!(generations(), 1);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn only_the_live_and_previous_generations_stay_on_disk() {
    let dir = temp("retention");
    let l = index(&dir);
    for w in 1..=4 {
        built(&l, &[doc("a", None, "word")], Some(w));
    }
    let names: Vec<String> = fs::read_dir(l.0.vault_dir(&v()))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(sorted(names), ["CURRENT", "gen-3", "gen-4"]);
    fs::remove_dir_all(dir).unwrap();
}

/// The largest data file in the live generation.
fn a_data_file(dir: &Path, l: &TantivyIndex) -> PathBuf {
    let current: serde_json::Value = serde_json::from_str(&fs::read_to_string(l.0.vault_dir(&v()).join("CURRENT")).unwrap()).unwrap();
    let live = l.0.vault_dir(&v()).join(format!("gen-{}", current["live"]));
    assert!(dir.exists());
    fs::read_dir(live)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "idx" || x == "pos" || x == "term"))
        .max_by_key(|p| fs::metadata(p).unwrap().len())
        .unwrap()
}

fn recovers(dir: &Path) {
    let store = MemStore::new();
    let lease = store.open_vault(&v()).unwrap();
    let engine = Engine::new(store, ManualClock::starting_at(0));
    let row = Row::Segment(SegmentRow {
        header: Header {
            id: ObjectId("from-log".into()),
            creator: "t".into(),
            status: Status::Active,
            version: 1,
            acl_id: None,
        },
        source_id: ObjectId("src".into()),
        language: None,
        text: "word".into(),
        rendition_id: None,
        start: 0,
        end: 4,
        heading: None,
        page_from: None,
        page_to: None,
    });
    let draft = EventDraft {
        actor: "t".into(),
        kind: "state.recorded".into(),
        schema_version: 1,
        payload: serde_json::to_value(StateRecorded {
            mutations: vec![Mutation::Put(row)],
        })
        .unwrap(),
    };
    engine.apply(&lease, draft).unwrap();
    let l = index(dir);
    let mut projector = LexicalProjector::default();
    let mut idle = 0;
    for _ in 0..100 {
        engine.clock().advance(Duration::from_secs(1));
        idle = if projector.step(engine.store(), &l, engine.clock(), &v()).unwrap() == StepOutcome::Idle {
            idle + 1
        } else {
            0
        };
        if idle == 2 {
            break;
        }
    }
    assert_eq!(l.health(&v(), true).unwrap(), IndexHealth::Ready { watermark: Some(0) });
    assert_eq!(search(&l, &["word"]), ["from-log"]);
}

#[test]
fn a_damaged_data_file_is_detected_and_rebuilt() {
    let dir = temp("damaged");
    let path = {
        let l = index(&dir);
        built(&l, &[doc("old", None, "word word word"), doc("old2", None, "other word")], Some(9));
        a_data_file(&dir, &l)
    };
    let mut bytes = fs::read(&path).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0xff;
    fs::write(&path, bytes).unwrap();
    let l = index(&dir);
    assert!(matches!(l.health(&v(), true).unwrap(), IndexHealth::Corrupt(_)), "the deep check finds it");
    drop(l);
    recovers(&dir);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_truncated_data_file_is_detected_and_rebuilt() {
    let dir = temp("truncated");
    let path = {
        let l = index(&dir);
        built(&l, &[doc("old", None, "word")], Some(9));
        a_data_file(&dir, &l)
    };
    fs::write(&path, b"x").unwrap();
    let l = index(&dir);
    assert!(matches!(l.health(&v(), true).unwrap(), IndexHealth::Corrupt(_)));
    drop(l);
    recovers(&dir);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_damaged_pointer_or_metadata_is_detected_and_rebuilt() {
    let dir = temp("pointer");
    let base = {
        let l = index(&dir);
        built(&l, &[doc("old", None, "word")], Some(9));
        l.0.vault_dir(&v())
    };
    fs::write(base.join("CURRENT"), b"{ not json").unwrap();
    let l = index(&dir);
    assert!(matches!(l.health(&v(), false).unwrap(), IndexHealth::Corrupt(_)));
    assert!(matches!(
        l.search(
            &v(),
            &LexicalQuery {
                terms: vec![QueryTerm::Word("word".into())],
                filter: LexicalFilter::default(),
                limit: 1
            }
        ),
        Err(LexicalError::Corrupt(_))
    ));
    drop(l);
    recovers(&dir);

    let dir2 = temp("metadata");
    let base = {
        let l = index(&dir2);
        built(&l, &[doc("old", None, "word")], Some(9));
        l.0.vault_dir(&v())
    };
    fs::write(base.join("gen-1").join("meta.json"), b"garbage").unwrap();
    let l = index(&dir2);
    assert!(matches!(l.health(&v(), false).unwrap(), IndexHealth::Corrupt(_)));
    drop(l);
    recovers(&dir2);
    fs::remove_dir_all(dir).unwrap();
    fs::remove_dir_all(dir2).unwrap();
}

#[test]
fn a_generation_from_an_older_schema_is_rebuilt() {
    let dir = temp("outdated");
    let base = {
        let l = index(&dir);
        built(&l, &[doc("old", None, "word")], Some(9));
        l.0.vault_dir(&v())
    };
    let meta = base.join("gen-1").join("meta.json");
    let text = fs::read_to_string(&meta).unwrap();
    let older = text.replace("\\\"schema\\\":2", "\\\"schema\\\":1");
    assert_ne!(text, older, "the payload's format changed, so this test needs updating");
    fs::write(&meta, older).unwrap();
    let l = index(&dir);
    assert_eq!(l.health(&v(), false).unwrap(), IndexHealth::Outdated);
    drop(l);
    recovers(&dir);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_vault_id_cannot_escape_the_data_folder() {
    let dir = temp("escape");
    let l = index(&dir);
    for bad in ["../up", "a/b", "", ".."] {
        let vault = VaultId(bad.into());
        assert!(matches!(l.begin_build(&vault), Err(LexicalError::Invalid(_))), "{bad}");
        assert!(matches!(l.delete_vault(&vault), Err(LexicalError::Invalid(_))), "{bad}");
    }
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn searching_during_commits_never_fails() {
    let dir = temp("concurrent");
    let l = std::sync::Arc::new(index(&dir));
    built(&l, &[doc("seed", None, "word")], Some(0));
    let reader = {
        let l = l.clone();
        std::thread::spawn(move || {
            let mut found = 0;
            for _ in 0..500 {
                found += search(&l, &["word"]).len();
            }
            found
        })
    };
    for i in 1..30u64 {
        l.write(&v(), Generation::Live, &[doc(&format!("d{i}"), None, "word")]).unwrap();
        l.commit(&v(), Generation::Live, Some(i)).unwrap();
    }
    assert!(reader.join().unwrap() >= 500);
    fs::remove_dir_all(dir).unwrap();
}
