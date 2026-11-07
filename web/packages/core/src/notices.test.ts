import { describe, expect, it } from "vitest";
import { noticeFor } from "./notices";
import { httpFailure, UNREACHABLE } from "./testing";

describe("noticeFor", () => {
  it("names the request id", () => {
    expect(noticeFor(httpFailure(500, "internal", "the request failed"), "read")).toEqual({ title: "the request failed", detail: "request id: req-1" });
  });
  it("leaves no answer to the banner, and a 401 to the sign-in page", () => {
    expect(noticeFor(UNREACHABLE, "action")).toBeNull();
    expect(noticeFor(httpFailure(401, "unauthenticated"), "action")).toBeNull();
  });
  it("a read shows 403 and 404 on the screen, and an action has only the toast", () => {
    expect(noticeFor(httpFailure(403, "forbidden"), "read")).toBeNull();
    expect(noticeFor(httpFailure(404, "not_found"), "read")).toBeNull();
    expect(noticeFor(httpFailure(403, "forbidden"), "action")).not.toBeNull();
  });
  it("an answer that isn't our API still says something, with no request id to give", () => {
    expect(noticeFor({ kind: "protocol", status: 502, message: "bad gateway" }, "read")).toEqual({ title: "bad gateway" });
  });
});
