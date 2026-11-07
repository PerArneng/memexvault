import { documentPath } from "@memex/core";
import type { Identity, VaultId } from "@memex/types";
import { EmptyState } from "@memex/ui";
import { AppLink } from "../wiki/wiki-common";
import { useDocumentRoute } from "./document-common";
import { LineageView } from "./lineage-view";
import { SourceDetail } from "./source-detail";
import { SourceList } from "./source-list";

const DocumentsScreen = ({ identity, vault, rest }: { readonly identity: Identity; readonly vault: VaultId; readonly rest: readonly string[] }) => {
  const route = useDocumentRoute(rest);
  switch (route.kind) {
    case "list":
      return <SourceList identity={identity} vault={vault} filter={route.filter} />;
    case "source":
      return <SourceDetail key={route.id} identity={identity} vault={vault} route={route} />;
    case "lineage":
      return <LineageView vault={vault} claim={route.claim} evidence={route.evidence} />;
    case "unknown":
      return (
        <EmptyState title="There's nothing at this address.">
          <AppLink to={documentPath(vault, { kind: "list", filter: { text: "", tier: "", kind: "", flagged: false } })}>Go to Documents</AppLink>
        </EmptyState>
      );
  }
};

export default DocumentsScreen;
