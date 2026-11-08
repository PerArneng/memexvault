import { canChangeSource, documentPath, formatSize, scopeIn } from "@memex/core";
import type { DocumentTab, SourceViewRoute } from "@memex/core";
import type { DocumentSource, Identity, VaultId } from "@memex/types";
import { Badge, Button } from "@memex/ui";
import { useQuery } from "@tanstack/react-query";
import { Download } from "lucide-react";
import { useState } from "react";
import { useEngine } from "../engine-context";
import { QUERY_KEYS, unwrap } from "../query";
import { QueryView } from "../shell/query-view";
import { TierOf } from "../viewer/badges";
import { AppLink, formatTime, LINK_BUTTON } from "../wiki/wiki-common";
import { SourceQueryView } from "./document-common";
import { ReprocessPanel, TrustTierPanel } from "./source-actions";
import { ClaimsTab, RenditionsTab, SegmentsTab, VersionsTab } from "./source-tabs";
import { SourceViewTab } from "./source-view";

const TABS: readonly { readonly id: DocumentTab; readonly label: string }[] = [
  { id: "view", label: "View" },
  { id: "segments", label: "Segments" },
  { id: "claims", label: "Claims" },
  { id: "renditions", label: "Renditions" },
  { id: "versions", label: "Versions" },
];

const Header = ({ vault, source, identity }: { readonly vault: VaultId; readonly source: DocumentSource; readonly identity: Identity }) => {
  const engine = useEngine();
  const [panel, setPanel] = useState<"trust" | "reprocess" | null>(null);
  const writable = canChangeSource(identity, vault);
  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-2">
        <h2 className="min-w-0 text-lg font-medium break-words">{source.name}</h2>
        <TierOf tier={source.trustTier} />
        <Badge>Version {source.versionNo}</Badge>
        <span className="ml-auto flex flex-wrap items-center gap-2">
          <a href={engine.fileUrl(vault, source.id)} className={LINK_BUTTON}><Download size={14} aria-hidden={true} /> Download original</a>
          <Button aria-expanded={panel === "trust"} onClick={() => setPanel(panel === "trust" ? null : "trust")}>Change trust tier</Button>
          {writable && <Button aria-expanded={panel === "reprocess"} onClick={() => setPanel(panel === "reprocess" ? null : "reprocess")}>Reprocess</Button>}
        </span>
      </div>
      <p className="flex flex-wrap gap-x-3 text-xs text-text-muted">
        <span>{source.mediaType}</span>
        <span>{formatSize(source.size)}</span>
        <span>recorded {formatTime(source.recordedAt)}</span>
        <span className="font-mono break-all">sha256 {source.sha256}</span>
        {!writable && <span>Your token has {scopeIn(identity, vault) ?? "no"} scope, so it can read this source and not change it.</span>}
      </p>
      {panel === "trust" && <TrustTierPanel vault={vault} source={source} identity={identity} onClose={() => setPanel(null)} />}
      {panel === "reprocess" && <ReprocessPanel vault={vault} source={source} identity={identity} onClose={() => setPanel(null)} />}
    </div>
  );
};

export const SourceDetail = ({ identity, vault, route }: { readonly identity: Identity; readonly vault: VaultId; readonly route: SourceViewRoute }) => {
  const engine = useEngine();
  const sourceQuery = useQuery({ queryKey: QUERY_KEYS.source(vault, route.id), queryFn: async () => unwrap(await engine.getSource(vault, route.id)) });
  const renditionsQuery = useQuery({ queryKey: QUERY_KEYS.renditions(vault, route.id), queryFn: async () => unwrap(await engine.listRenditions(vault, route.id)) });
  const renditions = renditionsQuery.data ?? [];
  return (
    <SourceQueryView vault={vault} query={sourceQuery} label="this document">
      {(source) => (
        <div className="flex flex-col gap-3">
          <Header vault={vault} source={source} identity={identity} />
          <nav aria-label="This document" className="flex gap-1 border-b border-border">
            {TABS.map((tab) => (
              <AppLink
                key={tab.id}
                to={documentPath(vault, { ...route, tab: tab.id, ...(tab.id === "segments" ? {} : { rendition: null }) })}
                aria-current={route.tab === tab.id ? "page" : undefined}
                className={`-mb-px border-b-2 px-3 py-1.5 text-sm ${route.tab === tab.id ? "border-accent text-text" : "border-transparent text-text-muted hover:text-text"}`}
              >
                {tab.label}
              </AppLink>
            ))}
          </nav>
          {route.tab === "view" && <SourceViewTab vault={vault} source={source} renditions={renditions} route={route} />}
          {route.tab === "segments" && (
            <QueryView query={renditionsQuery} label="renditions">
              {(items) => <SegmentsTab vault={vault} sourceId={source.id} renditions={items} renditionId={route.rendition} />}
            </QueryView>
          )}
          {route.tab === "claims" && <ClaimsTab vault={vault} sourceId={source.id} />}
          {route.tab === "renditions" && (
            <QueryView query={renditionsQuery} label="renditions">
              {(items) => <RenditionsTab vault={vault} sourceId={source.id} renditions={items} />}
            </QueryView>
          )}
          {route.tab === "versions" && <VersionsTab vault={vault} source={source} />}
        </div>
      )}
    </SourceQueryView>
  );
};
