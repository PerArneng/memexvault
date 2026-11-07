import { epochMs } from "@memex/types";
import type { EpochMs } from "@memex/types";
import type { Clock, Preferences } from "@memex/core";

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
