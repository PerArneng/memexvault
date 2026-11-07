//! Recordings over HTTP, in memory: real routing and JSON, with fake edges and a word table made by hand. The real transcriber is in `memexvault/tests/meeting_eval.rs`.
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use memex_core::testing::{ManualClock, MemBlobStore, MemRegistry, MemStore, SeededRandom};
use memex_core::{NewToken, RenditionInput, Server, ServerEdges, TRANSCRIBE_JOB};
use memex_http::router;
use memex_types::{Epistemic, EvidenceInput, Grant, MediaFacts, ObjectId, Op, PrincipalKind, Scope, Stance, TimedDocument, TimedWord, TranscribeUsage, VaultId};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

struct Fakes;

impl ServerEdges for Fakes {
    type Registry = MemRegistry;
    type Store = MemStore;
    type Blobs = MemBlobStore<ManualClock>;
    type Clock = ManualClock;
    type Seed = SeededRandom;
    type Rasterizer = memex_core::testing::FakeRasterizer;
    type Lexical = memex_core::testing::MemLexical;
    type Vector = memex_core::testing::MemVector;
    type Embedder = memex_core::testing::FakeEmbedder;
    type Reranker = memex_core::testing::FakeReranker;
    type Caches = memex_core::NoCaches;
}

struct World {
    s: Arc<Server<Fakes>>,
    admin: String,
    v: VaultId,
}

fn world() -> World {
    let s = Arc::new(Server::new(
        MemRegistry::new(),
        MemStore::new(),
        MemBlobStore::new(ManualClock::starting_at(0)),
        ManualClock::starting_at(1_700_000_000_000),
        SeededRandom::new(7),
        memex_core::testing::mem_search(),
    ));
    let admin = s.bootstrap("root").unwrap().token;
    let c = s.authenticate(&admin).unwrap();
    let v = s.create_vault(&c, "v").unwrap().id;
    World { s, admin, v }
}

async fn raw(w: &World, method: &str, path: &str, token: &str, headers: &[(&str, &str)], body: Body) -> (StatusCode, Vec<u8>, axum::http::HeaderMap) {
    let mut req = Request::builder().method(method).uri(path).header("authorization", format!("Bearer {token}"));
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let res = router(w.s.clone()).oneshot(req.body(body).unwrap()).await.unwrap();
    let (status, h) = (res.status(), res.headers().clone());
    (status, to_bytes(res.into_body(), 64 << 20).await.unwrap().to_vec(), h)
}

async fn call(w: &World, method: &str, path: &str, token: &str, body: Option<Value>) -> (StatusCode, Value) {
    let (b, headers): (Body, &[(&str, &str)]) = match body {
        Some(v) => (Body::from(v.to_string()), &[("content-type", "application/json")]),
        None => (Body::empty(), &[]),
    };
    let (status, bytes, _) = raw(w, method, path, token, headers, b).await;
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// A real WAV file's bytes: 16 kHz mono, where the sample at index n holds n, so any byte range shows where it came from.
fn wav(seconds: u32) -> Vec<u8> {
    let samples = seconds * 16_000;
    let bytes = samples * 2;
    let mut o = b"RIFF".to_vec();
    o.extend_from_slice(&(36 + bytes).to_le_bytes());
    o.extend_from_slice(b"WAVEfmt ");
    o.extend_from_slice(&16u32.to_le_bytes());
    o.extend_from_slice(&1u16.to_le_bytes());
    o.extend_from_slice(&1u16.to_le_bytes());
    o.extend_from_slice(&16_000u32.to_le_bytes());
    o.extend_from_slice(&32_000u32.to_le_bytes());
    o.extend_from_slice(&2u16.to_le_bytes());
    o.extend_from_slice(&16u16.to_le_bytes());
    o.extend_from_slice(b"data");
    o.extend_from_slice(&bytes.to_le_bytes());
    for n in 0..samples {
        o.extend_from_slice(&((n % 32_000) as i16).to_le_bytes());
    }
    o
}

const WAV_HEADER: u64 = 44;

fn word(text: &str, start_ms: i64, end_ms: i64, speaker: Option<&str>) -> TimedWord {
    TimedWord {
        text: text.into(),
        start_ms,
        end_ms,
        speaker: speaker.map(str::to_string),
    }
}

/// Two speakers with a long pause, and enough words for a segment of its own after it.
fn the_words() -> Vec<TimedWord> {
    let mut words = vec![
        word("Pumpen", 2000, 2400, Some("A")),
        word("flyttades", 2400, 3000, Some("A")),
        word("till", 3000, 3200, Some("A")),
        word("Malmö.", 3200, 3900, Some("A")),
        word("Jaså,", 4000, 4400, Some("B")),
        word("när", 4400, 4600, Some("B")),
        word("då?", 4600, 5000, Some("B")),
    ];
    for i in 0..120 {
        let at = 20_000 + i * 250;
        words.push(word(&format!("ord{i}."), at, at + 250, Some("A")));
    }
    words
}

fn doc(words: Vec<TimedWord>) -> TimedDocument {
    TimedDocument {
        parser: "fake-whisper".into(),
        media: MediaFacts {
            codec: "pcm".into(),
            sample_rate: 16_000,
            channels: 1,
            duration_ms: 60_000,
            ..Default::default()
        },
        words,
        usage: TranscribeUsage::default(),
    }
}

async fn upload(w: &World, query: &str, bytes: Vec<u8>) -> Value {
    let (status, body, _) = raw(w, "POST", &format!("/api/v1/vaults/{}/sources?{query}", w.v.0), &w.admin, &[], Body::from(bytes)).await;
    assert!(status.is_success(), "{status}: {}", String::from_utf8_lossy(&body));
    serde_json::from_slice(&body).unwrap()
}

/// A recording uploaded and transcribed. The job's worker is left out, as these tests are about what comes after.
async fn ingest(w: &World) -> String {
    let up = upload(w, "name=a.wav", wav(60)).await;
    assert_eq!(up["source"]["media_type"], "audio/wav");
    let id = ObjectId(up["source"]["id"].as_str().unwrap().into());
    let job = w.s.claim_job(&w.v, TRANSCRIBE_JOB, "w").unwrap().expect("a transcription job");
    let rows = w.s.build_rendition(&w.v, &id, RenditionInput::Timed(doc(the_words()))).unwrap();
    assert!(w.s.complete_job(&w.v, &job, rows).unwrap());
    id.0
}

async fn segments_of(w: &World, source: &str) -> Vec<Value> {
    let (_, l) = call(w, "GET", &format!("/api/v1/vaults/{}/sources/{source}/renditions", w.v.0), &w.admin, None).await;
    let rendition = l["items"][0]["id"].as_str().unwrap().to_string();
    let (_, s) = call(w, "GET", &format!("/api/v1/vaults/{}/renditions/{rendition}/segments", w.v.0), &w.admin, None).await;
    s["items"].as_array().unwrap().clone()
}

/// A person's claim citing a quote in a segment, committed at once with its anchor.
async fn claim(w: &World, segment: &str, quote: &str) -> (String, String) {
    let id = w.s.new_request_id();
    let evidence = format!("e-{id}");
    let ops = vec![Op::AddAssertion {
        id: ObjectId(format!("a-{id}")),
        subject: "pumpen".into(),
        predicate: "says".into(),
        value: json!(quote),
        epistemic: Epistemic::Asserted,
        machine: false,
        confidence_permille: 900,
        valid_from_ms: None,
        valid_to_ms: None,
        evidence: vec![(
            ObjectId(evidence.clone()),
            EvidenceInput {
                segment_id: ObjectId(segment.into()),
                quote: quote.into(),
                stance: Stance::Supports,
            },
        )],
    }];
    let body = json!({"ops": ops, "rationale": "test", "idempotency_key": id});
    let (status, out) = call(w, "POST", &format!("/api/v1/vaults/{}/proposals", w.v.0), &w.admin, Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["changeset"]["state"], "committed", "{out}");
    (format!("a-{id}"), evidence)
}

#[tokio::test]
async fn the_original_stays_unchanged_and_each_recording_type_is_accepted() {
    let w = world();
    let bytes = wav(1);
    let up = upload(&w, "name=a.wav", bytes.clone()).await;
    let id = up["source"]["id"].as_str().unwrap();
    assert_eq!(up["source"]["media_type"], "audio/wav");
    let (status, got, _) = raw(&w, "GET", &format!("/api/v1/vaults/{}/sources/{id}/stream", w.v.0), &w.admin, &[], Body::empty()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got, bytes, "byte for byte");
    let (status, limits) = call(&w, "GET", &format!("/api/v1/vaults/{}/upload-limits", w.v.0), &w.admin, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(limits["allowed_media_types"].as_array().unwrap().iter().any(|t| t == "video/webm"));
    // A bad language hint isn't passed on to anything.
    let (status, _, _) = raw(&w, "POST", &format!("/api/v1/vaults/{}/sources?name=b.wav&language=../x", w.v.0), &w.admin, &[], Body::from(wav(1))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_transcript_reads_around_a_time_with_speakers() {
    let w = world();
    let id = ingest(&w).await;
    let base = format!("/api/v1/vaults/{}/sources/{id}/transcript", w.v.0);
    let (status, t) = call(&w, "GET", &format!("{base}?around_ms=4500&window_ms=300"), &w.admin, None).await;
    assert_eq!(status, StatusCode::OK, "{t}");
    assert_eq!(t["duration_ms"], 60_000);
    let parts = t["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0]["text"], "Jaså, när då?");
    assert_eq!(parts[0]["speaker"], "B");
    assert!(parts[0]["words"].is_null());
    // Asked with its words, and either side of the turn.
    let (_, t) = call(&w, "GET", &format!("{base}?around_ms=3500&window_ms=1000&words=true"), &w.admin, None).await;
    let parts = t["parts"].as_array().unwrap();
    assert_eq!(parts.iter().map(|p| p["speaker"].as_str().unwrap()).collect::<Vec<_>>(), vec!["A", "B"]);
    assert_eq!(parts[1]["words"][0]["start_ms"], 4000);
    // Nothing was said then.
    let (_, t) = call(&w, "GET", &format!("{base}?around_ms=10000&window_ms=1000"), &w.admin, None).await;
    assert!(t["parts"].as_array().unwrap().is_empty());
    let (status, _) = call(&w, "GET", &format!("{base}?window_ms=5"), &w.admin, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn the_full_transcript_pages_through_everything_once() {
    let w = world();
    let id = ingest(&w).await;
    let base = format!("/api/v1/vaults/{}/sources/{id}/transcript?limit=1", w.v.0);
    let (mut all, mut cursor) = (vec![], None::<String>);
    loop {
        let path = cursor.as_ref().map_or(base.clone(), |c| format!("{base}&cursor={c}"));
        let (status, t) = call(&w, "GET", &path, &w.admin, None).await;
        assert_eq!(status, StatusCode::OK);
        all.extend(t["parts"].as_array().unwrap().iter().map(|p| p["text"].as_str().unwrap().to_string()));
        match t["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_string()),
            None => break,
        }
    }
    assert_eq!(all.len(), 3);
    let words: Vec<String> = all.iter().flat_map(|p| p.split_whitespace()).map(str::to_string).collect();
    assert_eq!(words, the_words().into_iter().map(|w| w.text).collect::<Vec<_>>());
}

#[tokio::test]
async fn a_claim_cites_a_time_range_and_the_transcript_and_provenance_agree() {
    let w = world();
    let id = ingest(&w).await;
    let segments = segments_of(&w, &id).await;
    assert!(segments[0]["t_start_ms"].as_i64().is_some(), "a recording's segment says where it plays");

    let quotes = ["flyttades till", "Jaså, när då?", "Pumpen flyttades till Malmö. Jaså,"];
    let mut checked = 0;
    for quote in quotes {
        let segment = segments.iter().find(|s| s["text"].as_str().unwrap().split_whitespace().collect::<Vec<_>>().join(" ").contains(quote)).unwrap();
        let (assertion, evidence) = claim(&w, segment["id"].as_str().unwrap(), quote).await;
        let (status, l) = call(&w, "GET", &format!("/api/v1/vaults/{}/assertions/{assertion}/lineage", w.v.0), &w.admin, None).await;
        assert_eq!(status, StatusCode::OK, "{l}");
        let e = &l["evidence"][0];
        assert_eq!(e["broken"], false);
        assert_eq!(e["media_type"], "audio/wav");
        let (t0, t1) = (e["t_start_ms"].as_i64().unwrap(), e["t_end_ms"].as_i64().unwrap());
        let (_, h) = call(&w, "GET", &format!("/api/v1/vaults/{}/assertions/{assertion}/evidence/{evidence}/highlight", w.v.0), &w.admin, None).await;
        assert_eq!((h["t_start_ms"].as_i64(), h["t_end_ms"].as_i64()), (Some(t0), Some(t1)));
        assert!(h["rects"].as_array().unwrap().is_empty());

        // The word table's own words, with the padding every cited range has.
        let first = quote.split_whitespace().next().unwrap();
        let last = quote.split_whitespace().last().unwrap();
        let words = the_words();
        let a = words.iter().find(|x| x.text == first).unwrap();
        let b = words.iter().find(|x| x.text == last).unwrap();
        assert_eq!(t0, a.start_ms - memex_core::meeting::START_PAD_MS);
        assert_eq!(t1, b.end_ms + memex_core::meeting::END_PAD_MS);

        // The same words asked for by time, from the transcript, give the same range.
        let (_, t) = call(
            &w,
            "GET",
            &format!("/api/v1/vaults/{}/sources/{id}/transcript?from_ms={}&to_ms={}", w.v.0, a.end_ms, b.start_ms),
            &w.admin,
            None,
        )
        .await;
        let parts = t["parts"].as_array().unwrap();
        let (from, to) = (parts[0]["t_start_ms"].as_i64().unwrap(), parts.last().unwrap()["t_end_ms"].as_i64().unwrap());
        assert_eq!((from, to), (t0, t1), "{quote}");
        checked += 1;
    }
    assert_eq!(checked, 3);
}

#[tokio::test]
async fn a_range_request_plays_from_the_cited_time() {
    let w = world();
    let id = ingest(&w).await;
    let segments = segments_of(&w, &id).await;
    let (assertion, _) = claim(&w, segments[0]["id"].as_str().unwrap(), "flyttades till").await;
    let (_, l) = call(&w, "GET", &format!("/api/v1/vaults/{}/assertions/{assertion}/lineage", w.v.0), &w.admin, None).await;
    let e = &l["evidence"][0];
    let (t0, t1) = (e["t_start_ms"].as_u64().unwrap(), e["t_end_ms"].as_u64().unwrap());
    // As a player does with a WAV: the file's rate is 32,000 bytes a second after its header.
    let (a, b) = (WAV_HEADER + t0 * 32, WAV_HEADER + t1 * 32 - 1);
    let path = format!("/api/v1/vaults/{}/sources/{id}/stream", w.v.0);
    let range = format!("bytes={a}-{b}");
    let (status, bytes, h) = raw(&w, "GET", &path, &w.admin, &[("range", &range)], Body::empty()).await;
    assert_eq!(status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(h["content-range"], format!("bytes {a}-{b}/{}", wav(60).len()));
    assert_eq!(h["content-type"], "audio/wav");
    assert_eq!(h["accept-ranges"], "bytes");
    assert_eq!(bytes.len() as u64, b - a + 1);
    // Each sample holds its own index, so the first sample says exactly where in the recording this starts.
    let first = i16::from_le_bytes([bytes[0], bytes[1]]) as u64;
    assert_eq!(first, t0 * 16 % 32_000, "{t0} ms into the recording");
    assert_eq!(&bytes[..], &wav(60)[a as usize..=b as usize]);
    // The other forms a player uses.
    let (status, bytes, h) = raw(&w, "GET", &path, &w.admin, &[("range", "bytes=-10")], Body::empty()).await;
    assert_eq!((status, bytes.len()), (StatusCode::PARTIAL_CONTENT, 10));
    assert_eq!(h["content-length"], "10");
    let (status, _, h) = raw(&w, "GET", &path, &w.admin, &[("range", "bytes=99999999-")], Body::empty()).await;
    assert_eq!(status, StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(h["content-range"], format!("bytes */{}", wav(60).len()));
}

#[tokio::test]
async fn the_stream_is_for_recordings_and_follows_scope() {
    let w = world();
    let up = upload(&w, "name=notes.txt", b"just some text".to_vec()).await;
    let text = up["source"]["id"].as_str().unwrap();
    let vault = &w.v.0;
    for route in ["stream", "meeting", "media", "transcript"] {
        let (status, _) = call(&w, "GET", &format!("/api/v1/vaults/{vault}/sources/{text}/{route}"), &w.admin, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{route} on a text file");
    }
    let id = ingest(&w).await;
    // No token sees nothing, and neither does a token with no grant on this vault.
    let c = w.s.authenticate(&w.admin).unwrap();
    let p = w.s.create_principal(&c, "nobody", PrincipalKind::Agent, false).unwrap();
    let nobody =
        w.s.create_token(
            &c,
            NewToken {
                principal_id: Some(p.id),
                name: "t".into(),
                grants: vec![],
                expires_at_ms: None,
            },
        )
        .unwrap()
        .token;
    for route in ["stream", "meeting", "media", "transcript"] {
        let (status, _) = call(&w, "GET", &format!("/api/v1/vaults/{vault}/sources/{id}/{route}"), &nobody, None).await;
        assert!(matches!(status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND), "{route}: {status}");
    }
    let res = router(w.s.clone())
        .oneshot(Request::builder().uri(format!("/api/v1/vaults/{vault}/sources/{id}/stream")).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn media_facts_appear_once_transcribed() {
    let w = world();
    let up = upload(&w, "name=a.wav", wav(2)).await;
    let id = up["source"]["id"].as_str().unwrap().to_string();
    let path = format!("/api/v1/vaults/{}/sources/{id}/media", w.v.0);
    let (status, m) = call(&w, "GET", &path, &w.admin, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(m["media_type"], "audio/wav");
    assert!(m["duration_ms"].is_null());
    let (status, _) = call(&w, "GET", &format!("/api/v1/vaults/{}/sources/{id}/transcript", w.v.0), &w.admin, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "not transcribed yet");
    let job = w.s.claim_job(&w.v, TRANSCRIBE_JOB, "w").unwrap().unwrap();
    let rows = w.s.build_rendition(&w.v, &ObjectId(id.clone()), RenditionInput::Timed(doc(the_words()))).unwrap();
    assert!(w.s.complete_job(&w.v, &job, rows).unwrap());
    let (_, m) = call(&w, "GET", &path, &w.admin, None).await;
    assert_eq!(m["duration_ms"], 60_000);
    assert_eq!(m["codec"], "pcm");
    assert_eq!(m["skipped_spans"], 0);
}

#[tokio::test]
async fn silence_makes_a_rendition_with_nothing_to_extract() {
    let w = world();
    let up = upload(&w, "name=a.wav", wav(2)).await;
    let id = up["source"]["id"].as_str().unwrap().to_string();
    let job = w.s.claim_job(&w.v, TRANSCRIBE_JOB, "w").unwrap().unwrap();
    let rows = w.s.build_rendition(&w.v, &ObjectId(id.clone()), RenditionInput::Timed(doc(vec![]))).unwrap();
    assert!(w.s.complete_job(&w.v, &job, rows).unwrap());
    let (status, t) = call(&w, "GET", &format!("/api/v1/vaults/{}/sources/{id}/transcript", w.v.0), &w.admin, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(t["parts"].as_array().unwrap().is_empty());
    assert!(segments_of(&w, &id).await.is_empty());
}

#[tokio::test]
async fn meeting_metadata_reads_writes_and_links_through_proposals() {
    let w = world();
    let id = ingest(&w).await;
    let path = format!("/api/v1/vaults/{}/sources/{id}/meeting", w.v.0);
    let (status, m) = call(&w, "GET", &path, &w.admin, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(m["title"], "a.wav", "the file's name until someone sets one");
    assert!(m["participants"].as_array().unwrap().is_empty());
    let version = m["version"].as_u64().unwrap();

    let body = json!({"title": "Veckomöte", "date_ms": 1_700_000_000_000i64, "participants": [{"name": "Åsa", "speaker": "A"}, {"name": "Bo"}], "base_version": version, "idempotency_key": "m1"});
    let (status, out) = call(&w, "PUT", &path, &w.admin, Some(body.clone())).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["result"]["changeset"]["state"], "committed", "a person's edit");
    let (_, m) = call(&w, "GET", &path, &w.admin, None).await;
    assert_eq!(m["title"], "Veckomöte");
    assert_eq!(m["date_ms"], 1_700_000_000_000i64);
    assert_eq!(m["participants"][0]["name"], "Åsa");
    assert_eq!(m["participants"][0]["speaker"], "A");
    assert!(m["participants"][0]["entity_id"].is_null());
    let pid = m["participants"][0]["id"].as_str().unwrap().to_string();
    assert!(!pid.is_empty());

    // An old version is refused, so someone's edit can't be lost.
    let stale = json!({"title": "x", "participants": [], "base_version": version, "idempotency_key": "m2"});
    let (status, _) = call(&w, "PUT", &path, &w.admin, Some(stale)).await;
    assert_eq!(status, StatusCode::CONFLICT);

    // A Person can be linked, and something else can't.
    let c = w.s.authenticate(&w.admin).unwrap();
    let new_entity = |type_name: &str, name: &str, key: &str| memex_core::NewEntity {
        type_name: type_name.into(),
        aliases: vec![name.into()],
        fields: vec![memex_core::FieldInput {
            field: "name".into(),
            value: json!(name),
            evidence: vec![],
            replaces: None,
        }],
        rationale: "test".into(),
        idempotency_key: key.into(),
    };
    let person = w.s.propose_new_entity(&c, &w.v, new_entity("Person", "Åsa Berg", "p1"), false).unwrap().0;
    let project = w.s.propose_new_entity(&c, &w.v, new_entity("Project", "Pumpen", "p2"), false).unwrap().0;
    let link = format!("{path}/participants/{pid}/link");
    let wrong = json!({"entity_id": project.0, "rationale": "r", "idempotency_key": "l0"});
    let (status, _) = call(&w, "POST", &link, &w.admin, Some(wrong)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let nobody = json!({"entity_id": person.0, "rationale": "r", "idempotency_key": "l0"});
    let (status, _) = call(&w, "POST", &format!("{path}/participants/none/link"), &w.admin, Some(nobody)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // An agent's link is a proposal that waits, and shows only once a person approves it.
    let p = w.s.create_principal(&c, "agent", PrincipalKind::Agent, false).unwrap();
    let agent =
        w.s.create_token(
            &c,
            NewToken {
                principal_id: Some(p.id),
                name: "t".into(),
                grants: vec![Grant {
                    vault: w.v.clone(),
                    scope: Scope::Propose,
                }],
                expires_at_ms: None,
            },
        )
        .unwrap()
        .token;
    let ask = json!({"entity_id": person.0, "rationale": "they said their name", "idempotency_key": "l1"});
    let (status, out) = call(&w, "POST", &link, &agent, Some(ask)).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["result"]["changeset"]["state"], "pending");
    let (_, m) = call(&w, "GET", &path, &w.admin, None).await;
    assert!(m["participants"][0]["entity_id"].is_null(), "not until it's approved");
    let cs = ObjectId(out["result"]["changeset"]["id"].as_str().unwrap().into());
    w.s.approve(&c, &w.v, &cs, None).unwrap();
    let (_, m) = call(&w, "GET", &path, &w.admin, None).await;
    assert_eq!(m["participants"][0]["entity_id"], person.0);
    assert!(m["participants"][1]["entity_id"].is_null());

    // The recording is on the Person's graph, and a revert takes it off.
    let (status, n) = call(&w, "GET", &format!("/api/v1/vaults/{}/objects/{}/neighborhood?kinds=meeting_participant", w.v.0, person.0), &w.admin, None).await;
    assert_eq!(status, StatusCode::OK, "{n}");
    assert!(n.to_string().contains(&id), "{n}");
    w.s.revert(&c, &w.v, &cs, "wrong person".into()).unwrap();
    let (_, m) = call(&w, "GET", &path, &w.admin, None).await;
    assert!(m["participants"][0]["entity_id"].is_null());

    // A new version of the recording is the same meeting.
    let again = upload(&w, &format!("name=b.wav&replaces={id}"), wav(3)).await;
    let (_, m2) = call(&w, "GET", &format!("/api/v1/vaults/{}/sources/{}/meeting", w.v.0, again["source"]["id"].as_str().unwrap()), &w.admin, None).await;
    assert_eq!(m2["title"], "Veckomöte");
    assert_eq!(m2["participants"][0]["id"], pid.as_str());
}

#[tokio::test]
async fn a_read_token_cant_write_a_meeting() {
    let w = world();
    let id = ingest(&w).await;
    let c = w.s.authenticate(&w.admin).unwrap();
    let p = w.s.create_principal(&c, "reader", PrincipalKind::Agent, false).unwrap();
    let reader =
        w.s.create_token(
            &c,
            NewToken {
                principal_id: Some(p.id),
                name: "t".into(),
                grants: vec![Grant {
                    vault: w.v.clone(),
                    scope: Scope::Read,
                }],
                expires_at_ms: None,
            },
        )
        .unwrap()
        .token;
    let path = format!("/api/v1/vaults/{}/sources/{id}/meeting", w.v.0);
    let (status, _) = call(&w, "GET", &path, &reader, None).await;
    assert_eq!(status, StatusCode::OK);
    let body = json!({"title": "x", "participants": [], "base_version": 1, "idempotency_key": "k"});
    let (status, _) = call(&w, "PUT", &path, &reader, Some(body)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (_, m) = call(&w, "GET", &path, &w.admin, None).await;
    assert_eq!(m["title"], "a.wav");
}

#[tokio::test]
async fn a_new_transcript_moves_a_claim_to_its_new_times() {
    let w = world();
    let id = ingest(&w).await;
    let segments = segments_of(&w, &id).await;
    let (assertion, _) = claim(&w, segments[0]["id"].as_str().unwrap(), "flyttades till").await;
    let c = w.s.authenticate(&w.admin).unwrap();
    let source = ObjectId(id.clone());
    let job = w.s.reprocess_source(&c, &w.v, &source).unwrap();
    assert_eq!(job.kind, TRANSCRIBE_JOB, "reprocessing a recording transcribes it");
    let held = w.s.claim_job(&w.v, TRANSCRIBE_JOB, "w").unwrap().unwrap();
    let moved: Vec<TimedWord> = the_words()
        .into_iter()
        .map(|mut x| {
            x.start_ms += 500;
            x.end_ms += 500;
            x
        })
        .collect();
    let rows = w.s.build_rendition(&w.v, &source, RenditionInput::Timed(doc(moved))).unwrap();
    assert!(w.s.complete_job(&w.v, &held, rows).unwrap());
    let (_, l) = call(&w, "GET", &format!("/api/v1/vaults/{}/assertions/{assertion}/lineage", w.v.0), &w.admin, None).await;
    let e = &l["evidence"][0];
    assert_eq!(e["broken"], false);
    assert_eq!(e["t_start_ms"], 2400 + 500 - memex_core::meeting::START_PAD_MS);
    // A transcript that no longer says it flags the claim, and doesn't guess at a new time.
    let job = w.s.reprocess_source(&c, &w.v, &source).unwrap();
    assert_eq!(job.kind, TRANSCRIBE_JOB);
    let held = w.s.claim_job(&w.v, TRANSCRIBE_JOB, "w").unwrap().unwrap();
    let rows = w.s.build_rendition(&w.v, &source, RenditionInput::Timed(doc(vec![word("annat", 0, 500, None)]))).unwrap();
    assert!(w.s.complete_job(&w.v, &held, rows).unwrap());
    let (_, l) = call(&w, "GET", &format!("/api/v1/vaults/{}/assertions/{assertion}/lineage", w.v.0), &w.admin, None).await;
    assert_eq!(l["evidence"][0]["broken"], true);
    assert_eq!(l["evidence"][0]["superseded"], true, "what it cites is the old transcript, which still resolves");
}
