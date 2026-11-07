import { SCOPES, vaultId } from "@memex/types";
import type { Identity, Scope, Vault, VaultId } from "@memex/types";

export const SECTION_IDS = ["ingest", "documents", "search", "pages", "entities", "review", "audit", "chat", "admin"] as const;
export type SectionId = (typeof SECTION_IDS)[number];

export interface Section {
  readonly id: SectionId;
  readonly label: string;
  /** The least scope that can use the screen, matching what the API allows. */
  readonly needs: Scope;
}

/**
 * Reading the review queue needs only `read`, and approving needs `admin`, which the screen itself will enforce. Chat is open to
 * `read`, since spec 049 lets a read token chat and refuses only its proposals.
 */
export const SECTIONS: readonly Section[] = [
  { id: "ingest", label: "Ingest", needs: "propose" },
  { id: "documents", label: "Documents", needs: "read" },
  { id: "search", label: "Search", needs: "read" },
  { id: "pages", label: "Pages", needs: "read" },
  { id: "entities", label: "Entities", needs: "read" },
  { id: "review", label: "Review", needs: "read" },
  { id: "audit", label: "Audit", needs: "read" },
  { id: "chat", label: "Chat", needs: "read" },
  { id: "admin", label: "Admin", needs: "admin" },
];

const DEFAULT_SECTION: SectionId = "documents";

export const sectionById = (id: string): Section | null => SECTIONS.find((s) => s.id === id) ?? null;

/** The same rule as the server's `vault_needs`: a server admin has every scope, and otherwise the highest grant counts. */
export const scopeIn = (identity: Identity, vault: VaultId): Scope | null => {
  if (identity.principal.serverAdmin) return "admin";
  let best: Scope | null = null;
  for (const grant of identity.grants) {
    if (grant.vault === vault && (best === null || SCOPES.indexOf(grant.scope) > SCOPES.indexOf(best))) best = grant.scope;
  }
  return best;
};

const includes = (have: Scope, need: Scope): boolean => SCOPES.indexOf(have) >= SCOPES.indexOf(need);

export type Access =
  | { readonly kind: "allowed" }
  | { readonly kind: "forbidden"; readonly have: Scope; readonly needs: Scope }
  /** No grant at all. Like the server, this doesn't say whether the vault exists. */
  | { readonly kind: "no_vault_access" };

export const accessTo = (identity: Identity, vault: VaultId, section: Section): Access => {
  const have = scopeIn(identity, vault);
  if (have === null) return { kind: "no_vault_access" };
  return includes(have, section.needs) ? { kind: "allowed" } : { kind: "forbidden", have, needs: section.needs };
};

export const visibleSections = (identity: Identity, vault: VaultId): readonly Section[] =>
  SECTIONS.filter((section) => accessTo(identity, vault, section).kind === "allowed");

export const vaultPath = (vault: VaultId): string => `/vaults/${encodeURIComponent(vault)}`;

export const sectionPath = (vault: VaultId, section: SectionId, objectId?: string): string =>
  `${vaultPath(vault)}/${section}${objectId === undefined ? "" : `/${encodeURIComponent(objectId)}`}`;

export interface VaultLocation {
  readonly vault: VaultId;
  /** Null at `/vaults/<id>`. May name no real section, as it comes from a URL. */
  readonly section: string | null;
  readonly rest: readonly string[];
}

/** Reads a URL path. Null when it isn't inside a vault or is badly encoded. */
export const parseVaultPath = (pathname: string): VaultLocation | null => {
  const parts = pathname.split("/").filter((p) => p !== "");
  const [root, vault, section, ...rest] = parts;
  if (root !== "vaults" || vault === undefined) return null;
  try {
    return { vault: vaultId(decodeURIComponent(vault)), section: section === undefined ? null : decodeURIComponent(section), rest: rest.map(decodeURIComponent) };
  } catch {
    return null;
  }
};

/** Where to land in a vault with no section named: the usual section, or else the first this token can use. */
export const defaultSectionPath = (identity: Identity, vault: VaultId): string => {
  const visible = visibleSections(identity, vault);
  const chosen = visible.find((s) => s.id === DEFAULT_SECTION) ?? visible[0];
  return chosen === undefined ? vaultPath(vault) : sectionPath(vault, chosen.id);
};

/**
 * Switching keeps the screen if the token may use it there. Object ids and the query are dropped, as they belong to the old vault.
 */
export const switchVaultPath = (identity: Identity, pathname: string, target: VaultId): string => {
  const section = sectionById(parseVaultPath(pathname)?.section ?? "");
  if (section !== null && accessTo(identity, target, section).kind === "allowed") return sectionPath(target, section.id);
  return defaultSectionPath(identity, target);
};

/** The vault for `/`: the last one used if it's still listed, or else the first. Null when there are none. */
export const homeVault = (vaults: readonly Vault[], last: string | null): VaultId | null =>
  (vaults.find((v) => v.id === last) ?? vaults[0])?.id ?? null;

/**
 * Where to go after signing in. The value comes from a URL anyone can link to, so only a plain same-origin path passes:
 * no scheme, no `//host`, no backslash tricks, and never back to the sign-in page.
 */
export const safeNext = (raw: unknown): string => {
  const fallback = "/";
  if (typeof raw !== "string" || !raw.startsWith("/") || raw.startsWith("//")) return fallback;
  // Browsers treat these as slashes or strip them when parsing a URL, so they could turn a path into another host.
  if (/[\\\t\r\n]/.test(raw)) return fallback;
  if (raw === "/login" || raw.startsWith("/login?") || raw.startsWith("/login/")) return fallback;
  return raw;
};

/** The sign-in URL that comes back here afterwards. */
export const loginPath = (current: string): string => {
  const next = safeNext(current);
  return next === "/" ? "/login" : `/login?next=${encodeURIComponent(next)}`;
};
