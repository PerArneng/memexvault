import { epochMs } from "@memex/types";
import type { AnchorRect, ChangeOrigin, CitingClaim, ClaimLineage, Derivation, DocumentSource, EvidenceStep, FlaggedPage, MediaFacts, Page, PageFlag, PageList, PageOutcome, PageWord, Quad, Rendition, Segment, Transcript } from "@memex/types";
import type { components } from "./generated/schema";

type Schemas = components["schemas"];

const isRecord = (value: unknown): value is Readonly<Record<string, unknown>> => typeof value === "object" && value !== null && !Array.isArray(value);
const numberOr = (value: unknown, fallback: number): number => (typeof value === "number" ? value : fallback);
const pageOf = <W, T>(items: readonly W[], next: string | null | undefined, one: (w: W) => T): Page<T> => ({ items: items.map(one), nextCursor: next ?? null });

export const sourceFromWire = (s: Schemas["SourceDto"]): DocumentSource => ({
  id: s.id,
  name: s.name,
  sha256: s.sha256,
  mediaType: s.media_type,
  trustTier: s.trust_tier,
  size: s.size,
  logicalId: s.logical_id,
  versionNo: s.version_no,
  version: s.version,
  createdBy: s.created_by,
  recordedAt: epochMs(s.recorded_at_ms),
});

export const sourcePageFromWire = (p: Schemas["SourcePage"]): Page<DocumentSource> => pageOf(p.items, p.next_cursor, sourceFromWire);

const flagFromWire = (raw: unknown): PageFlag => {
  const flag = isRecord(raw) ? raw : {};
  switch (flag.flag) {
    case "low_confidence":
      return { kind: "low_confidence" };
    case "hidden_text":
      return { kind: "hidden_text", words: numberOr(flag.words, 0) };
    case "zero_size_text":
      return { kind: "zero_size_text", words: numberOr(flag.words, 0) };
    case "invisible_unicode":
      return { kind: "invisible_unicode", chars: numberOr(flag.chars, 0) };
    case "ocr_failed":
      return { kind: "ocr_failed", reason: typeof flag.reason === "string" ? flag.reason : "" };
    default:
      return { kind: "other", name: typeof flag.flag === "string" ? flag.flag : "unknown" };
  }
};

const outcomeFromWire = (raw: unknown): PageOutcome => {
  const o = isRecord(raw) ? raw : {};
  return {
    index: numberOr(o.index, 0),
    route: typeof o.route === "string" ? o.route : "unknown",
    meanConfidencePermille: typeof o.mean_confidence_permille === "number" ? o.mean_confidence_permille : null,
    flags: Array.isArray(o.flags) ? o.flags.map(flagFromWire) : [],
  };
};

export const renditionFromWire = (r: Schemas["RenditionDto"]): Rendition => ({
  id: r.id,
  sourceId: r.source_id,
  parser: r.parser,
  pipelineVersion: r.pipeline_version,
  pageCount: r.page_count,
  segmentCount: r.segment_count,
  createdAt: epochMs(r.created_at_ms),
  supersededBy: r.superseded_by ?? null,
  pages: r.pages.map(outcomeFromWire),
});

export const pageListFromWire = (l: Schemas["PageListDto"]): PageList => ({
  renditionId: l.rendition_id,
  allowedDpi: l.allowed_dpi,
  pages: l.pages.map((p) => ({ index: p.index, width: p.width, height: p.height, rotation: p.rotation })),
});

const quadFromWire = (raw: unknown): Quad | null => {
  if (!Array.isArray(raw) || raw.length !== 4) return null;
  const points = raw.map((p: unknown) => (Array.isArray(p) && typeof p[0] === "number" && typeof p[1] === "number" ? ([p[0], p[1]] as const) : null));
  const [a, b, c, d] = points;
  return a === null || b === null || c === null || d === null || a === undefined || b === undefined || c === undefined || d === undefined ? null : [a, b, c, d];
};

/** A word table has no schema in the document, and plain text has none at all. A recording's has times instead of rects, so it gives no page words. */
export const pageWordsFromWire = (detail: Schemas["RenditionDetail"]): readonly PageWord[] => {
  const table: unknown = detail.word_table;
  if (!isRecord(table) || !Array.isArray(table.words)) return [];
  return table.words.flatMap((raw: unknown): readonly PageWord[] => {
    if (!isRecord(raw) || typeof raw.text !== "string" || typeof raw.page !== "number" || !isRecord(raw.rect)) return [];
    const { x0, y0, x1, y1 } = raw.rect;
    if (typeof x0 !== "number" || typeof y0 !== "number" || typeof x1 !== "number" || typeof y1 !== "number") return [];
    return [{ text: raw.text, page: raw.page, rect: { x0, y0, x1, y1 }, quad: quadFromWire(raw.quad) }];
  });
};

export const segmentFromWire = (s: Schemas["SegmentDto"]): Segment => ({
  id: s.id,
  sourceId: s.source_id,
  renditionId: s.rendition_id ?? null,
  text: s.text,
  heading: s.heading ?? null,
  language: s.language ?? null,
  pageFrom: s.page_from ?? null,
  pageTo: s.page_to ?? null,
  speaker: s.speaker ?? null,
  tStartMs: s.t_start_ms ?? null,
  tEndMs: s.t_end_ms ?? null,
});

export const segmentPageFromWire = (p: Schemas["SegmentPage"]): Page<Segment> => pageOf(p.items, p.next_cursor, segmentFromWire);

export const flaggedPagesFromWire = (p: Schemas["FlaggedPagePage"]): Page<FlaggedPage> =>
  pageOf(p.items, p.next_cursor, (f) => ({ sourceId: f.source_id, renditionId: f.rendition_id, page: f.page, flags: outcomeFromWire(f.outcome).flags }));

const claimValueText = (value: unknown): string => {
  if (typeof value === "string") return value;
  return value === null || value === undefined ? "" : JSON.stringify(value);
};

export const citingClaimsFromWire = (p: Schemas["ClaimRefPage"]): Page<CitingClaim> =>
  pageOf(p.items, p.next_cursor, (c) => ({ assertionId: c.assertion_id, evidenceId: c.evidence_id, subject: c.subject, predicate: c.predicate, value: claimValueText(c.value) }));

const rectsFromWire = (rects: readonly Schemas["RectDto"][]): readonly AnchorRect[] => rects.map((r) => ({ page: r.page, x0: r.x0, y0: r.y0, x1: r.x1, y1: r.y1 }));

const evidenceFromWire = (e: Schemas["EvidenceLineageDto"]): EvidenceStep => ({
  evidenceId: e.evidence_id,
  quote: e.quote,
  stance: e.stance,
  segmentId: e.segment_id,
  renditionId: e.rendition_id,
  parser: e.parser,
  sourceId: e.source_id,
  sourceName: e.source_name,
  sourceSha256: e.source_sha256,
  trustTier: e.trust_tier,
  mediaType: e.media_type,
  superseded: e.superseded,
  broken: e.broken,
  rects: rectsFromWire(e.rects),
  tStartMs: e.t_start_ms ?? null,
  tEndMs: e.t_end_ms ?? null,
  speaker: e.speaker ?? null,
});

export const claimLineageFromWire = (claimId: string, l: Schemas["LineageDto"]): ClaimLineage => {
  const row: Readonly<Record<string, unknown>> = isRecord(l.assertion) ? l.assertion : {};
  const text = (key: string): string | null => {
    const value = row[key];
    return typeof value === "string" ? value : null;
  };
  return {
    claimId,
    subject: text("subject"),
    predicate: text("predicate"),
    value: claimValueText(row.value),
    epistemic: text("epistemic"),
    supersededBy: text("superseded_by"),
    changesetId: l.changeset_id,
    proposer: l.proposer ?? null,
    proposerIsMachine: l.proposer_is_machine ?? null,
    decidedBy: l.decided_by ?? null,
    rationale: l.rationale ?? null,
    evidence: l.evidence.map(evidenceFromWire),
  };
};

export const changeOriginFromWire = (c: Schemas["ChangeSetDto"]): ChangeOrigin => ({ extractor: c.extractor ?? null, promptVersion: c.prompt_version ?? null, state: c.state });

export const derivationPageFromWire = (p: Schemas["DerivationPage"]): Page<Derivation> =>
  pageOf(p.items, p.next_cursor, (d) => ({ id: d.id, kind: d.kind, model: d.model, promptHash: d.prompt_hash, outcome: d.outcome, segmentIds: d.segment_ids, createdAt: epochMs(d.created_at_ms) }));

export const mediaFromWire = (m: Schemas["MediaInfoDto"]): MediaFacts => ({
  sourceId: m.source_id,
  mediaType: m.media_type,
  size: m.size,
  renditionId: m.rendition_id ?? null,
  durationMs: m.duration_ms ?? null,
  gaps: m.gaps ?? 0,
  skippedSpans: m.skipped_spans ?? 0,
  coarseSpans: m.coarse_spans ?? 0,
});

export const transcriptFromWire = (t: Schemas["TranscriptDto"]): Transcript => ({
  renditionId: t.rendition_id,
  parser: t.parser,
  durationMs: t.duration_ms,
  nextCursor: t.next_cursor ?? null,
  parts: t.parts.map((p) => ({
    start: p.start,
    text: p.text,
    tStartMs: p.t_start_ms,
    tEndMs: p.t_end_ms,
    speaker: p.speaker ?? null,
    words: (p.words ?? []).map((w) => ({ text: w.text, startMs: w.start_ms, endMs: w.end_ms })),
  })),
});

export const proposalOfTrust = (id: string, baseVersion: number, tier: string): Readonly<Record<string, unknown>> => ({ op: "set_source_trust", id, base_version: baseVersion, trust_tier: tier });
