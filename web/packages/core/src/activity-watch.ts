import type { ActivityFrame, ActivityPage, ApiFailure, DurableFrame, VaultId } from "@memex/types";
import { activityReducer, INITIAL_ACTIVITY, MAX_BOARD_DOCUMENTS } from "./activity";
import type { ActivityEvent, ActivityState } from "./activity";
import type { ApiResult } from "./api";
import type { Clock } from "./clock";

export type FeedEvent =
  | { readonly type: "frame"; readonly frame: ActivityFrame }
  /** The connection went, and the feed is trying again by itself. */
  | { readonly type: "lost" }
  /** It won't try again: the server refused, or ended the stream for good. */
  | { readonly type: "ended"; readonly reason: string; readonly failure: ApiFailure | null };

/** One open stream from a position. It resumes by itself after a lost connection, from the last position it saw. */
export interface ActivityFeed {
  follow(vault: VaultId, after: number, listener: (event: FeedEvent) => void): () => void;
}

/** A pending list longer than this means the snapshot took far too long, so the next one starts over. */
export const MAX_PENDING_FRAMES = 4096;
/** Snapshots are never closer than this, and never take more than a fifth of the time, whatever the size of the vault. */
export const MIN_SNAPSHOT_GAP_MS = 1000;
const SNAPSHOT_COST_FACTOR = 5;
const SNAPSHOT_RETRY_MS = 5000;
const MAX_PAGES = Math.ceil(MAX_BOARD_DOCUMENTS / 200);

export interface ActivityWatch {
  state(): ActivityState;
  /** The last snapshot's failure, while there's nothing better to show. Null once one has worked. */
  failure(): ApiFailure | null;
  now(): number;
  /** Called on any change, and at most once per `tick`. */
  subscribe(listener: () => void): () => void;
  /** Applies what has arrived, in one batch. The view calls this on a timer, which is what batches rendering. */
  tick(): void;
  /** A snapshot soon, as an action of this user's changed something. */
  refresh(): void;
  stop(): void;
}

interface WatchDeps {
  readonly vault: VaultId;
  readonly clock: Clock;
  readonly feed: ActivityFeed;
  readonly page: (vault: VaultId, cursor: string | null) => Promise<ApiResult<ActivityPage>>;
}

/** Holds no timer of its own, so tests move the clock and call `tick`, and nothing sleeps. */
export const createActivityWatch = ({ vault, clock, feed, page }: WatchDeps): ActivityWatch => {
  let state: ActivityState = INITIAL_ACTIVITY;
  let failure: ApiFailure | null = null;
  let stopped = false;
  let incoming: ActivityEvent[] = [];
  /** Frames that arrive while a snapshot is on its way, which it may not have seen. Null when none is. */
  let pending: DurableFrame[] | null = null;
  let overflowed = false;
  let lastSnapshotAt = Number.NEGATIVE_INFINITY;
  let lastCost = 0;
  let nextAttemptAt = 0;
  let wanted = false;
  let unfollow: (() => void) | null = null;
  const listeners = new Set<() => void>();

  const notify = () => {
    for (const listener of [...listeners]) listener();
  };

  const drain = () => {
    if (incoming.length === 0) return false;
    const batch = incoming;
    incoming = [];
    const next = activityReducer(state, batch);
    const changed = next !== state;
    state = next;
    return changed;
  };

  const onFeed = (event: FeedEvent) => {
    const at = clock.now();
    switch (event.type) {
      case "frame":
        if (event.frame.kind === "durable" && pending !== null) {
          if (pending.length < MAX_PENDING_FRAMES) pending.push(event.frame);
          else overflowed = true;
        }
        incoming.push({ type: "frame", frame: event.frame, at });
        break;
      case "lost":
        incoming.push({ type: "lost", at });
        break;
      case "ended":
        incoming.push({ type: "ended", reason: event.reason });
        break;
    }
  };

  const fetchAll = async (): Promise<ApiResult<{ head: number; items: ActivityPage["items"]; truncated: boolean }>> => {
    const items: ActivityPage["items"][number][] = [];
    let cursor: string | null = null;
    let head: number | null = null;
    for (let n = 0; n < MAX_PAGES; n += 1) {
      const result: ApiResult<ActivityPage> = await page(vault, cursor);
      if (!result.ok) return result;
      // The first page's position is the oldest, so everything after it gets replayed. Frames say what's so, not what changed, so going over one twice is harmless.
      head ??= result.value.head;
      items.push(...result.value.items);
      cursor = result.value.nextCursor;
      if (cursor === null) return { ok: true, value: { head, items, truncated: false } };
    }
    return { ok: true, value: { head: head ?? -1, items, truncated: true } };
  };

  const snapshot = async () => {
    const startedAt = clock.now();
    pending = [];
    overflowed = false;
    wanted = false;
    const result = await fetchAll();
    if (stopped) return;
    const replay = pending;
    pending = null;
    const at = clock.now();
    if (!result.ok) {
      failure = result.failure;
      nextAttemptAt = at + SNAPSHOT_RETRY_MS;
      wanted = true;
      notify();
      return;
    }
    failure = null;
    lastSnapshotAt = at;
    lastCost = at - startedAt;
    incoming.push({ type: "snapshot", head: result.value.head, documents: result.value.items, truncated: result.value.truncated, replay, at });
    if (overflowed) wanted = true;
    drain();
    // The stream starts from the first snapshot's position, so nothing falls between them.
    if (unfollow === null) unfollow = feed.follow(vault, result.value.head, onFeed);
    notify();
  };

  let snapshotting: Promise<void> | null = null;
  const startSnapshot = () => {
    snapshotting = snapshot().finally(() => {
      snapshotting = null;
    });
  };
  startSnapshot();

  return {
    state: () => state,
    failure: () => failure,
    now: () => clock.now(),
    subscribe: (listener) => {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    tick: () => {
      if (stopped) return;
      if (drain()) notify();
      if (snapshotting !== null) return;
      const now = clock.now();
      if (now < nextAttemptAt) return;
      // `unfollow === null` means the first snapshot failed, and it's still what everything waits for.
      const due = state.needsSnapshot || state.dirty || wanted || unfollow === null;
      if (due && now - lastSnapshotAt >= Math.max(MIN_SNAPSHOT_GAP_MS, lastCost * SNAPSHOT_COST_FACTOR)) startSnapshot();
    },
    refresh: () => {
      wanted = true;
    },
    stop: () => {
      stopped = true;
      unfollow?.();
      listeners.clear();
    },
  };
};
