import { describe, expect, it } from "vitest";
import { failureFromWire, identityFromWire, objectsFromWire, vaultPageFromWire } from "./translate";

const noHeaders = { get: () => null };
const principal = { id: "p", name: "pat", kind: "human", server_admin: true, disabled: false, created_by: "x", created_at_ms: 1 } as const;
const errorBody = { error: { code: "forbidden", message: "no", request_id: "r1" } };

describe("wire to domain", () => {
  it("translates who am I", () => {
    expect(identityFromWire({ principal, token_id: "t", grants: [{ vault_id: "v", scope: "read" }] })).toEqual({
      principal: { id: "p", name: "pat", serverAdmin: true, machine: false },
      tokenId: "t",
      grants: [{ vault: "v", scope: "read" }],
    });
  });
  it("treats a missing cursor as the end", () => {
    const page = vaultPageFromWire({ items: [{ id: "v", name: "V", created_by: "x", created_at_ms: 5 }] });
    expect(page).toEqual({ items: [{ id: "v", name: "V", createdAt: 5 }], nextCursor: null });
  });
  it("gives every object its kind", () => {
    const objects = objectsFromWire({
      sources: { items: [{ id: "s", name: "a.pdf" } as never] },
      pages: { items: [{ id: "g", title: "T" } as never] },
      entityTypes: { items: [{ name: "person" } as never] },
    });
    expect(objects.map((o) => [o.kind, o.id, o.label])).toEqual([["document", "s", "a.pdf"], ["page", "g", "T"], ["entity_type", "person", "person"]]);
  });
});

describe("failureFromWire", () => {
  it("reads the server's error format with its request id", () => {
    expect(failureFromWire(403, errorBody, noHeaders)).toEqual({ kind: "http", status: 403, code: "forbidden", message: "no", requestId: "r1" });
  });
  it("reads Retry-After", () => {
    const headers = { get: (n: string) => (n === "retry-after" ? "60" : null) };
    expect(failureFromWire(429, errorBody, headers)).toMatchObject({ retryAfterS: 60 });
  });
  it.each(["soon", "-1", "1.5"])("ignores a Retry-After of %s", (value) => {
    expect(failureFromWire(429, errorBody, { get: () => value })).not.toHaveProperty("retryAfterS");
  });
  it.each([["a proxy page", "<html>"], ["nothing", undefined], ["another shape", { status: "unavailable" }], ["half an error", { error: { code: "x" } }], ["null", null]])("calls %s protocol", (_name, body) => {
    expect(failureFromWire(502, body, noHeaders)).toMatchObject({ kind: "protocol", status: 502 });
  });
});
