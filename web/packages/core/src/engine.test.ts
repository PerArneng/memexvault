import { describe, expect, it } from "vitest";
import { createEngine } from "./engine";
import { DEFAULT_SETTINGS } from "./settings";
import { manualClock } from "./testing";

describe("createEngine", () => {
  it("measures uptime from the injected clock", () => {
    const clock = manualClock(1000);
    const engine = createEngine({ clock, settings: DEFAULT_SETTINGS });
    expect(engine.uptimeMs()).toBe(0);
    clock.advance(250);
    expect(engine.uptimeMs()).toBe(250);
    expect(engine.startedAt()).toBe(1000);
  });
});
