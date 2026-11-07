//! Domain results to DTOs, for the frontends that answer with them. HTTP and MCP share these, so the two can't drift apart.
use crate::{
    BlockDto, CandidateDto, ClaimParts, ContradictionDto, EntityDetailDto, EntityFieldDto, ErrorDetail, EvidenceLineageDto, FieldEvidenceDto, LineageDto,
    NeighborDto, NeighborPage, PageClaimDto, PageDetailDto, ProposalResult, RectDto, ResolveResult, UploadResult, json_text, page_of,
};
use memex_core::{
    ClaimRefStatus, ClaimReference, EntityClaim, EntityResolution, EntityView, Lineage, NeighborView, PageView, ProposalOutcome, ServerError, UploadOutcome,
};
use memex_types::{Anchor, ObjectId};
use serde_json::Value;

pub fn proposal_result(out: ProposalOutcome) -> ProposalResult {
    ProposalResult {
        changeset: out.changeset.map(Into::into),
        validators: out.validators.iter().filter_map(|v| serde_json::to_value(v).ok()).collect(),
        risk: json_text(&out.risk),
        contradictions: out
            .contradictions
            .into_iter()
            .map(|(a, b)| ContradictionDto { claim: a.0, other: b.0 })
            .collect(),
        replayed: out.replayed,
        dry_run: out.dry_run,
    }
}

pub fn upload_result(out: UploadOutcome) -> UploadResult {
    UploadResult {
        source: out.source.into(),
        job_id: out.job.map(|j| j.header.id.0),
        existing: out.existing,
    }
}

pub fn rects_of(a: Option<&Anchor>) -> Vec<RectDto> {
    a.map_or(vec![], |a| {
        a.rects
            .iter()
            .map(|(page, r)| RectDto {
                page: *page,
                x0: r.x0,
                y0: r.y0,
                x1: r.x1,
                y1: r.y1,
            })
            .collect()
    })
}

pub fn lineage_dto(l: Lineage) -> LineageDto {
    LineageDto {
        changeset_id: l.assertion.changeset_id.0.clone(),
        assertion: serde_json::to_value(&l.assertion).unwrap_or(Value::Null),
        proposer: l.changeset.as_ref().map(|c| c.proposer.clone()),
        proposer_is_machine: l.changeset.as_ref().map(|c| c.proposer_is_machine),
        decided_by: l.changeset.as_ref().and_then(|c| c.decided_by.clone()),
        rationale: l.changeset.as_ref().map(|c| c.rationale.clone()),
        evidence: l
            .evidence
            .into_iter()
            .map(|e| EvidenceLineageDto {
                rects: rects_of(e.anchor.as_ref()),
                t_start_ms: e.anchor.as_ref().and_then(|a| a.time.as_ref()).map(|t| t.t_start_ms),
                t_end_ms: e.anchor.as_ref().and_then(|a| a.time.as_ref()).map(|t| t.t_end_ms),
                speaker: e.anchor.as_ref().and_then(|a| a.time.as_ref()).and_then(|t| t.speaker.clone()),
                media_type: e.source.media_type,
                evidence_id: e.evidence.header.id.0,
                quote: e.evidence.quote,
                stance: json_text(&e.evidence.stance),
                segment_id: e.segment.header.id.0,
                rendition_id: e.rendition.header.id.0,
                parser: e.rendition.parser,
                source_id: e.source.header.id.0,
                source_name: e.source.name,
                source_sha256: e.source.sha256,
                trust_tier: e.source.trust_tier,
                superseded: e.superseded,
                broken: e.broken,
            })
            .collect(),
    }
}

pub fn resolve_result(r: EntityResolution) -> ResolveResult {
    ResolveResult {
        candidates: r
            .candidates
            .into_iter()
            .map(|c| CandidateDto {
                entity: c.entity.into(),
                matched_on: c.matched_on,
                exact: c.exact,
                score: c.score,
            })
            .collect(),
        degraded: r.degraded,
    }
}

pub fn entity_detail(view: EntityView) -> EntityDetailDto {
    let e = view.entity;
    let mut claims = view.claims;
    let mut lookup = |id: &ObjectId| -> Option<ClaimParts> {
        let EntityClaim { assertion, evidence } = claims.remove(id)?;
        Some(ClaimParts {
            epistemic: json_text(&assertion.epistemic),
            machine: assertion.machine,
            creator: assertion.header.creator,
            changeset_id: assertion.changeset_id.0,
            evidence: evidence.into_iter().map(|x| FieldEvidenceDto::new(x.evidence, x.segment, x.source)).collect(),
        })
    };
    EntityDetailDto {
        id: e.header.id.0,
        type_name: e.type_name,
        type_version: e.type_version,
        version: e.header.version,
        aliases: e.aliases,
        merged_from: e.merged_from.into_iter().map(|m| m.0).collect(),
        redirected_from: view.redirected_from.map(|r| r.0),
        changeset_id: e.changeset_id.0,
        updated_at_ms: e.updated_at_ms,
        fields: e.fields.into_iter().map(|f| EntityFieldDto::new(f, &mut lookup)).collect(),
    }
}

fn page_claim(c: ClaimReference) -> PageClaimDto {
    let status = match c.status {
        ClaimRefStatus::Current => "current",
        ClaimRefStatus::Superseded => "superseded",
        ClaimRefStatus::Missing => "missing",
    };
    let mut dto = PageClaimDto {
        claim_id: c.claim_id.0,
        status: status.into(),
        current_claim_id: None,
        subject: None,
        predicate: None,
        value: None,
        epistemic: None,
        citations: vec![],
    };
    if let Some(EntityClaim { assertion, evidence }) = c.current {
        dto.current_claim_id = Some(assertion.header.id.0);
        dto.epistemic = Some(json_text(&assertion.epistemic));
        dto.subject = Some(assertion.subject);
        dto.predicate = Some(assertion.predicate);
        dto.value = Some(assertion.value);
        dto.citations = evidence.into_iter().map(|x| FieldEvidenceDto::new(x.evidence, x.segment, x.source)).collect();
    }
    dto
}

pub fn page_detail(v: PageView) -> PageDetailDto {
    PageDetailDto {
        id: v.page_id.0,
        current_revision: v.current_revision,
        current_slug: v.current_slug,
        redirects: v.redirects,
        revision: v.content.revision,
        title: v.content.title,
        slug: v.content.slug,
        body: v.content.body,
        blocks: v.content.blocks.into_iter().map(BlockDto::from).collect(),
        claims: v.claims.into_iter().map(page_claim).collect(),
        creator: v.content.creator,
        updated_at_ms: v.content.updated_at_ms,
        restored_from: v.content.restored_from,
    }
}

/// `rows` holds one more than `limit` when there's a next page.
pub fn neighbor_page(rows: Vec<NeighborView>, limit: usize) -> NeighborPage {
    let (items, next_cursor) = page_of(rows, limit, |n| n.edge_id.0.clone());
    NeighborPage {
        items: items
            .into_iter()
            .map(|n| NeighborDto {
                edge_id: n.edge_id.0,
                kind: n.kind,
                direction: n.direction.into(),
                from: n.from.0,
                to: n.to.0,
                block_id: n.block_id.map(|b| b.0),
                neighbor: n.neighbor.map(|x| x.0),
                title: n.title,
            })
            .collect(),
        next_cursor,
    }
}

/// The stable code every frontend reports for this error.
pub fn error_code(e: &ServerError) -> &'static str {
    match e {
        ServerError::Unauthenticated => "unauthenticated",
        ServerError::Forbidden => "forbidden",
        ServerError::NotFound => "not_found",
        ServerError::Invalid(_) => "invalid",
        ServerError::Conflict(_) | ServerError::AlreadyBootstrapped => "conflict",
        ServerError::StaleVersion { .. } => "stale_version",
        ServerError::NotReversible(_) => "not_reversible",
        ServerError::TooLarge { .. } => "too_large",
        ServerError::UnsupportedMedia(_) => "unsupported_media",
        ServerError::Busy => "busy",
        ServerError::RateLimited { .. } => "rate_limited",
        ServerError::Unavailable(_) => "unavailable",
        ServerError::NotCaughtUp { .. } => "not_caught_up",
    }
}

/// Backend messages can carry internal detail, so callers get a plain one. The detail goes to the log.
pub fn error_message(e: &ServerError, request_id: &str) -> String {
    if let ServerError::Unavailable(detail) = e {
        tracing::error!(request_id, detail, "backend unavailable");
        "a backend is unavailable, so try again".to_string()
    } else {
        e.to_string()
    }
}

/// The error body, for a frontend that reports it whole.
pub fn error_detail(e: &ServerError, request_id: &str) -> ErrorDetail {
    ErrorDetail {
        code: error_code(e).into(),
        message: error_message(e, request_id),
        request_id: request_id.into(),
        current: if let ServerError::StaleVersion { current, .. } = e {
            serde_json::to_value(current).ok()
        } else {
            None
        },
    }
}
