import type { EpochMs } from "@memex/types";
import type { Clock } from "./clock";
import type { Settings } from "./settings";

export interface Edges {
  readonly clock: Clock;
  readonly settings: Settings;
}

/** The single entry point for views. Later specs add a method per use case. */
export interface Engine {
  startedAt(): EpochMs;
  uptimeMs(): number;
  settings(): Settings;
}

export const createEngine = (edges: Edges): Engine => {
  const started = edges.clock.now();
  return {
    startedAt: () => started,
    uptimeMs: () => edges.clock.now() - started,
    settings: () => edges.settings,
  };
};
