import type { ApiFailure, UploadLimits, UploadedSource, VaultId } from "@memex/types";
import type { ApiResult } from "./api";

/** What a browser's `File` has and everything here needs. The edge gets the same object back, and it alone can read it. */
export interface UploadFile {
  readonly name: string;
  readonly size: number;
  readonly type: string;
}

export interface UploadOptions {
  readonly trustTier: string;
  /** A recording's language, such as `sv`. */
  readonly language: string | null;
}

export interface RunningUpload {
  readonly result: Promise<ApiResult<UploadedSource>>;
  abort(): void;
}

export interface Uploader {
  start(vault: VaultId, file: UploadFile, options: UploadOptions, onProgress: (sent: number) => void): RunningUpload;
}

export type UploadItemState =
  /** Never sent, as it can't be accepted. */
  | { readonly kind: "refused"; readonly reason: string }
  | { readonly kind: "queued" }
  | { readonly kind: "uploading"; readonly sent: number }
  | { readonly kind: "stored"; readonly source: UploadedSource["source"] }
  /** These bytes were already in the vault, so nothing new was made. */
  | { readonly kind: "duplicate"; readonly source: UploadedSource["source"] }
  | { readonly kind: "failed"; readonly reason: string; readonly failure: ApiFailure }
  | { readonly kind: "cancelled" };

export interface UploadItem {
  readonly id: string;
  readonly name: string;
  readonly size: number;
  readonly options: UploadOptions;
  readonly state: UploadItemState;
}

export interface UploadQueueState {
  readonly items: readonly UploadItem[];
}

/** Browsers hold six connections per server, and the live stream and the page's reads need some of them. */
export const MAX_CONCURRENT_UPLOADS = 2;
const EXTENSIONS: Readonly<Record<string, string>> = {
  pdf: "application/pdf",
  txt: "text/plain",
  md: "text/markdown",
  markdown: "text/markdown",
  wav: "audio/wav",
  mp3: "audio/mpeg",
  m4a: "audio/mp4",
  m4b: "audio/mp4",
  ogg: "audio/ogg",
  opus: "audio/ogg",
  flac: "audio/flac",
  mp4: "video/mp4",
  webm: "video/webm",
  mkv: "video/x-matroska",
};
const RECORDING = /^(audio|video)\//;

const extensionOf = (name: string): string => name.slice(name.lastIndexOf(".") + 1).toLowerCase();

export const formatBytes = (bytes: number): string => {
  if (bytes >= 1024 ** 3) return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
  if (bytes >= 1024 ** 2) return `${(bytes / 1024 ** 2).toFixed(1)} MB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${String(bytes)} bytes`;
};

export const isRecordingFile = (file: UploadFile): boolean => RECORDING.test(file.type) || RECORDING.test(EXTENSIONS[extensionOf(file.name)] ?? "");

/**
 * Why a file can't be sent, or null. It decides only what's certain: the server reads the first bytes and has the last word, so a file this
 * can't place goes to the server, which accepts any text whatever it's called.
 */
export const uploadRefusal = (file: UploadFile, limits: UploadLimits): string | null => {
  if (file.size > limits.maxBytes) return `This is ${formatBytes(file.size)}, and the most this server takes is ${formatBytes(limits.maxBytes)}.`;
  if (file.size === 0) return "This file is empty.";
  if (limits.allowedMediaTypes.includes(file.type) || file.type === "" || file.type === "application/octet-stream" || file.type.startsWith("text/")) return null;
  const byName = EXTENSIONS[extensionOf(file.name)];
  if (byName !== undefined && limits.allowedMediaTypes.includes(byName)) return null;
  return `This server doesn't take ${file.type} files. It takes PDFs, text, Markdown and audio or video recordings.`;
};

const failureText = (failure: ApiFailure): string => {
  if (failure.kind === "unreachable") return "The connection was lost during the upload, so this file wasn't stored. Retry it when the connection is back.";
  if (failure.kind === "http" && failure.status === 413) return "This file is over the server's limit.";
  if (failure.kind === "http" && failure.status === 415) return "The server read the start of this file and doesn't take its type. It takes PDFs, text, Markdown and audio or video recordings.";
  if (failure.kind === "http" && failure.status === 403) return "Your token isn't allowed to upload to this vault.";
  if (failure.kind === "http" && failure.status === 429) return `The server has too much unfinished work to take more right now.${failure.retryAfterS === undefined ? "" : ` Try again in ${String(failure.retryAfterS)} s.`}`;
  return failure.message;
};

export interface UploadQueue {
  state(): UploadQueueState;
  subscribe(listener: () => void): () => void;
  add(files: readonly UploadFile[], options: UploadOptions): void;
  cancel(id: string): void;
  retry(id: string): void;
  remove(id: string): void;
  /** Aborts what's running, for when the session ends. Moving between screens doesn't stop an upload. */
  stop(): void;
}

interface QueueDeps {
  readonly vault: VaultId;
  readonly limits: UploadLimits;
  readonly uploader: Uploader;
  readonly nextId: () => string;
}

export const createUploadQueue = ({ vault, limits, uploader, nextId }: QueueDeps): UploadQueue => {
  let items: readonly UploadItem[] = [];
  let state: UploadQueueState = { items };
  const files = new Map<string, UploadFile>();
  const running = new Map<string, RunningUpload>();
  const listeners = new Set<() => void>();

  const commit = (next: readonly UploadItem[]) => {
    items = next;
    state = { items };
    for (const listener of [...listeners]) listener();
  };
  const update = (id: string, change: (item: UploadItem) => UploadItem) => commit(items.map((i) => (i.id === id ? change(i) : i)));

  const pump = () => {
    while (running.size < MAX_CONCURRENT_UPLOADS) {
      const next = items.find((i) => i.state.kind === "queued");
      const file = next === undefined ? undefined : files.get(next.id);
      if (next === undefined || file === undefined) return;
      const { id, options } = next;
      const upload = uploader.start(vault, file, options, (sent) => {
        if (running.has(id)) update(id, (i) => ({ ...i, state: { kind: "uploading", sent } }));
      });
      running.set(id, upload);
      update(id, (i) => ({ ...i, state: { kind: "uploading", sent: 0 } }));
      void upload.result.then((result) => {
        // Cancelled already, and whatever the request ended with is of no interest.
        if (!running.delete(id)) return;
        if (result.ok) {
          files.delete(id);
          update(id, (i) => ({ ...i, state: { kind: result.value.existing ? "duplicate" : "stored", source: result.value.source } }));
        } else {
          update(id, (i) => ({ ...i, state: { kind: "failed", reason: failureText(result.failure), failure: result.failure } }));
        }
        pump();
      });
    }
  };

  return {
    state: () => state,
    subscribe: (listener) => {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    add: (added, options) => {
      const fresh = added.map((file): UploadItem => {
        const id = nextId();
        const reason = uploadRefusal(file, limits);
        if (reason === null) files.set(id, file);
        return { id, name: file.name, size: file.size, options, state: reason === null ? { kind: "queued" } : { kind: "refused", reason } };
      });
      commit([...items, ...fresh]);
      pump();
    },
    cancel: (id) => {
      const upload = running.get(id);
      running.delete(id);
      upload?.abort();
      update(id, (i) => ({ ...i, state: { kind: "cancelled" } }));
      pump();
    },
    retry: (id) => {
      if (!files.has(id)) return;
      update(id, (i) => ({ ...i, state: { kind: "queued" } }));
      pump();
    },
    remove: (id) => {
      const upload = running.get(id);
      running.delete(id);
      upload?.abort();
      files.delete(id);
      commit(items.filter((i) => i.id !== id));
      pump();
    },
    stop: () => {
      for (const upload of running.values()) upload.abort();
      running.clear();
      listeners.clear();
    },
  };
};
