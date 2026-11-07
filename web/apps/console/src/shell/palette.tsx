import { paletteEntries } from "@memex/core";
import type { Identity, Vault, VaultId } from "@memex/types";
import { CommandPalette } from "@memex/ui";
import type { Command } from "@memex/ui";
import { useQuery } from "@tanstack/react-query";
import { useLocation, useNavigate } from "@tanstack/react-router";
import { useEffect, useMemo } from "react";
import { useEngine } from "../engine-context";
import { QUERY_KEYS, unwrap } from "../query";

interface PaletteProps {
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
  readonly identity: Identity;
  readonly vault: VaultId | null;
  readonly vaults: readonly Vault[];
  readonly onSignOut: () => void;
}

export const Palette = ({ open, onOpenChange, identity, vault, vaults, onSignOut }: PaletteProps) => {
  const engine = useEngine();
  const navigate = useNavigate();
  const { pathname } = useLocation();
  const objects = useQuery({
    queryKey: QUERY_KEYS.objects(vault ?? ""),
    queryFn: async () => unwrap(await engine.listObjects(vault as VaultId)),
    enabled: open && vault !== null,
  });

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        onOpenChange(!open);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onOpenChange, open]);

  const commands = useMemo<readonly Command[]>(
    () =>
      paletteEntries({ identity, pathname, vault, vaults, objects: objects.data ?? [] }).map((entry) => ({
        id: entry.id,
        label: entry.label,
        hint: entry.hint,
        run: () => {
          if (entry.action.kind === "sign_out") onSignOut();
          else void navigate({ to: entry.action.to });
        },
      })),
    [identity, navigate, objects.data, onSignOut, pathname, vault, vaults],
  );

  return <CommandPalette open={open} onOpenChange={onOpenChange} commands={commands} />;
};
