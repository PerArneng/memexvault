import { AlertTriangle, Check, CircleDashed, FileWarning, Hourglass, Loader, ShieldCheck, ShieldQuestion, Sparkles, Swords, X } from "lucide-react";
import type { ComponentType, ReactNode } from "react";

export type Tone = "neutral" | "success" | "warning" | "danger" | "info";

const TONES: Readonly<Record<Tone, string>> = {
  neutral: "bg-neutral-surface text-text",
  success: "bg-success-surface text-success",
  warning: "bg-warning-surface text-warning",
  danger: "bg-danger-surface text-danger",
  info: "bg-info-surface text-info",
};

export const Badge = ({ tone = "neutral", children }: { readonly tone?: Tone; readonly children: ReactNode }) => (
  <span className={`inline-flex h-5 items-center gap-1 rounded-sm px-1.5 text-xs font-medium whitespace-nowrap ${TONES[tone]}`}>{children}</span>
);

type Icon = ComponentType<{ readonly size?: number; readonly "aria-hidden"?: boolean }>;
type Spec = { readonly tone: Tone; readonly label: string; readonly icon: Icon };

// Every status has an icon and a word, so colour is never the only signal.
const iconBadge = ({ tone, label, icon: I }: Spec) => (
  <Badge tone={tone}>
    <I size={12} aria-hidden={true} />
    {label}
  </Badge>
);

export type TrustTier = "verified" | "trusted" | "unverified" | "untrusted";
const TRUST: Readonly<Record<TrustTier, Spec>> = {
  verified: { tone: "success", label: "Verified", icon: ShieldCheck },
  trusted: { tone: "info", label: "Trusted", icon: ShieldCheck },
  unverified: { tone: "neutral", label: "Unverified", icon: ShieldQuestion },
  untrusted: { tone: "warning", label: "Untrusted", icon: AlertTriangle },
};
export const TrustBadge = ({ tier }: { readonly tier: TrustTier }) => iconBadge(TRUST[tier]);

export type EpistemicStatus = "asserted" | "inferred" | "disputed" | "superseded";
const EPISTEMIC: Readonly<Record<EpistemicStatus, Spec>> = {
  asserted: { tone: "info", label: "Asserted", icon: Check },
  inferred: { tone: "neutral", label: "Inferred", icon: Sparkles },
  disputed: { tone: "warning", label: "Disputed", icon: Swords },
  superseded: { tone: "neutral", label: "Superseded", icon: CircleDashed },
};
export const EpistemicBadge = ({ status }: { readonly status: EpistemicStatus }) => iconBadge(EPISTEMIC[status]);

export type JobStage = "waiting" | "running" | "partial" | "failed" | "done";
const STAGE: Readonly<Record<JobStage, Spec>> = {
  waiting: { tone: "neutral", label: "Waiting", icon: Hourglass },
  running: { tone: "info", label: "Running", icon: Loader },
  partial: { tone: "warning", label: "Partial", icon: FileWarning },
  failed: { tone: "danger", label: "Failed", icon: X },
  done: { tone: "success", label: "Done", icon: Check },
};
export const StageBadge = ({ stage, detail }: { readonly stage: JobStage; readonly detail?: string }) => {
  const s = STAGE[stage];
  return iconBadge(detail === undefined ? s : { ...s, label: `${s.label}: ${detail}` });
};

export const ConflictBadge = ({ count }: { readonly count: number }) => iconBadge({ tone: "warning", label: `${count} ${count === 1 ? "conflict" : "conflicts"}`, icon: Swords });
