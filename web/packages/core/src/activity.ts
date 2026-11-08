import type { ActivityFrame, DocumentActivity, DurableFrame, JobFact, ProgressFrame } from "@memex/types";

/** The server's order. See the stage model in `docs/architecture.md`, section Live activity. */
export const STAGES = ["stored", "parsing", "ocr", "transcription", "segmenting", "indexing", "embedding", "extraction", "review", "done"] as const;
export type StageName = (typeof STAGES)[number];

/** A browser list shouldn't hold more than this, as with sources: 50 pages of 200. */
export const MAX_BOARD_DOCUMENTS = 10_000;
const MAX_HISTORY = 32;
const THROUGHPUT_WINDOW_MS = 300_000;
const RATE_WINDOW_MS = 10_000;

export interface StageVisit {
  readonly stage: string;
  readonly atMs: number;
}

export interface BoardDocument extends DocumentActivity {
  readonly stageSinceMs: number;
  /** The stages this console saw it in, oldest first. A stage too short to be seen isn't here. */
  readonly history: readonly StageVisit[];
  /** Late progress from a finished job mustn't move the document back. */
  readonly lastFinishedJob: string | null;
}

export type Connection =
  | { readonly status: "connecting" }
  | { readonly status: "live" }
  | { readonly status: "reconnecting"; readonly sinceMs: number }
  /** It won't come back by itself, and the reason says why. */
  | { readonly status: "ended"; readonly reason: string };

export interface ActivityState {
  readonly documents: ReadonlyMap<string, BoardDocument>;
  /** The last durable position applied. Null before the first snapshot. */
  readonly seq: number | null;
  readonly connection: Connection;
  /** More documents than a browser list holds, so this shows only the first of them. */
  readonly truncated: boolean;
  /** Something happened that frames can't place, so a new snapshot is due soon. */
  readonly dirty: boolean;
  /** The picture may be wrong until a new snapshot comes. */
  readonly needsSnapshot: boolean;
  readonly doneTimes: readonly number[];
  readonly frameTimes: readonly number[];
  readonly lastFrameAtMs: number | null;
  /** Durable frames already applied, or older than what's applied. */
  readonly ignoredFrames: number;
  readonly unknownFrames: number;
}

export const INITIAL_ACTIVITY: ActivityState = {
  documents: new Map(),
  seq: null,
  connection: { status: "connecting" },
  truncated: false,
  dirty: false,
  needsSnapshot: false,
  doneTimes: [],
  frameTimes: [],
  lastFrameAtMs: null,
  ignoredFrames: 0,
  unknownFrames: 0,
};

export type ActivityEvent =
  | { readonly type: "snapshot"; readonly head: number; readonly documents: readonly DocumentActivity[]; readonly truncated: boolean; readonly replay: readonly DurableFrame[]; readonly at: number }
  | { readonly type: "frame"; readonly frame: ActivityFrame; readonly at: number }
  | { readonly type: "lost"; readonly at: number }
  | { readonly type: "connecting" }
  | { readonly type: "ended"; readonly reason: string };

export const stageIndex = (stage: string): number => (STAGES as readonly string[]).indexOf(stage);

const PARSE_KINDS: readonly string[] = ["rendition", "transcription"];

/** A plain working copy for one batch: the map is copied once, however many frames the batch holds. */
interface Draft {
  documents: Map<string, BoardDocument>;
  seq: number | null;
  connection: Connection;
  truncated: boolean;
  dirty: boolean;
  needsSnapshot: boolean;
  doneTimes: number[];
  frameTimes: number[];
  lastFrameAtMs: number | null;
  ignoredFrames: number;
  unknownFrames: number;
}

const moved = (doc: BoardDocument, stage: string, at: number, draft: Draft): BoardDocument => {
  if (stage === doc.stage) return doc;
  if (stage === "done") draft.doneTimes.push(at);
  return { ...doc, stage, stageSinceMs: at, history: [...doc.history, { stage, atMs: at }].slice(-MAX_HISTORY) };
};

const newDocument = (d: DocumentActivity, at: number): BoardDocument => {
  const since = d.stage === "stored" ? d.recordedAtMs : at;
  return { ...d, stageSinceMs: since, history: [{ stage: d.stage, atMs: since }], lastFinishedJob: null };
};

const applyParseJob = (draft: Draft, doc: BoardDocument, job: JobFact, at: number): BoardDocument => {
  if (doc.jobId !== null && job.id < doc.jobId) return doc;
  const facts = { attempts: job.attempts, nextRunAtMs: job.nextRunAtMs, waitingSinceMs: job.waitingSinceMs };
  switch (job.state) {
    case "done":
      // A rendition comes with it or just before, and the slowest index comes next. The next snapshot says exactly.
      draft.dirty = true;
      return { ...moved(doc, "indexing", at, draft), ...facts, status: "active", jobId: null, attempts: 0, progress: null, lastFinishedJob: job.id };
    case "queued":
      if (job.attempts === 0) return { ...moved(doc, "stored", at, draft), ...facts, status: job.status, jobId: job.id };
      return { ...moved(doc, job.stage ?? "parsing", at, draft), ...facts, status: job.status, jobId: job.id, progress: null };
    case "running": {
      // Progress travels faster than the log, so it may already have said where this job is.
      const said = doc.progress !== null && doc.jobId === job.id ? doc.progress.stage : (job.stage ?? "parsing");
      return { ...moved(doc, said, at, draft), ...facts, status: job.status, jobId: job.id };
    }
    default:
      return { ...moved(doc, job.stage ?? "parsing", at, draft), ...facts, status: "failed", jobId: job.id, progress: null };
  }
};

const applyExtractJob = (draft: Draft, doc: BoardDocument, job: JobFact, at: number): BoardDocument => {
  draft.dirty = true;
  // The earliest unfinished work decides, so a later stage can't hide what's wrong with an earlier one.
  if (stageIndex(doc.stage) < stageIndex("indexing") || (doc.jobId !== null && doc.jobId !== job.id && job.id < doc.jobId)) return doc;
  const facts = { attempts: job.attempts, nextRunAtMs: job.nextRunAtMs, waitingSinceMs: job.waitingSinceMs };
  if (job.state === "done") return { ...doc, ...facts, jobId: null, progress: null, lastFinishedJob: job.id };
  return { ...moved(doc, "extraction", at, draft), ...facts, status: job.status, jobId: job.id };
};

const applyJob = (draft: Draft, job: JobFact, at: number) => {
  if (job.sourceId === null) {
    draft.dirty = true;
    return;
  }
  const doc = draft.documents.get(job.sourceId);
  if (doc === undefined) {
    draft.dirty = true;
    return;
  }
  if (PARSE_KINDS.includes(job.kind)) draft.documents.set(doc.sourceId, applyParseJob(draft, doc, job, at));
  else if (job.kind === "extract") draft.documents.set(doc.sourceId, applyExtractJob(draft, doc, job, at));
  else draft.dirty = true;
};

const applyDurable = (draft: Draft, frame: DurableFrame, at: number) => {
  if (draft.seq === null || frame.seq <= draft.seq) {
    draft.ignoredFrames += 1;
    return;
  }
  // Positions follow each other, so a jump means something went missing.
  if (frame.seq > draft.seq + 1) draft.needsSnapshot = true;
  draft.seq = frame.seq;
  if (frame.unplaced) draft.dirty = true;
  for (const source of frame.sources) {
    if (draft.documents.has(source.id)) continue;
    if (draft.documents.size >= MAX_BOARD_DOCUMENTS) {
      draft.truncated = true;
      continue;
    }
    draft.documents.set(source.id, newDocument({
      sourceId: source.id,
      logicalId: source.logicalId ?? source.id,
      versionNo: source.versionNo,
      recordedAtMs: frame.timeMs,
      stage: "stored",
      status: "queued",
      partial: false,
      attempts: 0,
      jobId: null,
      nextRunAtMs: null,
      waitingSinceMs: null,
      pageCount: null,
      segmentCount: null,
      segmentsExtracted: null,
      pendingReview: 0,
      renditionId: null,
      progress: null,
    }, at));
  }
  for (const job of frame.jobs) applyJob(draft, job, at);
  for (const r of frame.renditions) {
    const doc = r.sourceId === null ? undefined : draft.documents.get(r.sourceId);
    if (doc !== undefined && !r.superseded) draft.documents.set(doc.sourceId, { ...doc, renditionId: r.id, pageCount: r.pageCount, segmentCount: r.segmentCount });
  }
  for (const x of frame.extractions) {
    const doc = x.sourceId === null ? undefined : draft.documents.get(x.sourceId);
    if (doc !== undefined) draft.documents.set(doc.sourceId, { ...doc, segmentsExtracted: x.segmentsDone, partial: doc.partial || x.batchesFailed > 0 || x.stop !== null });
  }
};

const applyProgress = (draft: Draft, frame: ProgressFrame, at: number) => {
  const doc = frame.sourceId === null ? undefined : draft.documents.get(frame.sourceId);
  if (doc === undefined) return;
  const stale =
    (doc.progress !== null && frame.timeMs < doc.progress.timeMs) ||
    (doc.lastFinishedJob !== null && frame.jobId <= doc.lastFinishedJob) ||
    (doc.jobId !== null && frame.jobId < doc.jobId) ||
    (doc.jobId === frame.jobId && doc.status === "failed") ||
    // A snapshot already past this work, without this console having seen the job end.
    (doc.jobId === null && stageIndex(frame.stage) >= 0 && stageIndex(frame.stage) < stageIndex(doc.stage));
  if (stale) {
    draft.ignoredFrames += 1;
    return;
  }
  draft.documents.set(doc.sourceId, {
    ...moved(doc, frame.stage, at, draft),
    status: "active",
    jobId: frame.jobId,
    waitingSinceMs: null,
    progress: { stage: frame.stage, done: frame.done, total: frame.total, timeMs: frame.timeMs },
  });
};

const applySnapshot = (draft: Draft, event: Extract<ActivityEvent, { type: "snapshot" }>) => {
  const before = draft.documents;
  const after = new Map<string, BoardDocument>();
  for (const d of event.documents) {
    const prior = before.get(d.sourceId);
    if (prior === undefined) {
      after.set(d.sourceId, newDocument(d, event.at));
      continue;
    }
    const placed = moved(prior, d.stage, event.at, draft);
    after.set(d.sourceId, { ...placed, ...d, stage: placed.stage });
  }
  draft.documents = after;
  draft.truncated = event.truncated;
  draft.seq = event.head;
  draft.dirty = false;
  draft.needsSnapshot = false;
  for (const frame of event.replay) if (frame.seq > event.head) applyDurable(draft, frame, event.at);
};

const applyFrame = (draft: Draft, frame: ActivityFrame, at: number) => {
  draft.lastFrameAtMs = at;
  switch (frame.kind) {
    case "hello":
      // It carried on from a position, so what's durable is safe, but progress and anything before the loss may be old.
      if (draft.connection.status === "reconnecting") draft.needsSnapshot = true;
      draft.connection = { status: "live" };
      break;
    case "durable":
      draft.frameTimes.push(at);
      applyDurable(draft, frame, at);
      break;
    case "progress":
      draft.frameTimes.push(at);
      applyProgress(draft, frame, at);
      break;
    case "resync":
      // The stream carries on from here, so this can move back as well as forward.
      draft.seq = frame.head;
      draft.needsSnapshot = true;
      break;
    case "heartbeat":
      break;
    case "end":
      draft.connection = { status: "ended", reason: frame.reason };
      break;
    case "unknown":
      draft.unknownFrames += 1;
      break;
  }
};

/** Pure. Takes a batch, so a burst of frames costs one copy of the map and one render, however long it is. */
export const activityReducer = (state: ActivityState, events: readonly ActivityEvent[]): ActivityState => {
  if (events.length === 0) return state;
  const draft: Draft = { ...state, documents: new Map(state.documents), doneTimes: [...state.doneTimes], frameTimes: [...state.frameTimes] };
  let latest = 0;
  for (const event of events) {
    switch (event.type) {
      case "snapshot":
        latest = Math.max(latest, event.at);
        applySnapshot(draft, event);
        break;
      case "frame":
        latest = Math.max(latest, event.at);
        applyFrame(draft, event.frame, event.at);
        break;
      case "lost":
        latest = Math.max(latest, event.at);
        if (draft.connection.status !== "reconnecting" && draft.connection.status !== "ended") draft.connection = { status: "reconnecting", sinceMs: event.at };
        break;
      case "connecting":
        if (draft.connection.status === "live") draft.connection = { status: "connecting" };
        break;
      case "ended":
        draft.connection = { status: "ended", reason: event.reason };
        break;
    }
  }
  draft.doneTimes = draft.doneTimes.filter((t) => t > latest - THROUGHPUT_WINDOW_MS);
  draft.frameTimes = draft.frameTimes.filter((t) => t > latest - RATE_WINDOW_MS);
  return draft;
};

export const stageLabel = (stage: string): string => {
  switch (stage) {
    case "stored":
      return "Stored";
    case "parsing":
      return "Parsing";
    case "ocr":
      return "OCR";
    case "transcription":
      return "Transcription";
    case "segmenting":
      return "Segmenting";
    case "indexing":
      return "Indexing";
    case "embedding":
      return "Embedding";
    case "extraction":
      return "Extraction";
    case "review":
      return "Review";
    case "done":
      return "Done";
    default:
      return stage;
  }
};

export interface StageCount {
  readonly stage: string;
  readonly count: number;
  /** How many of them have something wrong, so a person should look. */
  readonly failed: number;
}

/** Every known stage in order, then any a newer server named. */
export const stageCounts = (documents: Iterable<DocumentActivity>): readonly StageCount[] => {
  const counts = new Map<string, { count: number; failed: number }>(STAGES.map((s) => [s, { count: 0, failed: 0 }]));
  for (const d of documents) {
    const entry = counts.get(d.stage) ?? { count: 0, failed: 0 };
    entry.count += 1;
    if (d.status === "failed") entry.failed += 1;
    counts.set(d.stage, entry);
  }
  return [...counts].map(([stage, c]) => ({ stage, ...c }));
};

export interface BoardFilter {
  readonly stage: string;
  readonly status: string;
  readonly text: string;
}

const needsAttention = (d: BoardDocument): number => (d.status === "failed" ? 0 : d.status === "waiting" || d.status === "retrying" ? 1 : d.stage === "done" ? 3 : 2);

/** What someone has to act on first, then what's moving, and what's finished last. Newest first within each. */
export const boardRows = (documents: Iterable<BoardDocument>, filter: BoardFilter, nameOf: (id: string) => string | undefined): readonly BoardDocument[] => {
  const text = filter.text.trim().toLowerCase();
  const rows: BoardDocument[] = [];
  for (const d of documents) {
    if (filter.stage !== "" && d.stage !== filter.stage) continue;
    if (filter.status !== "" && d.status !== filter.status) continue;
    if (text !== "" && !d.sourceId.toLowerCase().includes(text) && nameOf(d.sourceId)?.toLowerCase().includes(text) !== true) continue;
    rows.push(d);
  }
  return rows.sort((a, b) => needsAttention(a) - needsAttention(b) || b.recordedAtMs - a.recordedAtMs || (a.sourceId < b.sourceId ? -1 : 1));
};

/** Finished documents per minute, over what the window has seen so far. */
export const throughputPerMinute = (state: ActivityState, nowMs: number): number => {
  const recent = state.doneTimes.filter((t) => t > nowMs - THROUGHPUT_WINDOW_MS);
  if (recent.length === 0) return 0;
  const first = recent[0] ?? nowMs;
  return (recent.length * 60_000) / Math.max(nowMs - first, 60_000);
};

export const framesPerSecond = (state: ActivityState, nowMs: number): number => state.frameTimes.filter((t) => t > nowMs - RATE_WINDOW_MS).length / (RATE_WINDOW_MS / 1000);

export const progressText = (p: DocumentActivity["progress"]): string | null => {
  if (p === null) return null;
  const of = p.total === null ? String(p.done) : `${String(p.done)} of ${String(p.total)}`;
  switch (p.stage) {
    case "ocr":
      return `OCR page ${of}`;
    case "transcription":
      return p.total === null ? "Transcribing" : `Transcribing, ${of}`;
    case "embedding":
      return `Embedding ${of}`;
    case "extraction":
      return `Extracting segment ${of}`;
    case "parsing":
    case "segmenting":
      return p.total === null ? null : `${stageLabel(p.stage)} ${of}`;
    default:
      return `${stageLabel(p.stage)} ${of}`;
  }
};

export const durationText = (ms: number): string => {
  const seconds = Math.max(0, Math.floor(ms / 1000));
  if (seconds < 60) return `${String(seconds)} s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${String(minutes)} min ${String(seconds % 60)} s`;
  return `${String(Math.floor(minutes / 60))} h ${String(minutes % 60)} min`;
};

/** What a state means, in words for someone who doesn't know the pipeline. Null when the badge says enough. */
export const statusExplanation = (d: DocumentActivity, nowMs: number): string | null => {
  switch (d.status) {
    case "waiting": {
      const since = d.waitingSinceMs === null ? "" : ` It has been waiting for ${durationText(nowMs - d.waitingSinceMs)}.`;
      return d.stage === "extraction" || d.stage === "review"
        ? `Waiting for the LLM, the language model service that reads this document. It can't be reached right now. Nothing has failed and no attempt is used up, and this carries on by itself when the service is back.${since}`
        : `Waiting for a service this step needs, which can't be reached right now. Nothing has failed, and this carries on by itself when it's back.${since}`;
    }
    case "retrying": {
      const next = d.nextRunAtMs === null ? "soon" : d.nextRunAtMs <= nowMs ? "any moment now" : `in ${durationText(d.nextRunAtMs - nowMs)}`;
      return `The last attempt failed, and it will try again ${next}.`;
    }
    case "failed":
      return "This stopped and won't carry on by itself. The attempts below say why, and you can retry it.";
    case "partial":
      return "It finished, but part of it didn't work, such as a page OCR couldn't read.";
    default:
      return null;
  }
};

/** Whether a stage is one this document goes through at all. */
const pipelineOf = (d: BoardDocument): readonly string[] => {
  const seen = new Set(d.history.map((h) => h.stage));
  seen.add(d.stage);
  const recording = seen.has("transcription");
  return STAGES.filter((s) => (s === "ocr" ? seen.has("ocr") : s === "transcription" ? recording : s === "parsing" ? !recording || seen.has("parsing") : true));
};

export interface PipelineStep {
  readonly stage: string;
  /** `seen` went past with this console watching, and `passed` was over before it could see it. */
  readonly state: "seen" | "passed" | "current" | "todo";
  readonly atMs: number | null;
}

/** Every stage in order for one document, marked by how far it has got. */
export const pipelineSteps = (d: BoardDocument): readonly PipelineStep[] => {
  const stages = pipelineOf(d);
  const current = stages.indexOf(d.stage);
  const steps = stages.map((stage, i): PipelineStep => {
    const visit = d.history.findLast((h) => h.stage === stage);
    if (i === current) return { stage, state: "current", atMs: visit?.atMs ?? null };
    if (current >= 0 && i > current) return { stage, state: "todo", atMs: null };
    return { stage, state: visit === undefined ? "passed" : "seen", atMs: visit?.atMs ?? null };
  });
  return current >= 0 ? steps : [...steps, { stage: d.stage, state: "current", atMs: d.stageSinceMs }];
};
