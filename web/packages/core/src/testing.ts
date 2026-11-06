import { epochMs } from "@memex/types";
import type { EpochMs } from "@memex/types";
import type { Clock } from "./clock";

/** A clock tests move by hand, so nothing sleeps. */
export interface ManualClock extends Clock {
  advance(ms: number): void;
}

export const manualClock = (start = 0): ManualClock => {
  let current = start;
  return {
    now: (): EpochMs => epochMs(current),
    advance: (ms) => {
      current += ms;
    },
  };
};
