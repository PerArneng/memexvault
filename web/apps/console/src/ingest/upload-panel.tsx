import { formatBytes, isRecordingFile, sectionPath, TRUST_TIERS } from "@memex/core";
import type { UploadItem, UploadQueue } from "@memex/core";
import type { UploadLimits, VaultId } from "@memex/types";
import { Badge, Button, Input } from "@memex/ui";
import type { Tone } from "@memex/ui";
import { useQuery } from "@tanstack/react-query";
import { useRef, useState, useSyncExternalStore } from "react";
import type { DragEvent } from "react";
import { useEngine } from "../engine-context";
import { QUERY_KEYS, unwrap } from "../query";
import { QueryView } from "../shell/query-view";
import { TierOf } from "../viewer/badges";
import { AppLink } from "../wiki/wiki-common";
import { ProgressBar } from "./ingest-common";

const STATE_BADGE: Readonly<Record<UploadItem["state"]["kind"], { readonly tone: Tone; readonly label: string }>> = {
  refused: { tone: "danger", label: "Not sent" },
  queued: { tone: "neutral", label: "Queued" },
  uploading: { tone: "info", label: "Uploading" },
  stored: { tone: "success", label: "Stored" },
  duplicate: { tone: "warning", label: "Already here" },
  failed: { tone: "danger", label: "Failed" },
  cancelled: { tone: "neutral", label: "Cancelled" },
};

const ItemRow = ({ vault, item, queue }: { readonly vault: VaultId; readonly item: UploadItem; readonly queue: UploadQueue }) => {
  const { state } = item;
  const shown = STATE_BADGE[state.kind];
  const documentLink = (id: string) => sectionPath(vault, "ingest", id);
  return (
    <li data-upload-name={item.name} data-upload-state={state.kind} className="flex flex-col gap-1 rounded-md border border-border p-2">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span className="min-w-0 truncate text-sm" title={item.name}>{item.name}</span>
        <span className="flex items-center gap-2">
          <span className="font-mono text-xs text-text-muted tabular-nums">{formatBytes(item.size)}</span>
          <TierOf tier={item.options.trustTier} />
          <Badge tone={shown.tone}>{shown.label}</Badge>
          {(state.kind === "queued" || state.kind === "uploading") && <Button variant="ghost" aria-label={`Cancel uploading ${item.name}`} onClick={() => queue.cancel(item.id)}>Cancel</Button>}
          {(state.kind === "failed" || state.kind === "cancelled") && <Button aria-label={`Retry uploading ${item.name}`} onClick={() => queue.retry(item.id)}>Retry</Button>}
          {state.kind !== "queued" && state.kind !== "uploading" && <Button variant="ghost" aria-label={`Remove ${item.name} from this list`} onClick={() => queue.remove(item.id)}>Remove</Button>}
        </span>
      </div>
      {state.kind === "uploading" && <ProgressBar label={`Uploading ${item.name}`} value={state.sent} max={item.size} />}
      {state.kind === "refused" && <p className="text-xs text-danger">{state.reason}</p>}
      {state.kind === "failed" && <p className="text-xs text-danger">{state.reason}</p>}
      {state.kind === "stored" && <p className="text-xs text-text-muted"><AppLink to={documentLink(state.source.id)}>Follow it through the pipeline</AppLink></p>}
      {state.kind === "duplicate" && (
        <p className="text-xs text-text-muted">
          These exact bytes are already in this vault as <span className="text-text">{state.source.name}</span>, so nothing new was stored and nothing will be processed again. <AppLink to={documentLink(state.source.id)}>See that document</AppLink>
        </p>
      )}
    </li>
  );
};

const Panel = ({ vault, limits }: { readonly vault: VaultId; readonly limits: UploadLimits }) => {
  const engine = useEngine();
  const input = useRef<HTMLInputElement>(null);
  const queue = engine.uploadQueue(vault, limits);
  const [tier, setTier] = useState("unverified");
  const [language, setLanguage] = useState("");
  const [over, setOver] = useState(false);
  const { items } = useSyncExternalStore(queue.subscribe, queue.state);

  const add = (files: readonly File[]) => {
    const trimmed = language.trim();
    const hasRecording = files.some(isRecordingFile);
    queue.add(files, { trustTier: tier, language: hasRecording && trimmed !== "" ? trimmed : null });
  };
  const onDrop = (e: DragEvent) => {
    e.preventDefault();
    setOver(false);
    add([...e.dataTransfer.files]);
  };

  return (
    <section aria-label="Upload" className="flex flex-col gap-3 rounded-lg border border-border bg-surface-raised p-3">
      <h2 className="font-medium">Upload</h2>
      <div
        data-dropzone={true}
        onDragOver={(e) => {
          e.preventDefault();
          setOver(true);
        }}
        onDragLeave={() => setOver(false)}
        onDrop={onDrop}
        className={`flex flex-wrap items-center justify-center gap-2 rounded-md border border-dashed p-4 text-sm text-text-muted ${over ? "border-accent bg-surface-selected" : "border-border-strong"}`}
      >
        <span>Drop documents or recordings here, or</span>
        <Button onClick={() => input.current?.click()}>Choose files</Button>
        <input
          ref={input}
          type="file"
          multiple
          aria-label="Files to upload"
          className="sr-only"
          tabIndex={-1}
          onChange={(e) => {
            add([...(e.target.files ?? [])]);
            e.target.value = "";
          }}
        />
      </div>
      <p className="text-xs text-text-muted">
        Up to <strong>{formatBytes(limits.maxBytes)}</strong> each. Takes {limits.allowedMediaTypes.join(", ")}. Uploading the same bytes again stores nothing new.
      </p>
      <div className="flex flex-wrap items-end gap-6">
        <fieldset className="flex flex-col gap-1">
          <legend className="text-xs text-text-muted">Trust tier for these files</legend>
          <div className="flex flex-wrap gap-3">
            {TRUST_TIERS.map((option) => (
              <label key={option} className="inline-flex items-center gap-1.5">
                <input type="radio" name="upload-trust-tier" value={option} checked={tier === option} onChange={() => setTier(option)} />
                <TierOf tier={option} />
              </label>
            ))}
          </div>
        </fieldset>
        <Input label="Language of recordings, if known (such as sv)" value={language} maxLength={16} onChange={(e) => setLanguage(e.target.value)} />
      </div>
      {items.length > 0 && <ul aria-label="Upload queue" className="flex flex-col gap-2">{items.map((item) => <ItemRow key={item.id} vault={vault} item={item} queue={queue} />)}</ul>}
    </section>
  );
};

export const UploadPanel = ({ vault }: { readonly vault: VaultId }) => {
  const engine = useEngine();
  const limits = useQuery({ queryKey: QUERY_KEYS.uploadLimits(vault), queryFn: async () => unwrap(await engine.getUploadLimits(vault)) });
  return <QueryView query={limits} label="the upload limits">{(data) => <Panel vault={vault} limits={data} />}</QueryView>;
};
