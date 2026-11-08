import { NO_FILTER, parseReviewRoute, reviewPath } from "@memex/core";
import type { Identity, VaultId } from "@memex/types";
import { EmptyState } from "@memex/ui";
import { useLocation } from "@tanstack/react-router";
import { AppLink } from "../wiki/wiki-common";
import { AutoCommitView } from "./auto-commit-view";
import { ChangesetDetail, ChangesetList } from "./changesets-view";
import { ContradictionsView } from "./contradictions-view";
import { QueueView } from "./queue-view";

const ReviewScreen = ({ identity, vault, rest }: { readonly identity: Identity; readonly vault: VaultId; readonly rest: readonly string[] }) => {
  const location = useLocation();
  const params = new URLSearchParams(location.searchStr);
  const route = parseReviewRoute(rest, (name) => params.get(name));
  const tab = route.kind === "changeset" ? "changesets" : route.kind;
  const tabs = [
    { id: "queue", label: "Queue", to: reviewPath(vault, { kind: "queue", filter: NO_FILTER, item: null }) },
    { id: "changesets", label: "Changes", to: reviewPath(vault, { kind: "changesets", state: "" }) },
    { id: "contradictions", label: "Contradictions", to: reviewPath(vault, { kind: "contradictions" }) },
    { id: "auto_commit", label: "Auto-commit", to: reviewPath(vault, { kind: "auto_commit" }) },
  ];
  return (
    <div className="flex flex-col gap-3">
      <nav aria-label="Review" className="flex flex-wrap gap-4 border-b border-border pb-1 text-sm">
        {tabs.map((t) => <AppLink key={t.id} to={t.to} aria-current={t.id === tab ? "page" : undefined} className={t.id === tab ? "font-medium text-text underline" : "text-accent underline"}>{t.label}</AppLink>)}
      </nav>
      {route.kind === "queue" && <QueueView key={JSON.stringify(route.filter)} identity={identity} vault={vault} filter={route.filter} item={route.item} />}
      {route.kind === "changesets" && <ChangesetList vault={vault} state={route.state} />}
      {route.kind === "changeset" && <ChangesetDetail key={route.id} identity={identity} vault={vault} id={route.id} />}
      {route.kind === "contradictions" && <ContradictionsView vault={vault} />}
      {route.kind === "auto_commit" && <AutoCommitView identity={identity} vault={vault} />}
      {route.kind === "unknown" && <EmptyState title="There's nothing at this address."><AppLink to={reviewPath(vault, { kind: "queue", filter: NO_FILTER, item: null })}>Go to the queue</AppLink></EmptyState>}
    </div>
  );
};

export default ReviewScreen;
