import { vaultId } from "@memex/types";
import type { Scope } from "@memex/types";
import { describe, expect, it } from "vitest";
import { accessTo, defaultSectionPath, homeVault, loginPath, parseVaultPath, safeNext, scopeIn, sectionById, sectionPath, SECTION_IDS, SECTIONS, switchVaultPath, visibleSections } from "./navigation";
import { identityWith, vaultNamed } from "./testing";

const a = vaultId("a");
const b = vaultId("b");
const withScope = (scope: Scope) => identityWith([{ vault: a, scope }]);
const ids = (scope: Scope) => visibleSections(withScope(scope), a).map((s) => s.id);
const admin = sectionById("admin");
if (admin === null) throw new Error("no admin section");

describe("scopeIn", () => {
  it("takes the highest grant for that vault only", () => {
    const identity = identityWith([{ vault: a, scope: "read" }, { vault: a, scope: "admin" }, { vault: b, scope: "propose" }]);
    expect(scopeIn(identity, a)).toBe("admin");
    expect(scopeIn(identity, b)).toBe("propose");
  });
  it("a grant elsewhere gives nothing here", () => {
    expect(scopeIn(identityWith([{ vault: b, scope: "admin" }]), a)).toBeNull();
  });
  it("a server admin has every scope, with no grants", () => {
    expect(scopeIn(identityWith([], true), a)).toBe("admin");
  });
});

describe("sections by scope", () => {
  it("lists all nine screens from the spec", () => {
    expect(SECTIONS.map((s) => s.label)).toEqual(["Ingest", "Documents", "Search", "Pages", "Entities", "Review", "Audit", "Chat", "Admin"]);
    expect(SECTIONS.map((s) => s.id)).toEqual([...SECTION_IDS]);
  });
  it("read sees everything but admin, including ingest with no upload controls (spec 041)", () => {
    expect(ids("read")).toEqual(["ingest", "documents", "search", "pages", "entities", "review", "audit", "chat"]);
  });
  it("propose still has no admin", () => {
    expect(ids("propose")).toContain("ingest");
    expect(ids("propose")).not.toContain("admin");
  });
  it("admin sees everything", () => {
    expect(ids("admin")).toEqual([...SECTION_IDS]);
  });
  it("no grant sees nothing", () => {
    expect(visibleSections(identityWith([]), a)).toEqual([]);
  });
});

describe("accessTo", () => {
  it("allows at or above the needed scope", () => {
    expect(accessTo(withScope("admin"), a, admin)).toEqual({ kind: "allowed" });
  });
  it("says what's missing for a lower scope", () => {
    expect(accessTo(withScope("read"), a, admin)).toEqual({ kind: "forbidden", have: "read", needs: "admin" });
  });
  it("doesn't say more than the server when there's no grant", () => {
    expect(accessTo(identityWith([]), a, admin)).toEqual({ kind: "no_vault_access" });
  });
});

describe("paths", () => {
  it("round-trips a vault, section and object", () => {
    expect(parseVaultPath(sectionPath(a, "documents", "d/1 x"))).toEqual({ vault: a, section: "documents", rest: ["d/1 x"] });
  });
  it("reads a vault with no section", () => {
    expect(parseVaultPath("/vaults/a")).toEqual({ vault: a, section: null, rest: [] });
  });
  it.each(["/", "/login", "/vaults", "/gallery/a"])("%s isn't inside a vault", (path) => {
    expect(parseVaultPath(path)).toBeNull();
  });
  it("a badly encoded path is null, not a crash", () => {
    expect(parseVaultPath("/vaults/%E0%A4%A")).toBeNull();
  });
  it("an unknown section name is no section", () => {
    expect(sectionById("nope")).toBeNull();
  });
});

describe("defaultSectionPath", () => {
  it("opens documents", () => {
    expect(defaultSectionPath(withScope("read"), a)).toBe("/vaults/a/documents");
  });
  it("with no access stays at the vault, whose screen explains it", () => {
    expect(defaultSectionPath(identityWith([]), a)).toBe("/vaults/a");
  });
});

describe("switchVaultPath", () => {
  const both = identityWith([{ vault: a, scope: "admin" }, { vault: b, scope: "read" }]);
  it("keeps the screen", () => {
    expect(switchVaultPath(both, "/vaults/a/search", b)).toBe("/vaults/b/search");
  });
  it("drops an object id, which belongs to the old vault", () => {
    expect(switchVaultPath(both, "/vaults/a/documents/d1", b)).toBe("/vaults/b/documents");
  });
  it("falls back when the token can't use that screen there", () => {
    expect(switchVaultPath(both, "/vaults/a/admin", b)).toBe("/vaults/b/documents");
  });
  it("falls back from an unknown screen, or from outside a vault", () => {
    expect(switchVaultPath(both, "/vaults/a/nope", b)).toBe("/vaults/b/documents");
    expect(switchVaultPath(both, "/", b)).toBe("/vaults/b/documents");
  });
});

describe("homeVault", () => {
  const vaults = [vaultNamed("a"), vaultNamed("b")];
  it("returns to the last vault", () => {
    expect(homeVault(vaults, "b")).toBe(b);
  });
  it("uses the first when the last one is gone or unset", () => {
    expect(homeVault(vaults, "deleted")).toBe(a);
    expect(homeVault(vaults, null)).toBe(a);
  });
  it("is null with no vaults", () => {
    expect(homeVault([], "a")).toBeNull();
  });
});

describe("safeNext", () => {
  it("keeps a plain path with its query", () => {
    expect(safeNext("/vaults/a/search?q=x")).toBe("/vaults/a/search?q=x");
  });
  it.each([
    ["another host", "https://evil.example/"],
    ["protocol-relative", "//evil.example/"],
    ["backslash", "/\\evil.example/"],
    ["tab", "/\t/evil.example/"],
    ["newline", "/\n/evil.example/"],
    ["script", "javascript:alert(1)"],
    ["no slash", "vaults/a"],
    ["empty", ""],
    ["the sign-in page", "/login"],
    ["the sign-in page with a query", "/login?next=/x"],
    ["not a string", 7],
    ["missing", undefined],
  ])("refuses %s", (_name, raw) => {
    expect(safeNext(raw)).toBe("/");
  });
});

describe("loginPath", () => {
  it("carries the current path", () => {
    expect(loginPath("/vaults/a/search?q=x")).toBe("/login?next=%2Fvaults%2Fa%2Fsearch%3Fq%3Dx");
  });
  it("adds nothing for the root or a refused path", () => {
    expect(loginPath("/")).toBe("/login");
    expect(loginPath("//evil.example")).toBe("/login");
  });
});
