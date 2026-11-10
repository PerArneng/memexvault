import type { DocumentSource } from "./documents";

/** How far a worker has got. In memory on the server, so it can be missing or late. */
export interface DocumentProgress {
  readonly stage: string;
  readonly done: number;
  readonly total: number | null;
  readonly timeMs: number;
}

/**
 * One document on the live board. Stage and status stay plain strings, as a newer server may name one this console doesn't know,
 * and it still has to show.
 */
export interface DocumentActivity {
  readonly sourceId: string;
  readonly logicalId: string;
  readonly versionNo: number;
  readonly recordedAtMs: number;
  readonly stage: string;
  readonly status: string;
  readonly partial: boolean;
  readonly attempts: number;
  readonly jobId: string | null;
  readonly nextRunAtMs: number | null;
  readonly waitingSinceMs: number | null;
  readonly pageCount: number | null;
  readonly segmentCount: number | null;
  readonly segmentsExtracted: number | null;
  readonly pendingReview: number;
  readonly renditionId: string | null;
  readonly progress: DocumentProgress | null;
}

export interface ActivityPage {
  /** The log position this reflects. -1 is before the first event. */
  readonly head: number;
  readonly items: readonly DocumentActivity[];
  readonly nextCursor: string | null;
}

export interface SourceFact {
  readonly id: string;
  readonly logicalId: string | null;
  readonly versionNo: number;
}

export interface JobFact {
  readonly id: string;
  readonly kind: string;
  readonly state: string;
  readonly stage: string | null;
  readonly status: string;
  readonly attempts: number;
  readonly nextRunAtMs: number;
  readonly waitingSinceMs: number | null;
  readonly sourceId: string | null;
}

export interface RenditionFact {
  readonly id: string;
  readonly sourceId: string | null;
  readonly pageCount: number;
  readonly segmentCount: number;
  readonly superseded: boolean;
}

export interface ExtractionFact {
  readonly sourceId: string | null;
  readonly segmentsDone: number;
  readonly batchesFailed: number;
  readonly stop: string | null;
}

export interface DurableFrame {
  readonly kind: "durable";
  readonly seq: number;
  readonly timeMs: number;
  readonly eventKind: string;
  readonly sources: readonly SourceFact[];
  readonly jobs: readonly JobFact[];
  readonly renditions: readonly RenditionFact[];
  readonly extractions: readonly ExtractionFact[];
  /** It wrote something that changes a document's stage, and that a frame can't say enough about to place. */
  readonly unplaced: boolean;
}

export interface ProgressFrame {
  readonly kind: "progress";
  readonly jobId: string;
  readonly sourceId: string | null;
  readonly stage: string;
  readonly done: number;
  readonly total: number | null;
  readonly timeMs: number;
}

export type ActivityFrame =
  | { readonly kind: "hello"; readonly head: number; readonly resumed: boolean }
  | DurableFrame
  | ProgressFrame
  | { readonly kind: "resync"; readonly head: number; readonly reason: string }
  | { readonly kind: "heartbeat"; readonly head: number }
  | { readonly kind: "end"; readonly reason: string }
  /** A frame from a newer server, or one that can't be read. */
  | { readonly kind: "unknown"; readonly name: string };

export interface JobAttempt {
  readonly startedAtMs: number;
  readonly finishedAtMs: number | null;
  /** Can quote a document, so it's untrusted text. */
  readonly error: string | null;
}

export interface Job {
  readonly id: string;
  readonly kind: string;
  /** `queued`, `running`, `done`, `dead` or `cancelled`. */
  readonly state: string;
  readonly sourceId: string | null;
  readonly maxAttempts: number;
  readonly nextRunAtMs: number;
  readonly attempts: readonly JobAttempt[];
  readonly createdAtMs: number;
  readonly finishedAtMs: number | null;
  readonly waitingSinceMs: number | null;
}

export interface UploadLimits {
  readonly maxBytes: number;
  readonly allowedMediaTypes: readonly string[];
}

export interface UploadedSource {
  readonly source: DocumentSource;
  /** These bytes were already here, so nothing new was created. */
  readonly existing: boolean;
  readonly jobId: string | null;
}
