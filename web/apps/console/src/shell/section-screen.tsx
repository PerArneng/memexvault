import { accessTo, sectionById, sectionPath, visibleSections } from "@memex/core";
import type { Section } from "@memex/core";
import type { Identity, VaultId } from "@memex/types";
import { EmptyState, Skeleton } from "@memex/ui";
import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { lazy, Suspense } from "react";
import { useEngine } from "../engine-context";
import { QUERY_KEYS, unwrap } from "../query";
import { PermissionMessage, QueryView } from "./query-view";

// Lazy, as it brings in CodeMirror and the Markdown renderer, which nothing else needs yet.
const WikiScreen = lazy(() => import("../wiki/wiki-screen"));
// Lazy, as nothing before a signed-in user opens a document needs the viewer.
const DocumentsScreen = lazy(() => import("../documents/documents-screen"));
// Lazy, as only a reviewer needs it.
const ReviewScreen = lazy(() => import("../review/review-screen"));
// Lazy, so the sign-in route's budget doesn't pay for it.
const EntitiesScreen = lazy(() => import("../entities/entities-screen"));
// Lazy, as nothing before a signed-in user watches ingest needs the live board.
const IngestScreen = lazy(() => import("../ingest/ingest-screen"));
const SearchScreen = lazy(() => import("../search/search-screen"));
// Lazy, as only someone looking at audits needs it.
const AuditScreen = lazy(() => import("../audit/audit-screen"));
// Lazy, as the chat brings in Markdown and, on its first message, the AG-UI client.
const ChatPanel = lazy(() => import("../chat/chat-panel"));
// Lazy, as only an admin needs it.
const AdminScreen = lazy(() => import("../admin/admin-screen"));

interface SectionScreenProps {
  readonly identity: Identity;
  readonly vault: VaultId;
  readonly sectionName: string;
  readonly rest: readonly string[];
}

const Allowed = ({ identity, vault, section, rest }: { readonly identity: Identity; readonly vault: VaultId; readonly section: Section; readonly rest: readonly string[] }) => {
  // Admin reads the vault only on the screens that need it, so its status screen still works with Postgres down. See spec 048.
  if (section.id === "admin") return <AdminSection identity={identity} vault={vault} rest={rest} />;
  return <VaultScreen identity={identity} vault={vault} section={section} rest={rest} />;
};

/** Names the vault from the list the shell already holds, rather than a read of its own that would fail with Postgres down. */
const AdminSection = ({ identity, vault, rest }: { readonly identity: Identity; readonly vault: VaultId; readonly rest: readonly string[] }) => {
  const engine = useEngine();
  const vaults = useQuery({ queryKey: QUERY_KEYS.vaults, queryFn: async () => unwrap(await engine.listVaults()) });
  const name = vaults.data?.find((v) => v.id === vault)?.name;
  return (
    <div className="flex flex-col gap-4">
      {name !== undefined && <p className="text-sm text-text-muted">In vault <span className="text-text">{name}</span></p>}
      <Suspense fallback={<Skeleton className="h-24 w-full" />}>
        <AdminScreen identity={identity} vault={vault} rest={rest} />
      </Suspense>
    </div>
  );
};

const VaultScreen = ({ identity, vault, section, rest }: { readonly identity: Identity; readonly vault: VaultId; readonly section: Section; readonly rest: readonly string[] }) => {
  const engine = useEngine();
  const query = useQuery({ queryKey: QUERY_KEYS.vault(vault), queryFn: async () => unwrap(await engine.getVault(vault)) });
  return (
    <QueryView query={query} label="this vault">
      {(data) => (
        <div className="flex flex-col gap-4">
          <p className="text-sm text-text-muted">
            In vault <span className="text-text">{data.name}</span>
            {section.id !== "pages" && section.id !== "documents" && section.id !== "review" && section.id !== "entities" && section.id !== "search" && section.id !== "ingest" && section.id !== "chat" && section.id !== "audit" && rest[0] !== undefined && <> · <span className="font-mono text-text">{rest[0]}</span></>}
          </p>
          {section.id === "pages" ? (
            <Suspense fallback={<Skeleton className="h-24 w-full" />}>
              <WikiScreen identity={identity} vault={vault} rest={rest} />
            </Suspense>
          ) : section.id === "documents" ? (
            <Suspense fallback={<Skeleton className="h-24 w-full" />}>
              <DocumentsScreen identity={identity} vault={vault} rest={rest} />
            </Suspense>
          ) : section.id === "review" ? (
            <Suspense fallback={<Skeleton className="h-24 w-full" />}>
              <ReviewScreen identity={identity} vault={vault} rest={rest} />
            </Suspense>
          ) : section.id === "entities" ? (
            <Suspense fallback={<Skeleton className="h-24 w-full" />}>
              <EntitiesScreen identity={identity} vault={vault} rest={rest} />
            </Suspense>
          ) : section.id === "ingest" ? (
            <Suspense fallback={<Skeleton className="h-24 w-full" />}>
              <IngestScreen identity={identity} vault={vault} rest={rest} />
            </Suspense>
          ) : section.id === "search" ? (
            <Suspense fallback={<Skeleton className="h-24 w-full" />}>
              <SearchScreen vault={vault} rest={rest} />
            </Suspense>
          ) : section.id === "audit" ? (
            <Suspense fallback={<Skeleton className="h-24 w-full" />}>
              <AuditScreen identity={identity} vault={vault} rest={rest} />
            </Suspense>
          ) : section.id === "chat" ? (
            <Suspense fallback={<Skeleton className="h-24 w-full" />}>
              <div className="flex h-[calc(100vh-10rem)] min-h-96 flex-col overflow-hidden rounded-lg border border-border">
                <ChatPanel identity={identity} vault={vault} attachable={null} />
              </div>
            </Suspense>
          ) : (
            /* Each feature screen replaces this in its own spec. */
            <EmptyState title={`${section.label} isn't built yet.`}>This console build has the navigation for it, and the screen comes later.</EmptyState>
          )}
        </div>
      )}
    </QueryView>
  );
};

/** Decides what a URL may show before anything loads, so a direct link gets the same answer as the navigation. */
export const SectionScreen = ({ identity, vault, sectionName, rest }: SectionScreenProps) => {
  const section = sectionById(sectionName);
  if (section === null) {
    return (
      <EmptyState title="There's no such screen in this vault.">
        <Link to={sectionPath(vault, "documents")} className="text-accent underline">Go to Documents</Link>
      </EmptyState>
    );
  }
  const access = accessTo(identity, vault, section);
  return (
    <section className="flex flex-col gap-4" aria-labelledby="screen-title">
      <h1 id="screen-title" className="text-xl font-medium">{section.label}</h1>
      {access.kind === "allowed" && <Allowed identity={identity} vault={vault} section={section} rest={rest} />}
      {access.kind === "forbidden" && (
        <PermissionMessage title={`You don't have permission to use ${section.label} in this vault.`}>
          It needs <strong>{access.needs}</strong> scope, and your token has <strong>{access.have}</strong>. Ask an administrator for a token with more access.
          {visibleSections(identity, vault)[0] !== undefined && (
            <> <Link to={sectionPath(vault, visibleSections(identity, vault)[0]?.id ?? "documents")} className="text-accent underline">Go to {visibleSections(identity, vault)[0]?.label}</Link></>
          )}
        </PermissionMessage>
      )}
      {access.kind === "no_vault_access" && (
        <PermissionMessage title="This vault doesn't exist, or your token has no access to it.">
          <Link to="/" className="text-accent underline">Go to your vaults</Link>
        </PermissionMessage>
      )}
    </section>
  );
};
