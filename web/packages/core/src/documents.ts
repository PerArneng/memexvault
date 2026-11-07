import type { DocumentSource, EvidenceStep, FlaggedPage, Identity, PageFlag, PageWord, Rendition, TranscriptPart, TranscriptWord, VaultId } from "@memex/types";
import type { SourceFilter } from "./document-routes";
import { scopeIn } from "./navigation";

export type MediaKind = "pdf" | "text" | "audio" | "video" | "other";

export const mediaKind = (mediaType: string): MediaKind => {
  if (mediaType === "application/pdf") return "pdf";
  if (mediaType.startsWith("text/")) return "text";
  if (mediaType.startsWith("audio/")) return "audio";
  if (mediaType.startsWith("video/")) return "video";
  return "other";
};

export const sourceKinds: readonly { readonly id: MediaKind; readonly label: string }[] = [
  { id: "pdf", label: "PDF" },
  { id: "text", label: "Text" },
  { id: "audio", label: "Audio" },
  { id: "video", label: "Video" },
];

export const TRUST_TIERS = ["verified", "trusted", "unverified", "untrusted"] as const;
export type TrustTierName = (typeof TRUST_TIERS)[number];

/** The pages with flags, by source. */
export const flaggedSources = (flagged: readonly FlaggedPage[]): ReadonlySet<string> => new Set(flagged.map((f) => f.sourceId));

export const filterSources = (sources: readonly DocumentSource[], filter: SourceFilter, flagged: ReadonlySet<string>): readonly DocumentSource[] => {
  const needle = filter.text.trim().toLowerCase();
  return sources.filter(
    (s) =>
      (needle === "" || s.name.toLowerCase().includes(needle)) &&
      (filter.tier === "" || s.trustTier === filter.tier) &&
      (filter.kind === "" || mediaKind(s.mediaType) === filter.kind) &&
      (!filter.flagged || flagged.has(s.id)),
  );
};

export const flagText = (flag: PageFlag): string => {
  switch (flag.kind) {
    case "low_confidence":
      return "OCR had low confidence";
    case "hidden_text":
      return `${String(flag.words)} words of text that can't be seen on the page`;
    case "zero_size_text":
      return `${String(flag.words)} words of zero-size text`;
    case "invisible_unicode":
      return `${String(flag.chars)} invisible characters`;
    case "ocr_failed":
      return `OCR failed: ${flag.reason}`;
    case "other":
      return flag.name;
  }
};

/** Whether this token may ask for a change to a source. The server checks again. */
export const canChangeSource = (identity: Identity, vault: VaultId): boolean => {
  const scope = scopeIn(identity, vault);
  return scope === "propose" || scope === "admin";
};

const atLeastTrusted = (tier: string): boolean => tier === "trusted" || tier === "verified";

export type TrustEffect = "same" | "review_always" | "person_commits_agent_waits" | "lowers";

/** What the server's rules do with a change, so the screen can say it before it's asked. `governance.rs` decides, and the answer shows what it did. */
export const trustEffect = (from: string, to: string): TrustEffect => {
  if (from === to) return "same";
  if (from === "untrusted") return "review_always";
  return !atLeastTrusted(from) && atLeastTrusted(to) ? "person_commits_agent_waits" : "lowers";
};

export type EvidenceState = "broken" | "superseded" | "current";

/** Broken says more than superseded, and an old rendition can be both. */
export const evidenceState = (e: Pick<EvidenceStep, "broken" | "superseded">): EvidenceState => {
  if (e.broken) return "broken";
  return e.superseded ? "superseded" : "current";
};

/** A word's text runs along the turn of a page read from its text layer. OCR reads the page as displayed. */
export const pageAngle = (rendition: Pick<Rendition, "pages"> | null, page: { readonly index: number; readonly rotation: number }): number =>
  rendition?.pages.find((p) => p.index === page.index)?.route === "ocr" ? 0 : page.rotation;

export const pageWordsByPage = (words: readonly PageWord[]): ReadonlyMap<number, readonly PageWord[]> => {
  const byPage = new Map<number, PageWord[]>();
  for (const word of words) {
    const list = byPage.get(word.page);
    if (list === undefined) byPage.set(word.page, [word]);
    else list.push(word);
  }
  return byPage;
};

/** The part playing at a time, by bisection. In a pause it's the last that started, as there's nothing better to show. */
export const activePart = (parts: readonly TranscriptPart[], ms: number): number | null => {
  if (parts.length === 0 || (parts[0]?.tStartMs ?? 0) > ms) return null;
  let low = 0;
  let high = parts.length - 1;
  while (low < high) {
    const middle = Math.ceil((low + high) / 2);
    if ((parts[middle]?.tStartMs ?? 0) <= ms) low = middle;
    else high = middle - 1;
  }
  return low;
};

export const wordAt = (words: readonly TranscriptWord[], ms: number): number | null => {
  const index = words.findIndex((w) => w.startMs <= ms && ms < w.endMs);
  return index === -1 ? null : index;
};

/** Parts overlapping a cited range, so the screen can mark them. */
export const partsInRange = (parts: readonly TranscriptPart[], fromMs: number, toMs: number | null): readonly number[] =>
  parts.flatMap((part, index) => {
    const end = toMs ?? fromMs;
    return part.tStartMs <= end && part.tEndMs >= fromMs ? [index] : [];
  });

const pad = (n: number): string => String(n).padStart(2, "0");

export const formatClock = (ms: number): string => {
  const total = Math.max(0, Math.floor(ms / 1000));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = total % 60;
  return hours > 0 ? `${String(hours)}:${pad(minutes)}:${pad(seconds)}` : `${String(minutes)}:${pad(seconds)}`;
};

const KIB = 1024;

export const formatSize = (bytes: number): string => {
  if (bytes < KIB) return `${String(bytes)} B`;
  if (bytes < KIB * KIB) return `${(bytes / KIB).toFixed(1)} KB`;
  return `${(bytes / (KIB * KIB)).toFixed(1)} MB`;
};

export type Playability = "yes" | "no" | "try";

/** What a media element's `canPlayType` says. An empty answer is a real no, and the others only promise to try. */
export const playability = (answer: string): Playability => (answer === "" ? "no" : answer === "probably" ? "yes" : "try");
