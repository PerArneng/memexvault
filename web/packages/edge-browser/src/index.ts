import { epochMs } from "@memex/types";
import type { EpochMs } from "@memex/types";
import type { Clock } from "@memex/core";

export const systemClock: Clock = {
  now: (): EpochMs => epochMs(Date.now()),
};
