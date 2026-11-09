import type { ActivityPage, DocumentActivity, Job, UploadLimits, VaultId } from "@memex/types";
import { failed, ok } from "./api";
import type { ApiFailure } from "./api";
import type { FeedEvent, ActivityFeed } from "./activity-watch";
import type { IngestApi } from "./ingest-cases";
import type { RunningUpload, UploadFile, Uploader, UploadOptions } from "./upload-queue";
import type { UploadedSource } from "@memex/types";

export interface IngestSeed {
  readonly documents?: readonly DocumentActivity[];
  readonly head?: number;
  readonly jobs?: readonly Job[];
  readonly limits?: UploadLimits;
}

const NOT_FOUND: ApiFailure = { kind: "http", status: 404, code: "not_found", message: "not_found", requestId: "req-1" };

export const documentActivity = (sourceId: string, over: Partial<DocumentActivity> = {}): DocumentActivity => ({
  sourceId,
  logicalId: sourceId,
  versionNo: 1,
  recordedAtMs: 0,
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
  ...over,
});

export const jobNamed = (id: string, over: Partial<Job> = {}): Job => ({ id, kind: "rendition", state: "queued", sourceId: null, maxAttempts: 3, nextRunAtMs: 0, attempts: [], createdAtMs: 0, finishedAtMs: null, waitingSinceMs: null, ...over });

export const fakeIngestApi = (seed: IngestSeed): IngestApi => {
  const jobs = new Map((seed.jobs ?? []).map((j) => [j.id, j]));
  const page = (): ActivityPage => ({ head: seed.head ?? -1, items: seed.documents ?? [], nextCursor: null });
  const change = (vault: VaultId, id: string, state: string) => {
    void vault;
    const job = jobs.get(id);
    if (job === undefined) return Promise.resolve(failed<Job>(NOT_FOUND));
    const next = { ...job, state };
    jobs.set(id, next);
    return Promise.resolve(ok(next));
  };
  return {
    listActivityDocuments: () => Promise.resolve(ok(page())),
    getUploadLimits: () => Promise.resolve(ok(seed.limits ?? { maxBytes: 1000, allowedMediaTypes: ["application/pdf", "text/plain"] })),
    listJobs: (_vault, { state, kind }) => Promise.resolve(ok({ items: [...jobs.values()].filter((j) => (state === null || j.state === state) && (kind === null || j.kind === kind)), nextCursor: null })),
    getJob: (_vault, id) => {
      const job = jobs.get(id);
      return Promise.resolve(job === undefined ? failed<Job>(NOT_FOUND) : ok(job));
    },
    retryJob: (vault, id) => change(vault, id, "queued"),
    cancelJob: (vault, id) => change(vault, id, "cancelled"),
  };
};

/** A feed tests drive by hand. */
export interface ScriptedFeed extends ActivityFeed {
  readonly opened: readonly number[];
  emit(event: FeedEvent): void;
  readonly stopped: () => number;
}

export const scriptedFeed = (): ScriptedFeed => {
  const opened: number[] = [];
  let listener: ((event: FeedEvent) => void) | null = null;
  let stops = 0;
  return {
    opened,
    follow: (_vault, after, l) => {
      opened.push(after);
      listener = l;
      return () => {
        stops += 1;
        listener = null;
      };
    },
    emit: (event) => listener?.(event),
    stopped: () => stops,
  };
};

/** An uploader tests finish by hand. */
export interface ScriptedUploader extends Uploader {
  readonly started: readonly { readonly name: string; readonly options: UploadOptions }[];
  readonly aborted: readonly string[];
  progress(name: string, sent: number): void;
  finish(name: string, result: { readonly ok: true; readonly value: UploadedSource } | { readonly ok: false; readonly failure: ApiFailure }): Promise<void>;
}

export const scriptedUploader = (): ScriptedUploader => {
  const started: { name: string; options: UploadOptions }[] = [];
  const aborted: string[] = [];
  const open = new Map<string, { resolve: (r: Awaited<RunningUpload["result"]>) => void; onProgress: (sent: number) => void }>();
  return {
    started,
    aborted,
    start: (_vault, file: UploadFile, options, onProgress) => {
      started.push({ name: file.name, options });
      const result = new Promise<Awaited<RunningUpload["result"]>>((resolve) => {
        open.set(file.name, { resolve, onProgress });
      });
      return { result, abort: () => aborted.push(file.name) };
    },
    progress: (name, sent) => open.get(name)?.onProgress(sent),
    finish: async (name, result) => {
      open.get(name)?.resolve(result);
      open.delete(name);
      await Promise.resolve();
      await Promise.resolve();
    },
  };
};
