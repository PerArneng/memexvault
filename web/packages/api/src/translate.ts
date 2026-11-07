import { epochMs, vaultId } from "@memex/types";
import type { ApiFailure, Identity, ObjectSummary, Page, Vault } from "@memex/types";
import type { components } from "./generated/schema";

type Schemas = components["schemas"];

export const vaultFromWire = (v: Schemas["VaultDto"]): Vault => ({ id: vaultId(v.id), name: v.name, createdAt: epochMs(v.created_at_ms) });

export const vaultPageFromWire = (p: Schemas["VaultPage"]): Page<Vault> => ({ items: p.items.map(vaultFromWire), nextCursor: p.next_cursor ?? null });

export const identityFromWire = (w: Schemas["WhoAmI"]): Identity => ({
  principal: { id: w.principal.id, name: w.principal.name, serverAdmin: w.principal.server_admin },
  tokenId: w.token_id,
  grants: w.grants.map((g) => ({ vault: vaultId(g.vault_id), scope: g.scope })),
});

export interface ObjectPages {
  readonly sources: Schemas["SourcePage"];
  readonly pages: Schemas["PageSummaryPage"];
  readonly entityTypes: Schemas["EntityTypePage"];
}

export const objectsFromWire = ({ sources, pages, entityTypes }: ObjectPages): readonly ObjectSummary[] => [
  ...sources.items.map((s) => ({ kind: "document", id: s.id, label: s.name }) as const),
  ...pages.items.map((p) => ({ kind: "page", id: p.id, label: p.title }) as const),
  ...entityTypes.items.map((t) => ({ kind: "entity_type", id: t.name, label: t.name }) as const),
];

export interface WireHeaders {
  get(name: string): string | null;
}

const isErrorBody = (body: unknown): body is Schemas["ErrorBody"] => {
  if (typeof body !== "object" || body === null || !("error" in body)) return false;
  const { error } = body;
  return typeof error === "object" && error !== null && "code" in error && "message" in error && "request_id" in error;
};

const retryAfter = (headers: WireHeaders): number | undefined => {
  const raw = headers.get("retry-after");
  const seconds = raw === null ? Number.NaN : Number(raw);
  return Number.isInteger(seconds) && seconds >= 0 ? seconds : undefined;
};

/** Anything that isn't the server's error format, such as a proxy's page, stays `protocol` rather than being trusted as ours. */
export const failureFromWire = (status: number, body: unknown, headers: WireHeaders): ApiFailure => {
  if (!isErrorBody(body)) return { kind: "protocol", status, message: `The server answered ${String(status)} with something unexpected.` };
  const seconds = retryAfter(headers);
  const { code, message, request_id: requestId } = body.error;
  return { kind: "http", status, code, message, requestId, ...(seconds === undefined ? {} : { retryAfterS: seconds }) };
};
