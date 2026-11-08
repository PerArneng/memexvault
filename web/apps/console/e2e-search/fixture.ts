// The 50 results item 1 measures, in the API's own shape. The text is written for this test, with the letters item 4 cares about.
const SENTENCES = [
  "The measured river runs past the old mill, and the survey of its banks took three summers to finish.",
  "Räksmörgås och blåbär serverades på bryggan när mätningen av ån var klar för året.",
  "Die Straße über den Fluß führt zur Bäckerei in München, wo die Messung begann und die Grüße warteten.",
  "El niño de España soñaba con la montaña mientras la medición del río seguía su curso.",
  "Each quarter the report lists what changed, who confirmed it, and which documents the numbers came from.",
];

const textOf = (n: number): string => {
  const out: string[] = [];
  let length = 0;
  for (let i = 0; length < 1100; i += 1) {
    const sentence = SENTENCES[(n + i) % SENTENCES.length] ?? "";
    out.push(sentence);
    length += sentence.length + 1;
  }
  return out.join(" ");
};

const citation = (n: number, quote: string | null) => ({
  anchor: "located",
  source_id: `source-${String(n % 7)}`,
  source_name: `quarterly report ${String(n % 7)}.pdf`,
  source_sha256: "0".repeat(64),
  trust_tier: ["verified", "trusted", "unverified", "untrusted"][n % 4],
  segment_id: `segment-${String(n)}`,
  evidence_id: quote === null ? null : `evidence-${String(n)}`,
  rendition_id: `rendition-${String(n % 7)}`,
  start: n * 100,
  end: n * 100 + 1000,
  page_from: n % 12,
  page_to: n % 12,
  rects: quote === null ? [] : [{ page: n % 12, x0: 7200, y0: 10000, x1: 40000, y1: 11200 }],
  quote,
  t_start_ms: null,
  t_end_ms: null,
  speaker: null,
});

const item = (n: number, kind: string) => {
  const text = textOf(n);
  const claim = kind === "claim";
  return {
    id: `${kind}-${String(n)}`,
    kind,
    text,
    heading: claim ? null : `Section ${String(n)}`,
    language: ["en", "sv", "de", "es"][n % 4],
    epistemic: claim ? "asserted" : null,
    trust_tiers: [["verified", "trusted", "unverified", "untrusted"][n % 4]],
    valid_from_ms: null,
    valid_to_ms: null,
    score: 1 / (n + 1),
    estimated_tokens: Math.ceil(text.length / 4),
    page_id: null,
    citations: [citation(n, claim ? text.slice(0, 160) : null)],
    conflicts: claim && n % 5 === 0 ? [{ claim_id: `rival-${String(n)}`, text: "A rival value of the same fact", epistemic: "asserted", source_ids: [`source-${String(n % 7)}`] }] : [],
  };
};

/** 10 claims, 30 segments and 10 sections. */
export const fiftyResults = () => ({
  path: "natural",
  items: [...Array.from({ length: 10 }, (_, n) => item(n, "claim")), ...Array.from({ length: 30 }, (_, n) => item(n + 10, "segment")), ...Array.from({ length: 10 }, (_, n) => item(n + 40, "section"))],
  low_relevance: false,
  notices: [],
  budget: { limit: 20000, used: 15000, truncated: false },
  degraded: [],
  facets: {
    kind: [{ value: "claim", count: 10 }, { value: "segment", count: 30 }],
    type: [{ value: "application/pdf", count: 40 }],
    source_id: Array.from({ length: 7 }, (_, n) => ({ value: `source-${String(n)}`, count: 7 })),
    trust_tier: [{ value: "trusted", count: 12 }, { value: "verified", count: 13 }],
    epistemic: [{ value: "asserted", count: 10 }],
  },
  next_cursor: "r50",
});
