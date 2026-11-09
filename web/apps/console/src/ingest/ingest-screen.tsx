import { ingestPath, parseIngestRoute } from "@memex/core";
import type { Identity, VaultId } from "@memex/types";
import { EmptyState, Skeleton } from "@memex/ui";
import { useLocation } from "@tanstack/react-router";
import { AppLink } from "../wiki/wiki-common";
import { DocumentActivityView } from "./document-activity";
import { FailedJobs } from "./failed-jobs";
import { useActivityWatch } from "./ingest-common";
import { LiveBoard } from "./live-board";

const IngestScreen = ({ identity, vault, rest }: { readonly identity: Identity; readonly vault: VaultId; readonly rest: readonly string[] }) => {
  const { searchStr } = useLocation();
  const watch = useActivityWatch(vault);
  const query = new URLSearchParams(searchStr);
  const route = parseIngestRoute(rest, (name) => query.get(name));
  if (watch === null) return <Skeleton className="h-24 w-full" />;
  switch (route.kind) {
    case "board":
      return <LiveBoard identity={identity} vault={vault} watch={watch} rest={rest} search={searchStr} />;
    case "document":
      return <DocumentActivityView key={route.id} identity={identity} vault={vault} id={route.id} watch={watch} />;
    case "failed":
      return <FailedJobs identity={identity} vault={vault} state={route.state} watch={watch} />;
    case "unknown":
      return (
        <EmptyState title="There's nothing at this address.">
          <AppLink to={ingestPath(vault, { kind: "board", stage: "", status: "", text: "" })}>Go to the board</AppLink>
        </EmptyState>
      );
  }
};

export default IngestScreen;
