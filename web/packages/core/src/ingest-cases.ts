import type { ActivityPage, Job, Page, UploadLimits, VaultId } from "@memex/types";
import { createActivityWatch } from "./activity-watch";
import type { ActivityFeed, ActivityWatch } from "./activity-watch";
import { failed, ok } from "./api";
import type { ApiFailure, ApiResult } from "./api";
import type { Clock } from "./clock";
import { sectionPath } from "./navigation";
import { createUploadQueue } from "./upload-queue";
import type { Uploader, UploadQueue } from "./upload-queue";
import type { IdSource } from "./wiki-cases";

/** Routes the ingest screen reads and acts through. Part of `Api`. */
export interface IngestApi {
  listActivityDocuments(vault: VaultId, cursor: string | null): Promise<ApiResult<ActivityPage>>;
  getUploadLimits(vault: VaultId): Promise<ApiResult<UploadLimits>>;
  listJobs(vault: VaultId, filter: { readonly state: string | null; readonly kind: string | null }, cursor: string | null): Promise<ApiResult<Page<Job>>>;
  getJob(vault: VaultId, id: string): Promise<ApiResult<Job>>;
  retryJob(vault: VaultId, id: string): Promise<ApiResult<Job>>;
  cancelJob(vault: VaultId, id: string): Promise<ApiResult<Job>>;
}

export interface JobList {
  readonly jobs: readonly Job[];
  /** There are more than can be listed in the browser. */
  readonly truncated: boolean;
}

export interface BulkRetryOutcome {
  readonly retried: readonly string[];
  readonly failed: readonly { readonly id: string; readonly failure: ApiFailure }[];
}

export interface IngestCases {
  watchActivity(vault: VaultId): ActivityWatch;
  getUploadLimits(vault: VaultId): Promise<ApiResult<UploadLimits>>;
  createUploadQueue(vault: VaultId, limits: UploadLimits, onStored: () => void): UploadQueue;
  listJobsInState(vault: VaultId, state: string): Promise<ApiResult<JobList>>;
  /** Every job about one document that this browser can find, oldest first. `current` is the job the board names, found even when the list is cut short. */
  listDocumentJobs(vault: VaultId, source: string, current: string | null): Promise<ApiResult<JobList>>;
  retryJob(vault: VaultId, id: string): Promise<ApiResult<Job>>;
  cancelJob(vault: VaultId, id: string): Promise<ApiResult<Job>>;
  /** One after another, so it can't starve the writer. A failure doesn't stop the rest. */
  retryJobs(vault: VaultId, ids: readonly string[]): Promise<BulkRetryOutcome>;
}

const MAX_JOB_PAGES = 50;
const DOCUMENT_JOB_KINDS = ["rendition", "transcription", "extract"] as const;
/** Only the unfinished and the failed are worth looking through many pages for. */
const MAX_DOCUMENT_JOB_PAGES = 10;

interface IngestDeps {
  readonly edges: { readonly api: IngestApi; readonly clock: Clock; readonly ids: IdSource; readonly activity?: ActivityFeed; readonly uploader?: Uploader };
  readonly watched: <T>(call: Promise<ApiResult<T>>) => Promise<ApiResult<T>>;
}

/** Only tests of other screens leave these edges out, and those never open this one. */
const NO_FEED: ActivityFeed = { follow: () => () => undefined };
const NO_UPLOADER: Uploader = {
  start: () => ({ result: Promise.resolve(failed({ kind: "unreachable", message: "no uploader" })), abort: () => undefined }),
};

export const createIngestCases = ({ edges, watched }: IngestDeps): IngestCases => {
  const { api, clock, ids } = edges;

  const allJobs = async (vault: VaultId, filter: { readonly state: string | null; readonly kind: string | null }, maxPages: number): Promise<ApiResult<JobList>> => {
    const jobs: Job[] = [];
    let cursor: string | null = null;
    for (let n = 0; n < maxPages; n += 1) {
      const result: ApiResult<Page<Job>> = await watched(api.listJobs(vault, filter, cursor));
      if (!result.ok) return failed(result.failure);
      jobs.push(...result.value.items);
      cursor = result.value.nextCursor;
      if (cursor === null) return ok({ jobs, truncated: false });
    }
    return ok({ jobs, truncated: true });
  };

  return {
    watchActivity: (vault) => createActivityWatch({ vault, clock, feed: edges.activity ?? NO_FEED, page: (v, cursor) => watched(api.listActivityDocuments(v, cursor)) }),
    getUploadLimits: (vault) => watched(api.getUploadLimits(vault)),
    createUploadQueue: (vault, limits, onStored) => {
      const uploader = edges.uploader ?? NO_UPLOADER;
      return createUploadQueue({
        vault,
        limits,
        nextId: () => ids.next(),
        onStored,
        uploader: {
          start: (v, file, options, onProgress) => {
            const upload = uploader.start(v, file, options, onProgress);
            return { result: watched(upload.result), abort: () => upload.abort() };
          },
        },
      });
    },
    listJobsInState: (vault, state) => allJobs(vault, { state, kind: null }, MAX_JOB_PAGES),
    listDocumentJobs: async (vault, source, current) => {
      const found = new Map<string, Job>();
      let truncated = false;
      for (const kind of DOCUMENT_JOB_KINDS) {
        const result = await allJobs(vault, { state: null, kind }, MAX_DOCUMENT_JOB_PAGES);
        if (!result.ok) return failed(result.failure);
        truncated = truncated || result.value.truncated;
        for (const job of result.value.jobs) if (job.sourceId === source) found.set(job.id, job);
      }
      if (current !== null && !found.has(current)) {
        const job = await watched(api.getJob(vault, current));
        if (job.ok && job.value.sourceId === source) found.set(job.value.id, job.value);
      }
      return ok({ jobs: [...found.values()].sort((a, b) => a.createdAtMs - b.createdAtMs || (a.id < b.id ? -1 : 1)), truncated });
    },
    retryJob: (vault, id) => watched(api.retryJob(vault, id)),
    cancelJob: (vault, id) => watched(api.cancelJob(vault, id)),
    retryJobs: async (vault, list) => {
      const retried: string[] = [];
      const problems: { readonly id: string; readonly failure: ApiFailure }[] = [];
      for (const id of list) {
        const result = await watched(api.retryJob(vault, id));
        if (result.ok) retried.push(id);
        else problems.push({ id, failure: result.failure });
      }
      return { retried, failed: problems };
    },
  };
};

export type IngestRoute =
  | { readonly kind: "board"; readonly stage: string; readonly status: string; readonly text: string }
  | { readonly kind: "document"; readonly id: string }
  | { readonly kind: "failed"; readonly state: "dead" | "cancelled" }
  | { readonly kind: "unknown" };

/** The `-` keeps `failed` from ever meaning a document, whatever id one has. */
export const parseIngestRoute = (rest: readonly string[], query: (name: string) => string | null): IngestRoute => {
  const [first, second, ...more] = rest;
  if (first === undefined) return { kind: "board", stage: query("stage") ?? "", status: query("status") ?? "", text: query("q") ?? "" };
  if (first === "-") return second === "failed" && more.length === 0 ? { kind: "failed", state: query("state") === "cancelled" ? "cancelled" : "dead" } : { kind: "unknown" };
  return second === undefined ? { kind: "document", id: first } : { kind: "unknown" };
};

export const ingestPath = (vault: VaultId, route: IngestRoute): string => {
  const root = sectionPath(vault, "ingest");
  const query = (pairs: readonly (readonly [string, string])[]) => {
    const kept = pairs.filter(([, value]) => value !== "").map(([name, value]) => `${name}=${encodeURIComponent(value)}`);
    return kept.length === 0 ? "" : `?${kept.join("&")}`;
  };
  switch (route.kind) {
    case "board":
      return `${root}${query([["stage", route.stage], ["status", route.status], ["q", route.text]])}`;
    case "document":
      return `${root}/${encodeURIComponent(route.id)}`;
    case "failed":
      return `${root}/-/failed${route.state === "dead" ? "" : "?state=cancelled"}`;
    case "unknown":
      return root;
  }
};
