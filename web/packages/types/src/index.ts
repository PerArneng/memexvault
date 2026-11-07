/** Epoch milliseconds, so a raw number can't be mistaken for a duration. */
export type EpochMs = number & { readonly __brand: "EpochMs" };

export const epochMs = (value: number): EpochMs => value as EpochMs;

export type VaultId = string & { readonly __brand: "VaultId" };

export const vaultId = (value: string): VaultId => value as VaultId;

/** Ordered: each scope includes the ones before it. */
export const SCOPES = ["read", "propose", "admin"] as const;
export type Scope = (typeof SCOPES)[number];

export interface Grant {
  readonly vault: VaultId;
  readonly scope: Scope;
}

export interface Principal {
  readonly id: string;
  readonly name: string;
  readonly serverAdmin: boolean;
}

/** Who the current credential is, from `whoami`. */
export interface Identity {
  readonly principal: Principal;
  readonly tokenId: string;
  readonly grants: readonly Grant[];
}

export interface Vault {
  readonly id: VaultId;
  readonly name: string;
  readonly createdAt: EpochMs;
}

export interface Page<T> {
  readonly items: readonly T[];
  readonly nextCursor: string | null;
}

/** Something the command palette can open. The label is vault content, so it's untrusted. */
export interface ObjectSummary {
  readonly kind: "document" | "page" | "entity_type";
  readonly id: string;
  readonly label: string;
}

/** Why a call failed, as plain data. `http` is the server's own error format, which carries a request id. */
export type ApiFailure =
  | {
      readonly kind: "http";
      readonly status: number;
      readonly code: string;
      readonly message: string;
      readonly requestId: string;
      readonly retryAfterS?: number;
    }
  /** No answer at all: refused, timed out or the network is down. */
  | { readonly kind: "unreachable"; readonly message: string }
  /** An answer that isn't our API, such as a proxy's error page. */
  | { readonly kind: "protocol"; readonly status: number; readonly message: string };
