import { nameProblem } from "@memex/core";
import type { ChatSettings, Vault, VaultId } from "@memex/types";
import { Button, Input } from "@memex/ui";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useRouter } from "@tanstack/react-router";
import { useState } from "react";
import { useEngine } from "../engine-context";
import { QUERY_KEYS, unwrap } from "../query";
import { QueryView } from "../shell/query-view";
import { Callout, ConfirmPanel, failureText, formatWhen, KeyValues, Panel } from "./admin-common";

const RenameForm = ({ vault }: { readonly vault: Vault }) => {
  const engine = useEngine();
  const queryClient = useQueryClient();
  const [name, setName] = useState(vault.name);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<{ readonly tone: "info" | "danger"; readonly text: string } | null>(null);
  const save = async () => {
    const invalid = nameProblem(name);
    if (invalid !== null) {
      setMessage({ tone: "danger", text: invalid });
      return;
    }
    setBusy(true);
    const result = await engine.renameVault(vault.id, name);
    setBusy(false);
    if (!result.ok) {
      setMessage({ tone: "danger", text: failureText(result.failure) });
      return;
    }
    setMessage({ tone: "info", text: `Renamed to ${result.value.name}.` });
    await queryClient.invalidateQueries({ queryKey: QUERY_KEYS.vaults });
    await queryClient.invalidateQueries({ queryKey: QUERY_KEYS.vault(vault.id) });
  };
  return (
    <form
      aria-label="Rename the vault"
      className="flex flex-col gap-2"
      onSubmit={(e) => {
        e.preventDefault();
        void save();
      }}
    >
      <div className="flex items-end gap-2">
        <Input label="Vault name" value={name} onChange={(e) => setName(e.target.value)} />
        <Button type="submit" variant="primary" disabled={busy || name.trim() === vault.name}>Rename</Button>
      </div>
      {message !== null && <Callout tone={message.tone} title={message.text} />}
    </form>
  );
};

const positive = (text: string): number | null | "bad" => {
  if (text.trim() === "") return null;
  const n = Number(text);
  return Number.isInteger(n) && n > 0 ? n : "bad";
};

const ChatSettingsPanel = ({ vault, settings }: { readonly vault: VaultId; readonly settings: ChatSettings }) => {
  const engine = useEngine();
  const queryClient = useQueryClient();
  const [confirming, setConfirming] = useState<boolean | null>(null);
  const [thread, setThread] = useState(settings.threadTokenBudget === null ? "" : String(settings.threadTokenBudget));
  const [day, setDay] = useState(settings.dayTokenBudget === null ? "" : String(settings.dayTokenBudget));
  const threadBudget = positive(thread);
  const dayBudget = positive(day);
  const budgetsValid = threadBudget !== "bad" && dayBudget !== "bad";
  const apply = async (enabled: boolean) => {
    if (threadBudget === "bad" || dayBudget === "bad") return null;
    const result = await engine.putChatSettings(vault, { enabled, threadTokenBudget: threadBudget, dayTokenBudget: dayBudget });
    if (!result.ok) return result.failure;
    await queryClient.invalidateQueries({ queryKey: QUERY_KEYS.chat(vault) });
    return null;
  };
  return (
    <div className="flex flex-col gap-3">
      <KeyValues
        label="Where vault content goes"
        rows={[
          ["Chat", settings.enabled ? "On" : "Off"],
          ["Model", settings.model ?? "This server has no chat model, so chat can't be turned on."],
          ["Endpoint", settings.endpoint ?? "None"],
          ["Today's tokens", `${String(settings.dayTokensUsed)} of ${String(settings.dayTokensLimit)}`],
        ]}
      />
      <div className="flex flex-wrap items-end gap-2">
        <Input label="Tokens per thread (blank for the server's default)" inputMode="numeric" value={thread} onChange={(e) => setThread(e.target.value)} {...(threadBudget === "bad" ? { error: "A whole number above 0, or blank." } : {})} />
        <Input label="Tokens per day (blank for the server's default)" inputMode="numeric" value={day} onChange={(e) => setDay(e.target.value)} {...(dayBudget === "bad" ? { error: "A whole number above 0, or blank." } : {})} />
      </div>
      {confirming === null ? (
        <div className="flex gap-2">
          {settings.enabled ? (
            <>
              <Button onClick={() => setConfirming(true)} disabled={!budgetsValid}>Save budgets</Button>
              <Button variant="danger" onClick={() => setConfirming(false)}>Turn chat off</Button>
            </>
          ) : (
            <Button variant="primary" disabled={settings.model === null || !budgetsValid} onClick={() => setConfirming(true)}>Turn chat on…</Button>
          )}
        </div>
      ) : (
        <ConfirmPanel
          title={confirming ? (settings.enabled ? "Save the chat budgets" : "Turn chat on for this vault") : "Turn chat off for this vault"}
          danger={!confirming}
          description={
            confirming && !settings.enabled ? (
              <>Every chat message then sends passages of this vault's content to <strong>{settings.model ?? "the model"}</strong> at <strong>{settings.endpoint ?? "the configured endpoint"}</strong>. Anyone with read access to the vault can chat.</>
            ) : confirming ? (
              "The new budgets apply from the next message."
            ) : (
              "Nobody can chat in this vault until an admin turns it on again. Threads are kept."
            )
          }
          confirmLabel={confirming ? (settings.enabled ? "Save" : "Turn chat on") : "Turn chat off"}
          run={() => apply(confirming)}
          onDone={() => setConfirming(null)}
          onCancel={() => setConfirming(null)}
        />
      )}
    </div>
  );
};

const DeletePanel = ({ vault }: { readonly vault: Vault }) => {
  const engine = useEngine();
  const queryClient = useQueryClient();
  const router = useRouter();
  const [open, setOpen] = useState(false);
  return open ? (
    <ConfirmPanel
      title={`Delete the vault ${vault.name}`}
      description="This removes the vault and everything in it: documents, pages, entities, claims, its history and its files. It can't be undone, except from a backup or an export."
      typed={{ expected: vault.name, label: `Type the vault's name, ${vault.name}, to confirm` }}
      confirmLabel="Delete this vault"
      run={async () => {
        const result = await engine.deleteVault(vault.id);
        if (!result.ok) return result.failure;
        // The list has to be read again first, or the home page would send the person back to the vault that's gone.
        await queryClient.invalidateQueries({ queryKey: QUERY_KEYS.vaults });
        queryClient.removeQueries({ queryKey: QUERY_KEYS.vault(vault.id) });
        router.history.push("/");
        return null;
      }}
      onDone={() => undefined}
      onCancel={() => setOpen(false)}
    />
  ) : (
    <div><Button variant="danger" onClick={() => setOpen(true)}>Delete vault…</Button></div>
  );
};

export const VaultTab = ({ vault }: { readonly vault: VaultId }) => {
  const engine = useEngine();
  const vaultQuery = useQuery({ queryKey: QUERY_KEYS.vault(vault), queryFn: async () => unwrap(await engine.getVault(vault)) });
  const chatQuery = useQuery({ queryKey: QUERY_KEYS.chatSettings(vault), queryFn: async () => unwrap(await engine.getChatSettings(vault)) });
  return (
    <QueryView query={vaultQuery} label="this vault">
      {(data) => (
        <div className="flex flex-col gap-4">
          <Panel title="Vault">
            <KeyValues label="This vault" rows={[["Id", <span className="font-mono">{data.id}</span>], ["Created", formatWhen(data.createdAt)]]} />
            <RenameForm key={data.id} vault={data} />
          </Panel>
          <Panel title="Chat" description="Chat is off in every vault until an admin turns it on, as it sends vault content to the model.">
            <QueryView query={chatQuery} label="the chat settings">
              {(settings) => <ChatSettingsPanel key={`${String(settings.enabled)}-${String(settings.threadTokenBudget)}-${String(settings.dayTokenBudget)}`} vault={vault} settings={settings} />}
            </QueryView>
          </Panel>
          <Panel title="Delete">
            <DeletePanel vault={data} />
          </Panel>
        </div>
      )}
    </QueryView>
  );
};
