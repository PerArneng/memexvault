import { nameProblem } from "@memex/core";
import type { Vault } from "@memex/types";
import { Button, Input } from "@memex/ui";
import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { useEngine } from "../engine-context";
import { QUERY_KEYS } from "../query";
import { Callout, failureText } from "../wiki/wiki-common";

/** A server admin's. The home page has it too, so a new server's first vault needs no command line. */
export const CreateVaultForm = ({ onCreated }: { readonly onCreated: (vault: Vault) => void }) => {
  const engine = useEngine();
  const queryClient = useQueryClient();
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const create = async () => {
    const invalid = nameProblem(name);
    if (invalid !== null) {
      setProblem(invalid);
      return;
    }
    setBusy(true);
    setProblem(null);
    const result = await engine.createVault(name);
    setBusy(false);
    if (!result.ok) {
      setProblem(failureText(result.failure));
      return;
    }
    setName("");
    await queryClient.invalidateQueries({ queryKey: QUERY_KEYS.vaults });
    onCreated(result.value);
  };
  return (
    <form
      aria-label="Create a vault"
      className="flex flex-col gap-2"
      onSubmit={(e) => {
        e.preventDefault();
        void create();
      }}
    >
      <div className="flex items-end gap-2">
        <Input label="New vault's name" value={name} onChange={(e) => setName(e.target.value)} />
        <Button type="submit" variant="primary" disabled={busy}>Create vault</Button>
      </div>
      {problem !== null && <Callout tone="danger" title={problem} />}
    </form>
  );
};
