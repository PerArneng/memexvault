// Generated OpenAPI types, and the translation from wire data to domain data. See docs/frontend-architecture.md.
export type { components, paths } from "./generated/schema";
export {
  failureFromWire,
  identityFromWire,
  lineageStepFromWire,
  objectsFromWire,
  vaultFromWire,
  vaultPageFromWire,
  wikiAttributionFromWire,
  wikiBacklinkPageFromWire,
  wikiClaimFromWire,
  wikiDiffFromWire,
  wikiOutgoingPageFromWire,
  wikiPageFromWire,
  wikiRedirectFromWire,
  wikiRevisionPageFromWire,
  wikiSummaryFromWire,
  wikiSummaryPageFromWire,
  wikiUnresolvedPageFromWire,
  wikiWriteFromWire,
} from "./translate";
export {
  changeOriginFromWire,
  citingClaimsFromWire,
  claimLineageFromWire,
  derivationPageFromWire,
  flaggedPagesFromWire,
  mediaFromWire,
  pageListFromWire,
  pageWordsFromWire,
  proposalOfTrust,
  renditionFromWire,
  segmentFromWire,
  segmentPageFromWire,
  sourceFromWire,
  sourcePageFromWire,
  transcriptFromWire,
} from "./translate-documents";
export { activityFrameFromWire, activityPageFromWire, documentActivityFromWire, jobFromWire, jobPageFromWire, uploadLimitsFromWire, uploadResultFromWire } from "./translate-activity";
export { evidencePackageFromWire, renditionReadingFromWire } from "./translate-search";
export type { LineageStep, ObjectPages, WireHeaders } from "./translate";
