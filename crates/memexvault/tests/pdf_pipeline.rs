//! The rendition pipeline on real PDFs, with the real parser process and in-memory storage.
// These tests read fixtures, and the labelled-language test prints what it measured.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]
use memex_core::ocr::OcrPage;
use memex_core::rendition::detect_language;
use memex_core::testing::{ManualClock, MemBlobStore, MemRegistry, MemStore, SeededRandom};
use memex_core::{Caller, Proposal, RenditionInput, Server, ServerEdges, Snapshot, Store, UploadLimits, UploadRequest};
use memex_types::{Epistemic, EvidenceInput, JobState, ObjectId, Op, PageFlag, PageRoute, Row, RowKey, Stance, Table, VaultId};
use memexvault::rendition_job::{RenditionHandler, WorkerCommand};
use memexvault::workers::run_one;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

struct Fakes;

impl ServerEdges for Fakes {
    type Registry = MemRegistry;
    type Store = MemStore;
    type Blobs = MemBlobStore<ManualClock>;
    type Clock = ManualClock;
    type Seed = SeededRandom;
    type Rasterizer = memex_core::testing::FakeRasterizer;
    type Lexical = memex_core::testing::MemLexical;
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/../memex-pdf/tests/fixtures/{name}.pdf", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

struct World {
    s: Arc<Server<Fakes>>,
    c: Caller,
    v: VaultId,
    handler: RenditionHandler<Fakes>,
}

fn world() -> World {
    let s = Arc::new(Server::<Fakes>::new(
        MemRegistry::new(),
        MemStore::new(),
        MemBlobStore::new(ManualClock::starting_at(0)),
        ManualClock::starting_at(1_000_000),
        SeededRandom::new(15),
        memex_core::testing::MemLexical::new(),
    ));
    let c = s.authenticate(&s.bootstrap("root").unwrap().token).unwrap();
    let v = s.create_vault(&c, "v").unwrap().id;
    let worker = WorkerCommand {
        program: env!("CARGO_BIN_EXE_memexvault").into(),
        args: vec!["pdf-worker".into()],
        timeout: Duration::from_secs(60),
    };
    let handler = RenditionHandler {
        server: s.clone(),
        worker,
        ocr: None,
    };
    World { s, c, v, handler }
}

fn upload(w: &World, name: &str, bytes: &[u8]) -> ObjectId {
    let req = UploadRequest {
        name: name.into(),
        trust_tier: "unverified".into(),
        metadata: json!({}),
        replaces: None,
    };
    w.s.upload(&w.c, &w.v, req, UploadLimits::default(), &mut &bytes[..]).unwrap().source.header.id
}

fn run(w: &World) {
    assert!(run_one(w.s.as_ref(), &w.handler, "rendition", "worker"));
}

#[test]
fn a_pdf_becomes_a_rendition_with_segments_and_identical_bytes_when_parsed_again() {
    let w = world();
    let source = upload(&w, "two_column_en.pdf", &fixture("two_column_en"));
    run(&w);
    let list = w.s.list_renditions(&w.c, &w.v, &source).unwrap();
    assert_eq!(list.len(), 1);
    let first = list[0].clone();
    assert!(first.page_count >= 1 && first.segment_count >= 1);
    assert!(first.parser.starts_with("pdfium"));
    let segs = w.s.list_segments(&w.c, &w.v, &first.header.id, None, 1000).unwrap();
    assert_eq!(segs.len() as u32, first.segment_count);
    assert!(
        segs.iter().all(|s| s.language.as_deref() == Some("en")),
        "{:?}",
        segs.iter().map(|s| s.language.clone()).collect::<Vec<_>>()
    );
    assert!(segs.iter().all(|s| s.page_from.is_some()));
    // Reprocess: a new rendition, the old one marked, and the same bytes.
    w.s.reprocess_source(&w.c, &w.v, &source).unwrap();
    run(&w);
    let list = w.s.list_renditions(&w.c, &w.v, &source).unwrap();
    assert_eq!(list.len(), 2);
    let (old, new) = (&list[0], &list[1]);
    assert_eq!(old.superseded_by.as_ref(), Some(&new.header.id));
    assert_eq!(new.superseded_by, None);
    assert_eq!(
        (old.text_blob.as_str(), old.words_blob.as_str()),
        (new.text_blob.as_str(), new.words_blob.as_str()),
        "same input, same bytes"
    );
    assert_eq!(
        w.s.rendition_blob(&w.c, &w.v, &new.header.id, false).unwrap(),
        w.s.rendition_blob(&w.c, &w.v, &old.header.id, false).unwrap()
    );
}

#[test]
fn bad_pdfs_fail_one_job_each_and_the_next_job_still_runs() {
    let w = world();
    let bad: Vec<(&str, Vec<u8>)> = vec![
        ("encrypted", fixture("password_protected")),
        ("zero pages", fixture("zero_pages")),
        ("malformed", b"%PDF-1.7 this isn't really one".to_vec()),
        ("truncated", fixture("one_column_sv")[..300].to_vec()),
    ];
    for (name, bytes) in &bad {
        upload(&w, &format!("{name}.pdf"), bytes);
    }
    let good = upload(&w, "ok.pdf", &fixture("table_es"));
    for _ in 0..5 {
        run(&w);
    }
    let dead = w.s.list_jobs(&w.c, &w.v, None, Some(JobState::Dead), None, 10).unwrap();
    assert_eq!(dead.len(), 4);
    let errors: Vec<String> = dead.iter().map(|j| j.attempts[0].error.clone().unwrap()).collect();
    assert!(errors[0].contains("encrypted"), "{errors:?}");
    assert!(errors.iter().all(|e| !e.is_empty()));
    assert_eq!(w.s.list_renditions(&w.c, &w.v, &good).unwrap().len(), 1);
    assert_eq!(
        w.s.engine()
            .store()
            .snapshot(&w.v)
            .unwrap()
            .jobs(None, Some(JobState::Done), None, 10)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn a_time_bomb_is_stopped_at_its_limit_and_costs_one_job() {
    let mut w = world();
    w.handler.worker = WorkerCommand {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), "sleep 30".into()],
        timeout: Duration::from_millis(100),
    };
    upload(&w, "bomb.pdf", &fixture("table_es"));
    run(&w);
    let dead = w.s.list_jobs(&w.c, &w.v, None, Some(JobState::Dead), None, 10).unwrap();
    assert_eq!(dead.len(), 1);
    assert!(dead[0].attempts[0].error.as_ref().unwrap().contains("past its limit"));
}

fn claim(aid: &str, seg: &ObjectId, quote: &str) -> Op {
    Op::AddAssertion {
        id: ObjectId(aid.into()),
        subject: format!("s{aid}"),
        predicate: "says".into(),
        value: json!(quote),
        epistemic: Epistemic::Asserted,
        machine: false,
        confidence_permille: 900,
        valid_from_ms: None,
        valid_to_ms: None,
        evidence: vec![(
            ObjectId(format!("{aid}-e")),
            EvidenceInput {
                segment_id: seg.clone(),
                quote: quote.into(),
                stance: Stance::Supports,
            },
        )],
    }
}

fn evidence(w: &World, aid: &str) -> memex_types::EvidenceRow {
    let snap = w.s.engine().store().snapshot(&w.v).unwrap();
    let Some(Row::Evidence(e)) = snap
        .get(&RowKey {
            table: Table::Evidence,
            id: ObjectId(format!("{aid}-e")),
        })
        .unwrap()
    else {
        panic!("{aid}")
    };
    e
}

#[test]
fn a_changed_parser_makes_a_new_rendition_and_old_anchors_move_or_get_flagged_with_none_lost() {
    let w = world();
    let source = upload(
        &w,
        "a.md",
        b"First part keeps this sentence exactly.\n\nSecond part says something that will vanish.",
    );
    run(&w);
    let first = w.s.list_renditions(&w.c, &w.v, &source).unwrap().remove(0);
    let seg = w.s.list_segments(&w.c, &w.v, &first.header.id, None, 10).unwrap().remove(0);
    let ops = vec![
        claim("kept", &seg.header.id, "keeps this sentence"),
        claim("lost", &seg.header.id, "something that will vanish"),
    ];
    w.s.propose(
        &w.c,
        &w.v,
        Proposal {
            ops,
            rationale: "r".into(),
            idempotency_key: "k".into(),
        },
        false,
    )
    .unwrap();
    // A new parser version reads the document differently: one quote survives and one doesn't.
    let rows =
        w.s.build_rendition(
            &w.v,
            &source,
            RenditionInput::Plain {
                text: "Heading line.\n\nFirst part keeps this sentence exactly.\n\nThe second part was changed.".into(),
                parser: "plain-2".into(),
            },
        )
        .unwrap();
    let job = {
        w.s.reprocess_source(&w.c, &w.v, &source).unwrap();
        w.s.claim_job(&w.v, "rendition", "w").unwrap().unwrap()
    };
    assert!(w.s.complete_job(&w.v, &job, rows).unwrap());
    let list = w.s.list_renditions(&w.c, &w.v, &source).unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[1].parser, "plain-2");
    let kept = evidence(&w, "kept");
    assert!(!kept.anchor_broken);
    let anchor = kept.anchor.expect("moved");
    assert_eq!(anchor.rendition_id, list[1].header.id);
    assert_eq!(anchor.quote, "keeps this sentence");
    assert_ne!(kept.segment_id, seg.header.id);
    let lost = evidence(&w, "lost");
    assert!(lost.anchor_broken, "flagged, not silently dropped");
    assert_eq!(lost.quote, "something that will vanish");
    let failures = w.s.list_broken_anchors(&w.c, &w.v, None, 10).unwrap();
    assert_eq!(failures.iter().map(|e| e.header.id.0.as_str()).collect::<Vec<_>>(), vec!["lost-e"]);
}

#[test]
fn a_mixed_language_document_gets_each_segment_s_language() {
    let en = "The factory was moved to the coast after the fire, and the board decided that production would continue there for many years to come. ".repeat(8);
    let sv =
        "Fabriken flyttades till kusten efter branden, och styrelsen beslutade att tillverkningen skulle fortsätta där under många år framöver. ".repeat(8);
    let de = "Das Werk zog nach dem Brand an die Küste um, und der Vorstand beschloss, dass die Fertigung dort noch viele Jahre weitergehen sollte. ".repeat(8);
    let es = "La fábrica se trasladó a la costa después del incendio, y el consejo decidió que la producción continuaría allí durante muchos años. ".repeat(8);
    let w = world();
    let source = upload(&w, "mixed.txt", format!("{en}\n\n{sv}\n\n{de}\n\n{es}").as_bytes());
    run(&w);
    let r = w.s.list_renditions(&w.c, &w.v, &source).unwrap().remove(0);
    let langs: Vec<String> =
        w.s.list_segments(&w.c, &w.v, &r.header.id, None, 10)
            .unwrap()
            .into_iter()
            .map(|s| s.language.unwrap_or_default())
            .collect();
    assert_eq!(langs, vec!["en", "sv", "de", "es"]);
}

/// Measures language detection on real text, which isn't committed. Run by hand with the path to spec 005's corpus.
#[test]
#[ignore = "needs MEMEX_WIKI_JSON, the corpus from spec 005"]
fn language_accuracy_on_labelled_wikipedia_paragraphs() {
    let items: Vec<serde_json::Value> = serde_json::from_str(&std::fs::read_to_string(std::env::var("MEMEX_WIKI_JSON").unwrap()).unwrap()).unwrap();
    for (len, name) in [(usize::MAX, "whole leads"), (600, "600 chars"), (200, "200 chars")] {
        let (mut total, mut right, mut unknown) = (0, 0, 0);
        for item in &items {
            for lang in ["en", "sv", "de", "es"] {
                let text: String = item["p"][lang].as_str().unwrap().chars().take(len).collect();
                total += 1;
                match detect_language(&text) {
                    Some(l) if l == lang => right += 1,
                    None => unknown += 1,
                    _ => {}
                }
            }
        }
        println!(
            "language: {name}: {total} labelled, {right} right, {unknown} unknown, {:.1}% right, {:.1}% wrong",
            100.0 * right as f64 / total as f64,
            100.0 * (total - right - unknown) as f64 / total as f64
        );
    }
}

// The tests below read scanned pages with the real OCR models, so they're ignored unless a models folder is named.
fn ocr_world() -> World {
    let mut w = world();
    let dir = std::env::var("MEMEX_TEST_MODELS").expect("not run: MEMEX_TEST_MODELS isn't set");
    w.handler.ocr = Some(WorkerCommand {
        program: env!("CARGO_BIN_EXE_memexvault").into(),
        args: vec!["ocr-worker".into(), dir],
        timeout: Duration::from_secs(120),
    });
    w
}

fn pages_of(w: &World, source: &ObjectId) -> Vec<memex_types::PageOutcome> {
    w.s.list_renditions(&w.c, &w.v, source).unwrap().pop().unwrap().pages
}

fn has_hidden_flag(p: &memex_types::PageOutcome) -> bool {
    p.flags.iter().any(|f| matches!(f, PageFlag::HiddenText { .. } | PageFlag::ZeroSizeText { .. }))
}

#[test]
#[ignore = "needs MEMEX_TEST_MODELS"]
fn a_mixed_pdf_routes_each_page_and_ocr_pages_get_words_with_boxes() {
    let w = ocr_world();
    let source = upload(&w, "mixed.pdf", &fixture("mixed_text_image"));
    run(&w);
    let pages = pages_of(&w, &source);
    let routes: Vec<PageRoute> = pages.iter().map(|p| p.route).collect();
    println!("mixed: {pages:?}");
    assert!(routes.contains(&PageRoute::TextLayer) && routes.contains(&PageRoute::Ocr), "{routes:?}");
    let r = w.s.list_renditions(&w.c, &w.v, &source).unwrap().pop().unwrap();
    let table: memex_types::ParsedDocument = serde_json::from_slice(&w.s.rendition_blob(&w.c, &w.v, &r.header.id, true).unwrap()).unwrap();
    let ocr_page = pages.iter().find(|p| p.route == PageRoute::Ocr).unwrap().index;
    let on_ocr_page: Vec<_> = table.words.iter().filter(|x| x.page == ocr_page).collect();
    assert!(on_ocr_page.len() > 50, "{}", on_ocr_page.len());
    assert!(on_ocr_page.iter().all(|x| x.confidence.is_some() && x.quad.is_some()));
    let facts = &table.pages[ocr_page as usize];
    assert!(
        on_ocr_page
            .iter()
            .all(|x| x.rect.x0 >= 0 && x.rect.y0 >= 0 && x.rect.x1 <= facts.width + 100 && x.rect.y1 <= facts.height + 100)
    );
    // An anchor on an OCR page resolves like any other.
    let quote = format!("{} {}", on_ocr_page[10].text, on_ocr_page[11].text);
    let c = memex_core::rendition::canonical(&table.words);
    let anchor = memex_core::rendition::locate(&c, &table.words, "sha", &r.header.id, &quote, "", "").unwrap();
    assert!(!anchor.rects.is_empty());
    println!("ocr anchor example: {quote:?} {:?}", anchor.rects);
}

#[test]
#[ignore = "needs MEMEX_TEST_MODELS"]
fn hidden_text_fixtures_are_all_flagged_and_appear_in_the_list() {
    let w = ocr_world();
    for name in ["hidden_white", "hidden_white_between", "hidden_zero_size", "hidden_render_mode"] {
        let source = upload(&w, name, &fixture(name));
        run(&w);
        let pages = pages_of(&w, &source);
        println!("{name}: {:?}", pages[0].flags);
        assert!(has_hidden_flag(&pages[0]), "{name}: {pages:?}");
    }
    assert_eq!(w.s.flagged_pages(&w.c, &w.v).unwrap().len(), 4);
}

#[test]
#[ignore = "needs MEMEX_TEST_MODELS"]
fn clean_pages_have_few_false_flags() {
    let w = ocr_world();
    let (mut checked, mut flagged) = (0, 0);
    // Every page of a clean document is checked here, by making each one count as a sampled page.
    for name in ["clean_pages", "two_column_en", "three_column_de", "one_column_sv", "table_es"] {
        let doc_pdf = fixture(name);
        let doc = run_parse(&doc_pdf);
        for facts in &doc.pages {
            let layer: Vec<&memex_types::Word> = doc.words.iter().filter(|x| x.page == facts.index).collect();
            let c = w.handler.ocr.as_ref().unwrap();
            let page: OcrPage = run_ocr(c, &doc_pdf, facts.index, memex_core::ocr::dpi_for(facts.width, facts.height));
            let seen = memex_core::ocr::words_from_ocr(facts.index, &page);
            let out = memex_core::ocr::page_outcome(facts.index, PageRoute::TextLayer, &layer, Some(&seen), None);
            checked += 1;
            if has_hidden_flag(&out) {
                flagged += 1;
                println!("false flag: {name} page {}: {:?}", facts.index, out.flags);
            }
        }
    }
    println!("clean pages: {checked} checked, {flagged} flagged");
    assert!(checked >= 30);
    assert!(flagged * 100 < checked * 2, "{flagged} of {checked}");
}

fn run_parse(pdf: &[u8]) -> memex_types::ParsedDocument {
    let c = WorkerCommand {
        program: env!("CARGO_BIN_EXE_memexvault").into(),
        args: vec!["pdf-worker".into()],
        timeout: Duration::from_secs(60),
    };
    memexvault::rendition_job::run_worker(&c, pdf.to_vec()).unwrap()
}

fn run_ocr(c: &WorkerCommand, pdf: &[u8], page: u32, dpi: u32) -> OcrPage {
    let mut c = c.clone();
    c.args.push(page.to_string());
    c.args.push(dpi.to_string());
    memexvault::rendition_job::run_any_child(&c, pdf.to_vec()).unwrap()
}

#[test]
#[ignore = "needs MEMEX_TEST_MODELS"]
fn an_ocr_failure_on_one_page_keeps_the_rest_and_a_corrupted_model_is_refused() {
    let dir = std::env::var("MEMEX_TEST_MODELS").expect("not run");
    let bin = env!("CARGO_BIN_EXE_memexvault");
    let mut w = world();
    // A wrapper that fails on page 0, and runs the real worker otherwise.
    let script = format!("if [ \"$1\" = 0 ]; then echo 'page 0 is broken' >&2; exit 3; fi; exec '{bin}' ocr-worker '{dir}' \"$1\" \"$2\"");
    w.handler.ocr = Some(WorkerCommand {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), script, "ocr".into()],
        timeout: Duration::from_secs(120),
    });
    let source = upload(&w, "mixed.pdf", &fixture("mixed_text_image"));
    run(&w);
    let pages = pages_of(&w, &source);
    println!("failure case: {pages:?}");
    assert!(pages.iter().any(|p| p.flags.iter().any(|f| matches!(f, PageFlag::OcrFailed { .. }))));
    assert_eq!(
        w.s.engine()
            .store()
            .snapshot(&w.v)
            .unwrap()
            .jobs(None, Some(JobState::Done), None, 10)
            .unwrap()
            .len(),
        1,
        "the job still finishes"
    );
    // A model with the right size and wrong bytes.
    let bad = std::env::temp_dir().join(format!("memex-bad-models-{}", std::process::id()));
    std::fs::create_dir_all(&bad).unwrap();
    for m in memexvault::models::OCR_MODELS {
        let mut bytes = std::fs::read(std::path::Path::new(&dir).join(m.file)).unwrap();
        if m.file == memex_ocr::DETECTOR {
            bytes[1000] ^= 0xff;
        }
        std::fs::write(bad.join(m.file), bytes).unwrap();
    }
    let e = memexvault::models::verify(&bad, &memexvault::models::OCR_MODELS).unwrap_err();
    assert!(e.contains("wrong hash"), "{e}");
    std::fs::remove_dir_all(bad).unwrap();
    assert!(
        memexvault::models::check_present(std::path::Path::new("/no/such/place"), &memexvault::models::OCR_MODELS)
            .unwrap_err()
            .contains("models fetch")
    );
}

#[test]
#[ignore = "needs MEMEX_TEST_MODELS"]
fn a_blank_page_and_a_huge_page_are_handled_with_bounded_memory() {
    let w = ocr_world();
    let source = upload(&w, "blank.pdf", &fixture("blank_and_huge"));
    run(&w);
    let pages = pages_of(&w, &source);
    println!("blank and huge: {pages:?}");
    assert_eq!(pages[0].route, PageRoute::Blank);
    assert_eq!(pages[1].route, PageRoute::Blank);
    // A huge page that does go to OCR renders within the limit. 200 inches square at 300 dpi would be 3.6 GB.
    let dpi = memex_core::ocr::dpi_for(1_440_000, 1_440_000);
    let c = w.handler.ocr.as_ref().unwrap();
    let out = run_ocr(c, &fixture("blank_and_huge"), 1, dpi);
    println!("huge page: rendered at {dpi} dpi, {} regions", out.regions.len());
}
