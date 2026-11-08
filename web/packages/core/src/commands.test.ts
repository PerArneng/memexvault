import { vaultId } from "@memex/types";
import type { ObjectSummary } from "@memex/types";
import { describe, expect, it } from "vitest";
import { paletteEntries } from "./commands";
import { identityWith, vaultNamed } from "./testing";

const a = vaultId("a");
const objects: readonly ObjectSummary[] = [
  { kind: "document", id: "s1", label: "report.pdf" },
  { kind: "page", id: "s1", label: "Overview" },
  { kind: "entity_type", id: "person", label: "person" },
];
const input = { pathname: "/vaults/a/search", vault: a, vaults: [vaultNamed("a"), vaultNamed("b", "Beta")], objects };

describe("paletteEntries", () => {
  it("offers screens, objects, other vaults and sign out", () => {
    const entries = paletteEntries({ ...input, identity: identityWith([{ vault: a, scope: "admin" }, { vault: vaultId("b"), scope: "read" }]) });
    const labels = entries.map((e) => e.label);
    expect(labels).toContain("Go to Admin");
    expect(labels).toContain("report.pdf");
    expect(labels).toContain("Switch to Beta");
    expect(labels).not.toContain("Switch to a");
    expect(labels.at(-1)).toBe("Sign out");
  });
  it("routes each object to its own screen, and a vault switch keeps the screen", () => {
    const entries = paletteEntries({ ...input, identity: identityWith([{ vault: a, scope: "read" }, { vault: vaultId("b"), scope: "read" }]) });
    const to = (label: string) => entries.find((e) => e.label === label)?.action;
    expect(to("report.pdf")).toEqual({ kind: "go", to: "/vaults/a/documents/s1" });
    expect(to("Overview")).toEqual({ kind: "go", to: "/vaults/a/pages/s1" });
    expect(to("person")).toEqual({ kind: "go", to: "/vaults/a/entities/person" });
    expect(to("Switch to Beta")).toEqual({ kind: "go", to: "/vaults/b/search" });
  });
  it("a read token is offered no admin screen", () => {
    const entries = paletteEntries({ ...input, identity: identityWith([{ vault: a, scope: "read" }]) });
    expect(entries.map((e) => e.label)).not.toContain("Go to Admin");
    expect(entries.map((e) => e.label)).toContain("Go to Ingest");
  });
  it("ids stay unique when two kinds share an id", () => {
    const entries = paletteEntries({ ...input, identity: identityWith([], true) });
    expect(new Set(entries.map((e) => e.id)).size).toBe(entries.length);
  });
  it("outside a vault, or with no access to it, offers no screens or objects", () => {
    const outside = paletteEntries({ ...input, vault: null, identity: identityWith([], true) });
    expect(outside.map((e) => e.hint)).toEqual(["vault", "vault", "session"]);
    const denied = paletteEntries({ ...input, identity: identityWith([]) });
    expect(denied.some((e) => e.hint === "screen" || e.hint === "document")).toBe(false);
  });
});
