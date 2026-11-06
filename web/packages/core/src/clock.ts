import type { EpochMs } from "@memex/types";

export interface Clock {
  now(): EpochMs;
}
