import { epochMs } from "@memex/types";
import type { EpochMs, VaultId } from "@memex/types";
import type { Clock } from "./clock";

/**
 * Local storage for unsaved text. Unlike a preference, a lost write matters to the person typing, so `set` says whether it worked.
 */
export interface DraftStorage {
  get(key: string): string | null;
  /** False if it couldn't be kept, such as when storage is full or blocked. */
  set(key: string, value: string): boolean;
  remove(key: string): void;
}

export interface Draft {
  readonly body: string;
  readonly title: string | null;
  /** The version this was written against, or null for a page that doesn't exist yet. */
  readonly baseVersion: number | null;
  readonly savedAt: EpochMs;
}

/** Writes wait until typing pauses this long, and never longer than the second figure during steady typing. */
export const DRAFT_PAUSE_MS = 400;
export const DRAFT_MAX_WAIT_MS = 3000;
/** Forgotten drafts don't stay in a browser for ever. */
export const DRAFT_MAX_AGE_MS = 30 * 24 * 60 * 60 * 1000;

/** The person is in the key, so someone else signing in on this browser doesn't see it. `page` is an id, or `new`. */
export const draftKey = (principal: string, vault: VaultId, page: string): string => `memex.draft.${principal}.${vault}.${page}`;

export const encodeDraft = (draft: Draft): string => JSON.stringify(draft);

/** Storage can hold anything, so anything that isn't a draft reads as none. */
export const decodeDraft = (raw: string | null): Draft | null => {
  if (raw === null) return null;
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    return null;
  }
  if (typeof value !== "object" || value === null) return null;
  const { body, title, baseVersion, savedAt } = value as Readonly<Record<string, unknown>>;
  if (typeof body !== "string" || typeof savedAt !== "number" || !Number.isFinite(savedAt)) return null;
  if (title !== null && typeof title !== "string") return null;
  if (baseVersion !== null && (typeof baseVersion !== "number" || !Number.isInteger(baseVersion) || baseVersion < 0)) return null;
  return { body, title, baseVersion, savedAt: epochMs(savedAt) };
};

export type DraftStanding =
  /** Written against the version the page is at. */
  | { readonly kind: "current" }
  /** The page has moved on since, so saving will show the conflict view. */
  | { readonly kind: "behind"; readonly baseVersion: number; readonly pageVersion: number };

export const draftStanding = (draft: Draft, pageVersion: number): DraftStanding =>
  draft.baseVersion !== null && draft.baseVersion < pageVersion ? { kind: "behind", baseVersion: draft.baseVersion, pageVersion } : { kind: "current" };

export interface DraftChange {
  readonly body: string;
  readonly title: string | null;
  readonly baseVersion: number | null;
  /** What's already on the server, since text equal to it isn't a draft. */
  readonly pristineBody: string;
  readonly pristineTitle: string | null;
}

export interface DraftKeeper {
  /** The draft to open with. One past its age is removed and doesn't come back. */
  load(): Draft | null;
  edit(change: DraftChange): void;
  /** Writes if typing has paused or waited long enough. A view calls this on a timer. */
  flushIfDue(): void;
  /** Writes now, for when the page is going away. */
  flush(): void;
  /** After a save, or when the person throws the draft away. Nothing written before this can come back. */
  discard(): void;
  /** Whether the last write failed, which the editor has to say, as a reload would lose the text. */
  writeFailed(): boolean;
}

export const createDraftKeeper = ({ clock, storage, key }: { readonly clock: Clock; readonly storage: DraftStorage; readonly key: string }): DraftKeeper => {
  let pending: DraftChange | null = null;
  let firstEditAt = 0;
  let lastEditAt = 0;
  let failed = false;

  const write = () => {
    if (pending === null) return;
    const { body, title, baseVersion, pristineBody, pristineTitle } = pending;
    pending = null;
    if (body === pristineBody && title === pristineTitle) {
      storage.remove(key);
      failed = false;
      return;
    }
    failed = !storage.set(key, encodeDraft({ body, title, baseVersion, savedAt: clock.now() }));
  };

  return {
    load: () => {
      const draft = decodeDraft(storage.get(key));
      if (draft === null) return null;
      if (clock.now() - draft.savedAt > DRAFT_MAX_AGE_MS) {
        storage.remove(key);
        return null;
      }
      return draft;
    },
    edit: (change) => {
      if (pending === null) firstEditAt = clock.now();
      lastEditAt = clock.now();
      pending = change;
    },
    flushIfDue: () => {
      if (pending === null) return;
      const now = clock.now();
      if (now - lastEditAt >= DRAFT_PAUSE_MS || now - firstEditAt >= DRAFT_MAX_WAIT_MS) write();
    },
    flush: write,
    discard: () => {
      pending = null;
      failed = false;
      storage.remove(key);
    },
    writeFailed: () => failed,
  };
};
