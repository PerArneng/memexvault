import { describe, expect, it } from "vitest";
import { DEFAULT_SETTINGS, settingsFrom } from "./settings";

const from = (vars: Record<string, string>) => settingsFrom((name) => vars[name]);

describe("settingsFrom", () => {
  it("uses defaults when nothing is set", () => {
    expect(from({})).toEqual({ ok: true, settings: DEFAULT_SETTINGS });
  });
  it("reads valid values", () => {
    expect(from({ MEMEX_API_BASE_URL: "/x", MEMEX_ACTIVITY_BUFFER_EVENTS: "10" })).toEqual({
      ok: true,
      settings: { apiBaseUrl: "/x", activityBufferEvents: 10 },
    });
  });
  it.each(["0", "-1", "1.5", "many", ""])("refuses buffer size %j", (value) => {
    expect(from({ MEMEX_ACTIVITY_BUFFER_EVENTS: value }).ok).toBe(false);
  });
  it("refuses an API on another origin", () => {
    expect(from({ MEMEX_API_BASE_URL: "https://elsewhere.example/api" }).ok).toBe(false);
  });
});
