import { TRUST_TIERS } from "@memex/core";
import { Badge, EpistemicBadge, TrustBadge } from "@memex/ui";
import type { EpistemicStatus, TrustTier } from "@memex/ui";

const STATUSES: readonly string[] = ["verified", "asserted", "inferred", "disputed", "superseded"];

/** A tier or status from a newer server still shows, as what it says, rather than failing. */
export const TierOf = ({ tier }: { readonly tier: string }) => ((TRUST_TIERS as readonly string[]).includes(tier) ? <TrustBadge tier={tier as TrustTier} /> : <Badge>{tier}</Badge>);

export const EpistemicOf = ({ status }: { readonly status: string | null }) => {
  if (status === null) return null;
  const lower = status.toLowerCase();
  return STATUSES.includes(lower) ? <EpistemicBadge status={lower as EpistemicStatus} /> : <Badge>{status}</Badge>;
};
