import type { EpochMs } from "./index";

/** Everything on these types comes from a vault, so all of the text is untrusted. */
export interface WikiPageSummary {
  readonly id: string;
  readonly title: string;
  readonly slug: string;
  /** Old slugs that still lead here. */
  readonly redirects: readonly string[];
  /** What an edit bases itself on, and also the newest revision. */
  readonly version: number;
  readonly creator: string;
  readonly updatedAt: EpochMs;
}

export interface WikiCitation {
  readonly evidenceId: string;
  readonly quote: string;
  /** `supports` or `refutes`. */
  readonly stance: string;
  readonly sourceId: string | null;
  readonly sourceName: string | null;
  readonly pageFrom: number | null;
  readonly pageTo: number | null;
  readonly trustTier: string | null;
  readonly anchorBroken: boolean;
}

export type WikiClaimStatus = "current" | "superseded" | "missing";

/** What a `{{claim:<id>}}` shows now. */
export interface WikiClaim {
  readonly claimId: string;
  readonly status: WikiClaimStatus;
  readonly currentClaimId: string | null;
  readonly subject: string | null;
  readonly predicate: string | null;
  readonly value: string | null;
  readonly epistemic: string | null;
  readonly citations: readonly WikiCitation[];
}

export interface WikiPage {
  readonly id: string;
  readonly currentRevision: number;
  readonly currentSlug: string;
  readonly redirects: readonly string[];
  /** The revision shown, which may be an old one. */
  readonly revision: number;
  readonly title: string;
  readonly slug: string;
  readonly body: string;
  readonly claims: readonly WikiClaim[];
  readonly creator: string;
  readonly updatedAt: EpochMs;
  readonly restoredFrom: number | null;
}

export interface WikiRevision {
  readonly revision: number;
  readonly title: string;
  readonly slug: string;
  readonly creator: string;
  readonly changesetId: string;
  readonly createdAt: EpochMs;
  readonly restoredFrom: number | null;
}

export interface WikiBlockChange {
  readonly blockId: string;
  /** `added`, `removed`, `changed` or `moved`. */
  readonly change: string;
  readonly before: string | null;
  readonly after: string | null;
}

export interface WikiDiff {
  readonly from: number;
  readonly to: number;
  readonly title: readonly string[] | null;
  readonly slug: readonly string[] | null;
  readonly blocks: readonly WikiBlockChange[];
}

export interface WikiBacklink {
  readonly edgeId: string;
  readonly fromPage: string;
  readonly fromTitle: string | null;
  readonly fromSlug: string | null;
  /** The slug as the link wrote it, which may be an old one. */
  readonly viaSlug: string;
}

export interface WikiOutgoingLink {
  readonly edgeId: string;
  readonly kind: "wiki_link" | "claim_ref";
  readonly target: string;
  readonly toPage: string | null;
  readonly resolved: boolean;
}

export interface WikiUnresolvedLink {
  readonly edgeId: string;
  readonly kind: "wiki_link" | "claim_ref";
  readonly fromPage: string;
  readonly target: string;
}

export interface WikiRedirect {
  readonly slug: string;
  readonly pageId: string;
  readonly currentSlug: string;
  readonly redirected: boolean;
}

/** What governance decided about a write. `pending` waits for review, so the page hasn't changed yet. */
export interface WikiWrite {
  readonly changesetId: string | null;
  readonly state: "committed" | "pending" | "rejected";
  readonly replayed: boolean;
  /** Why validators refused, when any did. */
  readonly refusals: readonly string[];
}

/** Who a change came from. */
export interface WikiAttribution {
  readonly proposer: string;
  readonly machine: boolean;
  readonly extractor: string | null;
  readonly rationale: string;
  readonly state: string;
}

/** Something a `{{claim:` can complete to. */
export interface WikiClaimOption {
  readonly id: string;
  readonly label: string;
}
