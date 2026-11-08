import type { Identity, ObjectSummary, Vault, VaultId } from "@memex/types";
import { entityPath } from "./entity-routes";
import { sectionPath, switchVaultPath, visibleSections } from "./navigation";
import type { SectionId } from "./navigation";

export type PaletteAction = { readonly kind: "go"; readonly to: string } | { readonly kind: "sign_out" };

export interface PaletteEntry {
  readonly id: string;
  readonly label: string;
  readonly hint: string;
  readonly action: PaletteAction;
}

const OBJECT_SECTION: Readonly<Record<ObjectSummary["kind"], { readonly section: SectionId; readonly hint: string }>> = {
  document: { section: "documents", hint: "document" },
  page: { section: "pages", hint: "page" },
  entity_type: { section: "entities", hint: "entity type" },
};

export interface PaletteInput {
  readonly identity: Identity;
  readonly pathname: string;
  readonly vault: VaultId | null;
  readonly vaults: readonly Vault[];
  readonly objects: readonly ObjectSummary[];
}

/** Everything the palette offers, already limited to what this token can use. Navigation comes first, then objects, then vaults. */
export const paletteEntries = ({ identity, pathname, vault, vaults, objects }: PaletteInput): readonly PaletteEntry[] => {
  const sections: readonly PaletteEntry[] =
    vault === null
      ? []
      : visibleSections(identity, vault).map((s) => ({ id: `go-${s.id}`, label: `Go to ${s.label}`, hint: "screen", action: { kind: "go", to: sectionPath(vault, s.id) } }));
  const opened: readonly PaletteEntry[] =
    vault === null
      ? []
      : objects.flatMap((o) => {
          const target = OBJECT_SECTION[o.kind];
          // Ids aren't unique across kinds, so the kind is part of the entry's id.
          return visibleSections(identity, vault).some((s) => s.id === target.section)
            ? [{ id: `open-${o.kind}-${o.id}`, label: o.label, hint: target.hint, action: { kind: "go", to: o.kind === "entity_type" ? entityPath(vault, { kind: "table", type: o.id, filters: [], columns: null, sort: null, find: "" }) : sectionPath(vault, target.section, o.id) } } as const]
            : [];
        });
  const others: readonly PaletteEntry[] = vaults
    .filter((v) => v.id !== vault)
    .map((v) => ({ id: `vault-${v.id}`, label: `Switch to ${v.name}`, hint: "vault", action: { kind: "go", to: switchVaultPath(identity, pathname, v.id) } }));
  return [...sections, ...opened, ...others, { id: "sign-out", label: "Sign out", hint: "session", action: { kind: "sign_out" } }];
};
