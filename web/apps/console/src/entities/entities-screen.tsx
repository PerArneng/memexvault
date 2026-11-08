import { canWritePages, entityPath } from "@memex/core";
import type { Identity, VaultId } from "@memex/types";
import { EmptyState } from "@memex/ui";
import { useLocation } from "@tanstack/react-router";
import { PermissionMessage } from "../shell/query-view";
import { AppLink } from "../wiki/wiki-common";
import { EntityDetailView } from "./entity-detail";
import { EditEntity, NewEntity } from "./entity-form";
import { MergeEntity, SameAsList, SplitEntity } from "./entity-proposals";
import { EntityTable } from "./entity-table";
import { routeFrom } from "./entity-common";
import { EditType, NewType } from "./type-editor";
import { TypeDetail, TypeList } from "./type-list";

const EntitiesScreen = ({ identity, vault, rest }: { readonly identity: Identity; readonly vault: VaultId; readonly rest: readonly string[] }) => {
  const { searchStr } = useLocation();
  const route = routeFrom(rest, searchStr);
  const writable = canWritePages(identity, vault);
  const writes = route.kind === "new_type" || route.kind === "edit_type" || route.kind === "new_entity" || route.kind === "edit" || route.kind === "merge" || route.kind === "split";
  if (writes && !writable) {
    return <PermissionMessage title="You don't have permission to propose changes in this vault.">It needs <strong>propose</strong> scope, and your token has <strong>read</strong>.</PermissionMessage>;
  }
  switch (route.kind) {
    case "types":
      return <TypeList vault={vault} writable={writable} />;
    case "new_type":
      return <NewType vault={vault} />;
    case "type":
      return <TypeDetail vault={vault} type={route.type} writable={writable} />;
    case "edit_type":
      return <EditType key={route.type} vault={vault} type={route.type} />;
    case "table":
      return <EntityTable key={route.type} vault={vault} route={route} writable={writable} />;
    case "new_entity":
      return <NewEntity key={route.type} vault={vault} type={route.type} />;
    case "same_as":
      return <SameAsList vault={vault} type={route.type} writable={writable} />;
    case "entity":
      return <EntityDetailView key={route.id} vault={vault} id={route.id} writable={writable} />;
    case "edit":
      return <EditEntity key={route.id} vault={vault} id={route.id} />;
    case "merge":
      return <MergeEntity key={route.id} vault={vault} id={route.id} into={route.into} />;
    case "split":
      return <SplitEntity key={route.id} vault={vault} id={route.id} />;
    case "unknown":
      return (
        <EmptyState title="There's nothing at this address.">
          <AppLink to={entityPath(vault, { kind: "types" })}>Go to Entities</AppLink>
        </EmptyState>
      );
  }
};

export default EntitiesScreen;
