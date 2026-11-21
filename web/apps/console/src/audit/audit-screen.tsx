import { auditPath, parseAuditRoute } from "@memex/core";
import type { Identity, VaultId } from "@memex/types";
import { EmptyState } from "@memex/ui";
import { useLocation } from "@tanstack/react-router";
import { AppLink } from "../wiki/wiki-common";
import { AgentsView } from "./agents-view";
import { RecallView } from "./recall-view";
import { RunDetail } from "./run-detail";
import { RunsView } from "./runs-view";
import { ScheduleView } from "./schedule-view";

const AuditScreen = ({ identity, vault, rest }: { readonly identity: Identity; readonly vault: VaultId; readonly rest: readonly string[] }) => {
  const location = useLocation();
  const params = new URLSearchParams(location.searchStr);
  const route = parseAuditRoute(rest, (name) => params.get(name));
  const tab = route.kind === "run" ? "runs" : route.kind;
  const tabs = [
    { id: "runs", label: "Runs", to: auditPath(vault, { kind: "runs" }) },
    { id: "schedule", label: "Schedule and budget", to: auditPath(vault, { kind: "schedule" }) },
    { id: "recall", label: "Recall", to: auditPath(vault, { kind: "recall" }) },
    { id: "agents", label: "Agent usage", to: auditPath(vault, { kind: "agents", month: null }) },
  ];
  return (
    <div className="flex flex-col gap-3">
      <nav aria-label="Audit" className="flex flex-wrap gap-4 border-b border-border pb-1 text-sm">
        {tabs.map((t) => <AppLink key={t.id} to={t.to} aria-current={t.id === tab ? "page" : undefined} className={t.id === tab ? "font-medium text-text underline" : "text-accent underline"}>{t.label}</AppLink>)}
      </nav>
      {route.kind === "runs" && <RunsView identity={identity} vault={vault} />}
      {route.kind === "run" && <RunDetail key={route.id} vault={vault} id={route.id} findingsOnly={route.findingsOnly} />}
      {route.kind === "schedule" && <ScheduleView identity={identity} vault={vault} />}
      {route.kind === "recall" && <RecallView vault={vault} />}
      {route.kind === "agents" && <AgentsView identity={identity} vault={vault} month={route.month} />}
      {route.kind === "unknown" && <EmptyState title="There's nothing at this address."><AppLink to={auditPath(vault, { kind: "runs" })}>Go to the runs</AppLink></EmptyState>}
    </div>
  );
};

export default AuditScreen;
