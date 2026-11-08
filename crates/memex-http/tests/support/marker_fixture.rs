//! Everything on `main` that writes to the log, with a marker wherever text can go. Shared by the activity tests in memory and on Postgres.
#![allow(dead_code)]
use memex_core::testing::ScriptedAgent;
use memex_core::{
    Caller, EXTRACT_JOB, ExtractionLimits, NewEntity, NewPage, Proposal, RENDITION_JOB, REVIEW_JOB, RenditionInput, Server, ServerEdges, Snapshot, Store,
    TRANSCRIBE_JOB, TriggerAudit, UploadLimits, UploadRequest,
};
use memex_types::{
    AgentError, AgentReply, AgentUsage, Epistemic, EvidenceInput, MediaFacts, ObjectId, Op, Stance, TimedDocument, TimedWord, TranscribeUsage, VaultId,
};
use serde_json::{Value, json};
use std::time::Duration;

pub const MARKER: &str = "ZQXMARKERZQX";

pub fn plain(text: &str) -> RenditionInput {
    RenditionInput::Plain {
        text: text.into(),
        parser: "plain-1".into(),
    }
}

pub fn reply(doc: Value) -> Result<AgentReply, AgentError> {
    Ok(AgentReply {
        json: doc.to_string(),
        usage: AgentUsage {
            input_tokens: 10,
            output_tokens: 5,
            thinking_tokens: 0,
        },
        tool_calls: 0,
    })
}

fn word(text: &str, start_ms: i64, end_ms: i64) -> TimedWord {
    TimedWord {
        text: text.into(),
        start_ms,
        end_ms,
        speaker: Some(MARKER.into()),
    }
}

fn wav() -> Vec<u8> {
    let samples: u32 = 16_000;
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
    o.extend(std::iter::repeat_n(0u8, bytes as usize));
    o
}

/// Everything on `main` that writes to the log, with the marker wherever text can go: names, metadata, document text, failure reasons, quotes, verdicts, findings, transcripts and page bodies.
pub fn fill_with_marker<E>(s: &Server<E>, c: &Caller, v: &VaultId, advance: &dyn Fn(Duration))
where
    E: ServerEdges,
{
    s.set_extraction(Some(ExtractionLimits::default()));
    s.set_reviewer(Some(memex_core::ReviewLimits::default()));
    s.set_auditor(Some(memex_core::AuditLimits::default()));
    let sentence = format!("Fartyget {MARKER} byggdes i Stockholm.");
    let text = format!("{sentence} Det är en mening om fartyget {MARKER}.");
    // A document, with the marker in its name, metadata and text, through extraction.
    let req = UploadRequest {
        name: format!("{MARKER}.txt"),
        trust_tier: "trusted".into(),
        metadata: json!({"title": MARKER}),
        replaces: None,
    };
    let source = s.upload(c, v, req, UploadLimits::default(), &mut text.as_bytes()).unwrap().source.header.id;
    let job = s.claim_job(v, RENDITION_JOB, "w").unwrap().unwrap();
    let rows = s.build_rendition(v, &source, plain(&text)).unwrap();
    assert!(s.complete_job(v, &job, rows).unwrap());
    let job = s.claim_job(v, EXTRACT_JOB, "w").unwrap().unwrap();
    let agent = ScriptedAgent::new(vec![reply(json!({
        "claims": [
            {"segment_id": "s1", "quote": sentence, "subject": format!("Fartyget {MARKER}"), "predicate": "byggdes i", "value": format!("Stockholm {MARKER}")},
            {"segment_id": "s1", "quote": format!("not in the text {MARKER}"), "subject": MARKER, "predicate": MARKER, "value": MARKER}
        ],
        "entities": {}
    }))]);
    let rows = s.run_extraction_job(v, &job, &agent).unwrap();
    assert!(s.complete_job(v, &job, rows).unwrap());
    // The reviewer's verdict.
    while let Some(job) = s.claim_job(v, REVIEW_JOB, "w").unwrap() {
        let agent = ScriptedAgent::new(vec![reply(json!({"verdict": "approve", "reason": format!("it says {MARKER}")}))]);
        let rows = s.run_review_job(v, &job, &agent).unwrap();
        assert!(s.complete_job(v, &job, rows).unwrap());
    }
    // Failure reasons of every kind: a retry, a wait with a reason, and a permanent failure, as a parser's stderr might quote a document.
    let req = UploadRequest {
        name: format!("{MARKER}-bad.txt"),
        trust_tier: "unverified".into(),
        metadata: json!({}),
        replaces: None,
    };
    s.upload(c, v, req, UploadLimits::default(), &mut format!("bad {MARKER}").as_bytes()).unwrap();
    let job = s.claim_job(v, RENDITION_JOB, "w").unwrap().unwrap();
    s.fail_job(v, &job, &format!("line 3: {MARKER} is wrong"), true).unwrap();
    advance(Duration::from_secs(600));
    let job = s.claim_job(v, RENDITION_JOB, "w").unwrap().unwrap();
    s.wait_job(v, &job, &format!("away because {MARKER}")).unwrap();
    advance(Duration::from_secs(600));
    let job = s.claim_job(v, RENDITION_JOB, "w").unwrap().unwrap();
    s.fail_job(v, &job, &format!("gave up on {MARKER}"), false).unwrap();
    // A recording: its words, its speakers, its meeting metadata and a participant.
    let req = UploadRequest {
        name: format!("{MARKER}.wav"),
        trust_tier: "trusted".into(),
        metadata: json!({"meeting": {"title": MARKER, "participants": [MARKER]}}),
        replaces: None,
    };
    let recording = s.upload(c, v, req, UploadLimits::default(), &mut wav().as_slice()).unwrap().source;
    let job = s.claim_job(v, TRANSCRIBE_JOB, "w").unwrap().unwrap();
    let doc = TimedDocument {
        parser: "fake-whisper".into(),
        media: MediaFacts {
            codec: "pcm".into(),
            sample_rate: 16_000,
            channels: 1,
            duration_ms: 1_000,
            ..Default::default()
        },
        words: vec![word(MARKER, 0, 400), word("sa", 400, 700)],
        usage: TranscribeUsage::default(),
    };
    let rows = s.build_rendition(v, &recording.header.id, RenditionInput::Timed(doc)).unwrap();
    assert!(s.complete_job(v, &job, rows).unwrap());
    let meta: memex_types::MeetingMeta = serde_json::from_value(json!({"title": MARKER, "participants": [{"name": MARKER}]})).unwrap();
    let base = s.get_source(c, v, &recording.header.id).unwrap().header.version;
    s.propose_meeting(c, v, &recording.header.id, meta, base, format!("meeting-{MARKER}")).unwrap();
    // A claim a person made with evidence, and a page and an entity.
    let segment = {
        let snap = s.engine().store().snapshot(v).unwrap();
        let rendition = snap.renditions_of(&source).unwrap().remove(0);
        snap.segments_of(&rendition.header.id, None, 1).unwrap().remove(0).header.id
    };
    let claim = Proposal {
        ops: vec![Op::AddAssertion {
            id: ObjectId("claimmarker".into()),
            subject: MARKER.into(),
            predicate: "says".into(),
            value: json!(MARKER),
            epistemic: Epistemic::Asserted,
            machine: false,
            confidence_permille: 1000,
            valid_from_ms: None,
            valid_to_ms: None,
            evidence: vec![(
                ObjectId("evidencemarker".into()),
                EvidenceInput {
                    segment_id: segment.clone(),
                    quote: sentence.clone(),
                    stance: Stance::Supports,
                },
            )],
        }],
        rationale: format!("because {MARKER}"),
        idempotency_key: format!("claim-{MARKER}"),
    };
    s.propose(c, v, claim, false).unwrap();
    let (_, page) = s
        .propose_new_page(
            c,
            v,
            NewPage {
                title: format!("Sidan {MARKER}"),
                slug: Some(MARKER.to_ascii_lowercase()),
                body: format!("# {MARKER}\n\nText {MARKER} and a [[{}]] link.", MARKER.to_ascii_lowercase()),
                rationale: format!("because {MARKER}"),
                idempotency_key: format!("page-{MARKER}"),
            },
            false,
        )
        .unwrap();
    assert!(page.changeset.is_some());
    s.propose_entity_type(
        c,
        v,
        "Markertype",
        None,
        json!({"type": "object", "additionalProperties": false, "properties": {"name": {"type": "string", "identity": true, "searchable": true}}}),
        format!("type {MARKER}"),
        format!("type-{MARKER}"),
    )
    .unwrap();
    let (_, entity) = s
        .propose_new_entity(
            c,
            v,
            NewEntity {
                type_name: "Markertype".into(),
                aliases: vec![MARKER.into()],
                fields: vec![],
                rationale: format!("because {MARKER}"),
                idempotency_key: format!("entity-{MARKER}"),
            },
            false,
        )
        .unwrap();
    let entity = entity.changeset.unwrap();
    if entity.state == memex_types::ChangeSetState::Pending {
        s.approve(c, v, &entity.header.id, Some(format!("yes, {MARKER}"))).unwrap();
    }
    // An audit with findings: a deterministic run, and one where the model answers with the marker.
    while s.project_lexical(v).unwrap() == memex_core::StepOutcome::Progress {}
    s.trigger_audit(
        c,
        v,
        TriggerAudit {
            seed: Some(1),
            sample_size: Some(10),
            run_token_budget: Some(1_000_000),
        },
    )
    .unwrap();
    let job = s.claim_job(v, memex_core::AUDIT_JOB, "w").unwrap().unwrap();
    let agent = ScriptedAgent::answering(|_| reply(json!({"verdict": "not_entailed", "reason": format!("not so: {MARKER}"), "claims": [], "entities": {}})));
    let rows = s.run_audit_job(v, &job, Some(&agent)).unwrap();
    assert!(s.complete_job(v, &job, rows).unwrap());
    // The kinds written straight to the log.
    for (kind, version, payload) in [
        (
            "source.recorded",
            3,
            json!({"id": "srcdirect", "logical_id": "srcdirect", "version_no": 1, "name": MARKER, "size": 1, "metadata": {"x": MARKER}, "sha256": "ab", "media_type": "text/plain", "trust_tier": "trusted"}),
        ),
        (
            "segment.recorded",
            1,
            json!({"id": "segdirect", "source_id": "srcdirect", "language": null, "text": MARKER}),
        ),
        ("page.created", 1, json!({"id": "pagedirect", "title": MARKER, "body": MARKER})),
        (
            "page.edited",
            1,
            json!({"id": "pagedirect", "base_version": 1, "body": format!("edit {MARKER}")}),
        ),
        (
            "edge.linked",
            1,
            json!({"id": "edgedirect", "from": "pagedirect", "to": format!("slug:{MARKER}"), "kind": MARKER}),
        ),
        ("edge.unlinked", 1, json!({"id": "edgedirect"})),
        ("page.deleted", 1, json!({"id": "pagedirect"})),
    ] {
        s.apply_as(c, v, kind, version, payload).unwrap_or_else(|e| panic!("{kind}: {e}"));
    }
}
