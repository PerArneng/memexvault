import { documentPath, entityGraph, graphClaims, wikiPath } from "@memex/core";
import type { GraphNode } from "@memex/core";
import type { EntityDetail, VaultId } from "@memex/types";
import { Badge, EmptyState } from "@memex/ui";
import { useQuery } from "@tanstack/react-query";
import { useMemo } from "react";
import { useEngine } from "../engine-context";
import { unwrap } from "../query";
import { QueryView } from "../shell/query-view";
import { AppLink, Callout } from "../wiki/wiki-common";
import { ENTITY_KEYS } from "./entity-common";

const CENTER = 50;
const MAX_LABEL = 22;

const nodeName = (node: GraphNode): string => node.title ?? node.objectId ?? "A link to a page that doesn't exist";
const shortName = (text: string): string => (text.length > MAX_LABEL ? `${text.slice(0, MAX_LABEL - 1)}…` : text);

/** A page is the only neighbor the API titles. Anything else with an id here is a claim, as those are the edges an entity's claims have, and lineage says so if not. */
const NodeLink = ({ vault, node }: { readonly vault: VaultId; readonly node: GraphNode }) => {
  if (node.objectId === null) return <span className="text-text-muted">{nodeName(node)}</span>;
  if (node.title !== null) return <AppLink to={wikiPath(vault, { kind: "read", id: node.objectId, revision: null })}>{node.title}</AppLink>;
  return <AppLink to={documentPath(vault, { kind: "lineage", claim: node.objectId, evidence: null })}><span className="font-mono text-xs break-all">{node.objectId}</span></AppLink>;
};

/** Nearby objects through the neighborhood API. An entity's links are on its claims, so it asks about those too, and says which field each link came through. The list is the real content, and the picture repeats it. */
export const GraphPanel = ({ vault, entity, label }: { readonly vault: VaultId; readonly entity: EntityDetail; readonly label: string }) => {
  const engine = useEngine();
  const query = useQuery({ queryKey: ENTITY_KEYS.graph(vault, entity.id, entity.version), queryFn: async () => unwrap(await engine.entityNeighborhood(vault, entity)) });
  const claimIds = useMemo(() => new Set(graphClaims(entity).claims.map((c) => c.id)), [entity]);
  return (
    <section aria-label="Nearby objects" data-graph={true} className="flex flex-col gap-2 rounded-lg border border-border p-3">
      <h3 className="font-medium">Nearby objects</h3>
      <QueryView query={query} label="nearby objects">
        {(found) => {
          const graph = entityGraph(entity.id, found.answers, claimIds);
          return (
            <>
              {found.failures > 0 && <Callout tone="warning" title={`${String(found.failures)} of these lookups failed, so this may be missing some.`} />}
              {(found.truncated || graph.truncated) && <Callout tone="info" title="This shows the first links only, as this entity has more than fit here." />}
              {graph.nodes.length === 0 ? (
                <div data-graph-empty={true}><EmptyState title="No page or claim links to this entity yet." /></div>
              ) : (
                <div className="grid grid-cols-1 gap-3 lg:grid-cols-[minmax(0,22rem)_minmax(0,1fr)]">
                  <svg viewBox="0 0 100 100" role="img" aria-label={`${String(graph.nodes.length)} objects near ${label}`} className="w-full">
                    {graph.nodes.map((n) => <line key={n.key} x1={CENTER} y1={CENTER} x2={n.x} y2={n.y} className="stroke-border-strong" strokeWidth={0.4} />)}
                    <circle cx={CENTER} cy={CENTER} r={3} className="fill-accent" />
                    <text x={CENTER} y={CENTER + 7} textAnchor="middle" fontSize={3.2} className="fill-text">{shortName(label)}</text>
                    {graph.nodes.map((n) => (
                      <g key={n.key}>
                        <circle cx={n.x} cy={n.y} r={2} className={n.title === null ? "fill-info" : "fill-success"} />
                        <text x={n.x} y={n.y + 5} textAnchor="middle" fontSize={2.6} className="fill-text-muted">{shortName(nodeName(n))}</text>
                      </g>
                    ))}
                  </svg>
                  <ul className="flex flex-col gap-1 text-sm">
                    {graph.nodes.map((n) => (
                      <li key={n.key} data-graph-node={n.objectId ?? n.key} className="flex flex-wrap items-center gap-2">
                        <NodeLink vault={vault} node={n} />
                        {n.links.map((l) => (
                          <Badge key={l.edgeId} tone={l.kind === "contradicts" ? "warning" : "neutral"}>{l.kind}{l.viaField === null ? "" : ` via ${l.viaField}`}</Badge>
                        ))}
                      </li>
                    ))}
                  </ul>
                </div>
              )}
            </>
          );
        }}
      </QueryView>
    </section>
  );
};
