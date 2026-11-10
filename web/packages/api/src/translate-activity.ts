import type { ActivityFrame, ActivityPage, DocumentActivity, DurableFrame, Job, JobAttempt, JobFact, Page, UploadLimits, UploadedSource } from "@memex/types";
import type { components } from "./generated/schema";
import { sourceFromWire } from "./translate-documents";

type Schemas = components["schemas"];

export const documentActivityFromWire = (d: Schemas["DocumentActivityDto"]): DocumentActivity => ({
  sourceId: d.source_id,
  logicalId: d.logical_id,
  versionNo: d.version_no,
  recordedAtMs: d.recorded_at_ms,
  stage: d.stage,
  status: d.status,
  partial: d.partial,
  attempts: d.attempts,
  jobId: d.job_id ?? null,
  nextRunAtMs: d.next_run_at_ms ?? null,
  waitingSinceMs: d.waiting_since_ms ?? null,
  pageCount: d.page_count ?? null,
  segmentCount: d.segment_count ?? null,
  segmentsExtracted: d.segments_extracted ?? null,
  pendingReview: d.pending_review,
  renditionId: d.rendition_id ?? null,
  progress: d.progress === null || d.progress === undefined ? null : { stage: d.progress.stage, done: d.progress.done, total: d.progress.total ?? null, timeMs: d.progress.time_ms },
});

export const activityPageFromWire = (p: Schemas["ActivityDocumentPage"]): ActivityPage => ({ head: p.head, items: p.items.map(documentActivityFromWire), nextCursor: p.next_cursor ?? null });

const jobFactFromWire = (j: Schemas["JobFact"]): JobFact => ({
  id: j.id,
  kind: j.kind,
  state: j.state,
  stage: j.stage ?? null,
  status: j.status,
  attempts: j.attempts,
  nextRunAtMs: j.next_run_at_ms,
  waitingSinceMs: j.waiting_since_ms ?? null,
  sourceId: j.source_id ?? null,
});

/** The rows that decide a stage and that a frame says too little about: an embedding job's documents, a claim awaiting review. */
const UNPLACED_COUNTS: readonly string[] = ["changeset", "review_verdict", "auto_commit", "segment", "assertion", "derivation"];

const durableFromWire = (f: Schemas["DurableFrame"]): DurableFrame => {
  const jobs = (f.jobs ?? []).map(jobFactFromWire);
  const counts = f.counts ?? {};
  return {
    kind: "durable",
    seq: f.seq,
    timeMs: f.time_ms,
    eventKind: f.kind,
    sources: (f.sources ?? []).map((s) => ({ id: s.id, logicalId: s.logical_id ?? null, versionNo: s.version_no })),
    jobs,
    renditions: (f.renditions ?? []).map((r) => ({ id: r.id, sourceId: r.source_id ?? null, pageCount: r.page_count, segmentCount: r.segment_count, superseded: r.superseded })),
    extractions: (f.extractions ?? []).map((x) => ({ sourceId: x.source_id ?? null, segmentsDone: x.segments_done, batchesFailed: x.batches_failed, stop: x.stop ?? null })),
    // A frame cut short, or any job not tied to a document, may also have moved one.
    unplaced: (f.omitted ?? 0) > 0 || jobs.some((j) => j.sourceId === null) || UNPLACED_COUNTS.some((name) => (counts[name] ?? 0) > 0),
  };
};

const isObject = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null;

/** One frame off the wire. Whatever can't be read is `unknown`, as a newer server's frame mustn't stop the stream. */
export const activityFrameFromWire = (name: string, data: string): ActivityFrame => {
  let parsed: unknown;
  try {
    parsed = JSON.parse(data);
  } catch {
    return { kind: "unknown", name };
  }
  if (!isObject(parsed)) return { kind: "unknown", name };
  switch (name) {
    case "hello": {
      const f = parsed as unknown as Schemas["HelloFrame"];
      return { kind: "hello", head: f.head, resumed: f.resumed };
    }
    case "durable":
      return typeof parsed.seq === "number" ? durableFromWire(parsed as unknown as Schemas["DurableFrame"]) : { kind: "unknown", name };
    case "progress": {
      const f = parsed as unknown as Schemas["ProgressFrame"];
      return { kind: "progress", jobId: f.job_id, sourceId: f.source_id ?? null, stage: f.stage, done: f.done, total: f.total ?? null, timeMs: f.time_ms };
    }
    case "resync": {
      const f = parsed as unknown as Schemas["ResyncFrame"];
      return { kind: "resync", head: f.head, reason: f.reason };
    }
    case "heartbeat":
      return { kind: "heartbeat", head: (parsed as unknown as Schemas["HeartbeatFrame"]).head };
    case "end":
      return { kind: "end", reason: (parsed as unknown as Schemas["EndFrame"]).reason };
    default:
      return { kind: "unknown", name };
  }
};

const attemptFromWire = (a: unknown): JobAttempt => {
  // The API documents an attempt only as an object, so nothing about its shape is taken for granted.
  const o = isObject(a) ? a : {};
  return {
    startedAtMs: typeof o.started_at_ms === "number" ? o.started_at_ms : 0,
    finishedAtMs: typeof o.finished_at_ms === "number" ? o.finished_at_ms : null,
    error: typeof o.error === "string" ? o.error : null,
  };
};

export const jobFromWire = (j: Schemas["JobDto"]): Job => {
  const input: unknown = j.input;
  const source = isObject(input) ? input.source_id : null;
  return {
    id: j.id,
    kind: j.kind,
    state: j.state,
    sourceId: typeof source === "string" ? source : null,
    maxAttempts: j.max_attempts,
    nextRunAtMs: j.next_run_at_ms,
    attempts: j.attempts.map(attemptFromWire),
    createdAtMs: j.created_at_ms,
    finishedAtMs: j.finished_at_ms ?? null,
    waitingSinceMs: j.waiting_since_ms ?? null,
  };
};

export const jobPageFromWire = (p: Schemas["JobPage"]): Page<Job> => ({ items: p.items.map(jobFromWire), nextCursor: p.next_cursor ?? null });

export const uploadLimitsFromWire = (l: Schemas["UploadLimitsDto"]): UploadLimits => ({ maxBytes: l.max_bytes, allowedMediaTypes: l.allowed_media_types });

export const uploadResultFromWire = (r: Schemas["UploadResult"]): UploadedSource => ({ source: sourceFromWire(r.source), existing: r.existing, jobId: r.job_id ?? null });
