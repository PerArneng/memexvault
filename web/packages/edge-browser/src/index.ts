import { epochMs } from "@memex/types";
import type { EpochMs } from "@memex/types";
import type { Clock, DraftStorage, IdSource, Preferences } from "@memex/core";

export const systemClock: Clock = {
  now: (): EpochMs => epochMs(Date.now()),
};

/** Storage can be blocked or full, and losing a preference breaks nothing, so failures are ignored. */
export const browserPreferences: Preferences = {
  get: (key) => {
    try {
      return window.localStorage.getItem(key);
    } catch {
      return null;
    }
  },
  set: (key, value) => {
    try {
      window.localStorage.setItem(key, value);
    } catch {
      // Nothing to do. The next visit opens the first vault.
    }
  },
};

/** Unlike a preference, a draft that can't be kept has to be reported, as a reload would lose the text. */
export const browserDraftStorage: DraftStorage = {
  get: (key) => {
    try {
      return window.localStorage.getItem(key);
    } catch {
      return null;
    }
  },
  set: (key, value) => {
    try {
      window.localStorage.setItem(key, value);
      return true;
    } catch {
      return false;
    }
  },
  remove: (key) => {
    try {
      window.localStorage.removeItem(key);
    } catch {
      // Nothing was kept, or it can't be reached. Either way the next read decides.
    }
  },
};

/** `crypto.randomUUID` needs a secure context, and a server on a private network over plain HTTP isn't one. */
export const browserIds: IdSource = {
  next: () => Array.from(crypto.getRandomValues(new Uint8Array(16)), (b) => b.toString(16).padStart(2, "0")).join(""),
};
