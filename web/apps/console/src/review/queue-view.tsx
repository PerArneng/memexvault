import { afterRemoving, decisionAccess, decisionAccessText, defaultApproveReason, describeOp, documentPath, INITIAL_QUEUE, NO_FILTER, opsAsText, parseEditedOps, QUEUE_KEY_HELP, queueCommandFor, queueReconciled, queueStep, reviewPath, selectedIn, sourcePlace, summarizeBatch, VERDICT_FILTERS } from "@memex/core";
import type { DecisionOutcome, QueueCommand, QueueFeedEvent, QueueRefusal, QueueState } from "@memex/core";
import type { Identity, Page, ReviewDecision, ReviewDetail, ReviewFilter, ReviewItem, VaultId } from "@memex/types";
import { Badge, Button, EmptyState, Input } from "@memex/ui";
import { useInfiniteQuery, useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { sourceRoute } from "../documents/document-common";
import { useEngine } from "../engine-context";
import { QUERY_KEYS, unwrap } from "../query";
import { QueryView } from "../shell/query-view";
import { AppLink, Callout, useGo } from "../wiki/wiki-common";
import { ItemView } from "./item-view";
import { outcomeNotice, ReasonForm, useRefreshReview, WhoBadge } from "./review-common";
import { SourcePane } from "./source-pane";

interface Notice {
  readonly tone: "info" | "warning" | "danger";
  readonly title: string;
  readonly detail: string;
}

const REFUSALS: Readonly<Record<Exclude<QueueRefusal, null>, string>> = {
  cant_decide: "You can read this queue, but not decide in it.",
  nothing_selected: "Nothing is selected. Press x to select an item.",
  nothing_here: "There's nothing here to act on.",
  not_loaded: "This item is still loading, so nothing was decided.",
};

type LiveState = "connecting" | "live" | "away" | "off";

/** Reading again costs a request, and a burst of events can't block input, so events close together become one read. */
const COALESCE_MS = 400;
/** With no stream to say otherwise, the counts are read this often instead. */
const POLL_WITHOUT_STREAM_MS = 30_000;

const useLiveQueue = (vault: VaultId): LiveState => {
  const engine = useEngine();
  const refresh = useRefreshReview(vault);
  const [live, setLive] = useState<LiveState>("connecting");
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | null = null;
    const soon = () => {
      timer ??= setTimeout(() => {
        timer = null;
        void refresh();
      }, COALESCE_MS);
    };
    const stop = engine.followQueue(vault, (event: QueueFeedEvent) => {
      if (event.kind === "open") setLive("live");
      if (event.kind === "away") setLive("away");
      if (event.kind === "closed") setLive("off");
      if (event.kind === "open" || event.kind === "changed" || event.kind === "resync") soon();
    });
    return () => {
      if (timer !== null) clearTimeout(timer);
      stop();
    };
  }, [engine, refresh, vault]);
  return live;
};

const LiveBadge = ({ live }: { readonly live: LiveState }) => {
  if (live === "live") return <Badge tone="success">Live</Badge>;
  if (live === "off") return <Badge tone="warning">Not live: read every 30 s</Badge>;
  return <Badge>{live === "away" ? "Reconnecting" : "Connecting"}</Badge>;
};

const Counts = ({ vault, filter, live, onFilter }: { readonly vault: VaultId; readonly filter: ReviewFilter; readonly live: LiveState; readonly onFilter: (next: ReviewFilter) => void }) => {
  const engine = useEngine();
  const query = useQuery({
    queryKey: QUERY_KEYS.reviewCounts(vault),
    queryFn: async () => unwrap(await engine.getQueueCounts(vault)),
    refetchInterval: live === "live" ? false : POLL_WITHOUT_STREAM_MS,
  });
  return (
    <div className="flex flex-col gap-2">
      <p className="flex items-center gap-2 text-sm">
        <span data-testid="pending-count" className="text-lg font-medium tabular-nums">{query.data?.pending ?? "–"}</span> waiting for a decision
        <span data-testid="live-state"><LiveBadge live={live} /></span>
      </p>
      {query.data !== undefined && query.data.groups.length > 0 && (
        <ul aria-label="Counts by kind" className="flex flex-wrap gap-1 text-xs">
          {query.data.groups.map((g) => (
            <li key={`${g.extractor}-${g.promptVersion}-${g.itemType}-${String(g.verdict)}`}>
              <button
                type="button"
                data-testid="count-group"
                className="rounded-sm border border-border px-1.5 py-0.5 hover:bg-surface-selected"
                onClick={() => onFilter({ ...filter, extractor: g.extractor, promptVersion: g.promptVersion, itemType: g.itemType, verdict: g.verdict ?? "none" })}
              >
                {g.itemType} from {g.extractor}{g.verdict === null ? "" : `, reviewer said ${g.verdict}`}: <span className="tabular-nums">{g.count}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
};

const Filters = ({ filter, onFilter }: { readonly filter: ReviewFilter; readonly onFilter: (next: ReviewFilter) => void }) => {
  const [proposer, setProposer] = useState(filter.proposer);
  const active = Object.values(filter).some((v) => v !== "");
  return (
    <form
      className="flex flex-wrap items-end gap-2"
      aria-label="Filter the queue"
      onSubmit={(e) => {
        e.preventDefault();
        onFilter({ ...filter, proposer: proposer.trim() });
      }}
    >
      <Input label="Proposer" value={proposer} onChange={(e) => setProposer(e.target.value)} className="w-40" />
      <div className="flex flex-col gap-1">
        <label htmlFor="review-verdict" className="text-xs text-text-muted">Reviewer agent</label>
        <select id="review-verdict" value={filter.verdict} onChange={(e) => onFilter({ ...filter, verdict: e.target.value })} className="h-8 rounded-md border border-border-strong bg-surface-raised px-1 text-sm text-text">
          {VERDICT_FILTERS.map((v) => <option key={v.id} value={v.id}>{v.label}</option>)}
        </select>
      </div>
      <Button type="submit">Apply</Button>
      {active && <Button onClick={() => { setProposer(""); onFilter(NO_FILTER); }}>Clear filters</Button>}
      {filter.itemType !== "" && <Badge>Type: {filter.itemType}</Badge>}
      {filter.extractor !== "" && <Badge>From: {filter.extractor}</Badge>}
    </form>
  );
};

const Row = ({ item, current, selected }: { readonly item: ReviewItem; readonly current: boolean; readonly selected: boolean }) => {
  const { changeset } = item;
  const first = describeOp(changeset.ops[0]);
  const ref = useRef<HTMLLIElement>(null);
  useEffect(() => {
    if (current) ref.current?.scrollIntoView({ block: "nearest" });
  }, [current]);
  return (
    <li
      ref={ref}
      id={`queue-${changeset.id}`}
      role="option"
      aria-selected={current}
      data-queue-row={changeset.id}
      data-checked={selected}
      className={`flex flex-col gap-0.5 border-b border-border p-2 text-sm ${current ? "bg-surface-selected outline-2 -outline-offset-2 outline-accent" : ""}`}
    >
      <span className="flex items-center gap-1.5">
        {selected && <Badge tone="info">Selected</Badge>}
        <span className="min-w-0 truncate font-medium">{first.title}{changeset.ops.length > 1 ? ` and ${String(changeset.ops.length - 1)} more` : ""}</span>
      </span>
      <span className="truncate text-xs text-text-muted">{first.fields[0]?.value ?? ""}</span>
      <span className="flex items-center gap-1.5 text-xs">
        <WhoBadge machine={changeset.proposerIsMachine} />
        <span className="min-w-0 truncate text-text-muted">{changeset.proposer}</span>
        {item.verdict !== null && <Badge tone={item.verdict.verdict === "approve" ? "success" : item.verdict.verdict === "reject" ? "danger" : "neutral"}>{item.verdict.verdict}</Badge>}
      </span>
    </li>
  );
};

const EditForm = ({ detail, problem, busy, onSubmit, onCancel }: { readonly detail: ReviewDetail; readonly problem: string | null; readonly busy: boolean; readonly onSubmit: (ops: readonly unknown[], reason: string) => void; readonly onCancel: () => void }) => {
  const [text, setText] = useState(() => opsAsText(detail.changeset.ops));
  const [reason, setReason] = useState("");
  const [local, setLocal] = useState<string | null>(null);
  const send = () => {
    if (reason.trim() === "") {
      setLocal("This needs a reason, so it's on record why it changed.");
      return;
    }
    const parsed = parseEditedOps(text);
    if (!parsed.ok) {
      setLocal(parsed.message);
      return;
    }
    setLocal(null);
    if (!busy) onSubmit(parsed.ops, reason.trim());
  };
  const shown = local ?? problem;
  return (
    <div
      className="flex flex-col gap-2 rounded-lg border border-border-strong bg-surface-raised p-3"
      data-testid="edit-form"
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          e.preventDefault();
          e.stopPropagation();
          onCancel();
        }
        if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
          e.preventDefault();
          send();
        }
      }}
    >
      <p className="text-sm text-text-muted">What was proposed stays on record, and this counts against the extractor. Only a person can do this.</p>
      <label htmlFor="edit-ops" className="text-sm">The ops, as JSON</label>
      <textarea id="edit-ops" autoFocus={true} rows={12} spellCheck={false} value={text} onChange={(e) => setText(e.target.value)} className="rounded-md border border-border-strong bg-surface px-2 py-1 font-mono text-xs text-text" />
      <label htmlFor="edit-reason" className="text-sm">Why it changed</label>
      <input id="edit-reason" value={reason} onChange={(e) => setReason(e.target.value)} className="h-8 rounded-md border border-border-strong bg-surface px-2 text-sm text-text" />
      {shown !== null && <p role="alert" className="text-sm text-danger">{shown}</p>}
      <div className="flex items-center gap-2">
        <Button variant="primary" disabled={busy} onClick={send}>Approve the edit</Button>
        <Button onClick={onCancel}>Cancel</Button>
        <span className="text-xs text-text-muted">Ctrl+Enter to send, Esc to cancel</span>
      </div>
    </div>
  );
};

const KeyHelp = () => (
  <div data-testid="key-help" className="rounded-lg border border-border-strong bg-surface-raised p-3">
    <h2 className="mb-2 text-sm font-medium">Keys</h2>
    <dl className="grid grid-cols-[6rem_minmax(0,1fr)] gap-x-3 gap-y-0.5 text-sm">
      {QUEUE_KEY_HELP.map((k) => (
        <div key={k.keys} className="contents"><dt className="font-mono text-xs text-text-muted">{k.keys}</dt><dd>{k.what}</dd></div>
      ))}
    </dl>
  </div>
);

const isTyping = (target: EventTarget | null, key: string): boolean => {
  if (!(target instanceof Element)) return false;
  if (target.closest("input, textarea, select, [contenteditable=true], [role=dialog], [role=combobox]") !== null) return true;
  // Arrow keys in the document scroll the document.
  return key.startsWith("Arrow") && target.closest("[data-testid=source-pane]") !== null;
};

export const QueueView = ({ identity, vault, filter, item }: { readonly identity: Identity; readonly vault: VaultId; readonly filter: ReviewFilter; readonly item: string | null }) => {
  const engine = useEngine();
  const go = useGo();
  const queryClient = useQueryClient();
  const refresh = useRefreshReview(vault);
  const live = useLiveQueue(vault);
  const access = decisionAccess(identity, vault);
  const canDecide = access.kind === "allowed";
  const listRef = useRef<HTMLUListElement>(null);
  const [state, setState] = useState<QueueState>({ ...INITIAL_QUEUE, current: item });
  const [notice, setNotice] = useState<Notice | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // Decided here and on the way out, so the next key acts on the next item without waiting for the server.
  const [leaving, setLeaving] = useState<readonly string[]>([]);

  const list = useInfiniteQuery({
    queryKey: QUERY_KEYS.reviewQueue(vault, filter),
    initialPageParam: null as string | null,
    getNextPageParam: (last: Page<ReviewItem>) => last.nextCursor,
    queryFn: async ({ pageParam }) => unwrap(await engine.listReviewQueue(vault, filter, pageParam)),
  });
  const rows = useMemo(() => (list.data?.pages.flatMap((p) => p.items) ?? []).filter((r) => !leaving.includes(r.changeset.id)), [leaving, list.data]);
  const listIds = useMemo(() => rows.map((r) => r.changeset.id), [rows]);
  // A link can name an item that isn't in this list, such as one already decided or beyond the first page, so it's held apart from the list until someone moves on.
  const [pinned, setPinned] = useState<string | null>(item);
  const ids = useMemo(() => (pinned !== null && !listIds.includes(pinned) ? [pinned, ...listIds] : listIds), [listIds, pinned]);

  const previous = useRef<readonly string[]>([]);
  useEffect(() => {
    if (list.data === undefined) return;
    const before = previous.current;
    previous.current = ids;
    setState((s) => queueReconciled(s, before, ids));
  }, [ids, list.data]);

  const current = state.current;
  const detailQuery = useQuery({
    queryKey: QUERY_KEYS.reviewItem(vault, current ?? "none"),
    enabled: current !== null,
    staleTime: 5_000,
    queryFn: async () => unwrap(await engine.getReviewItem(vault, current ?? "")),
  });
  const detail = detailQuery.data !== undefined && detailQuery.data.changeset.id === current ? detailQuery.data : null;
  const pending = detail?.changeset.state === "pending";

  // Reading the next item before it's needed, as a reviewer goes through them in order.
  const at = current === null ? -1 : ids.indexOf(current);
  const upNext = at === -1 ? undefined : ids[at + 1];
  useEffect(() => {
    if (upNext !== undefined) void queryClient.prefetchQuery({ queryKey: QUERY_KEYS.reviewItem(vault, upNext), staleTime: 5_000, queryFn: async () => unwrap(await engine.getReviewItem(vault, upNext)) });
  }, [engine, queryClient, upNext, vault]);

  // The URL holds the place, so a reviewer's view can be linked to and reloaded.
  const path = reviewPath(vault, { kind: "queue", filter, item: current });
  const loaded = list.data !== undefined;
  useEffect(() => {
    if (loaded) go(path, true);
  }, [go, loaded, path]);

  const focusList = useCallback(() => {
    listRef.current?.focus();
  }, []);

  const closePanel = useCallback(() => {
    setProblem(null);
    setState((s) => ({ ...s, panel: { kind: "none" } }));
    focusList();
  }, [focusList]);

  const moveOn = useCallback((ids_: readonly string[], gone: readonly string[]) => {
    setPinned((p) => (p !== null && gone.includes(p) ? null : p));
    setLeaving((l) => [...l, ...gone]);
    setState((s) => {
      const next = afterRemoving(ids_, gone, s.current);
      return { ...s, current: next, evidence: 0, selected: s.selected.filter((id) => !gone.includes(id)), panel: { kind: "none" } };
    });
    focusList();
  }, [focusList]);

  const settle = useCallback(async (gone: readonly string[]) => {
    await refresh();
    setLeaving((l) => l.filter((id) => !gone.includes(id)));
  }, [refresh]);

  const report = useCallback((outcome: DecisionOutcome, decision: ReviewDecision, what: string) => {
    if (outcome.kind === "done") {
      setNotice({ tone: "info", title: `${what}.`, detail: `Recorded as ${outcome.changeset.decidedBy ?? "you"}${outcome.changeset.state === "rejected" && decision.kind !== "reject" ? ", but it was rejected" : ""}.` });
    } else {
      setNotice({ tone: "danger", ...outcomeNotice(outcome) });
    }
  }, []);

  /** A plain approval. The queue moves on at once, and the item comes back if it didn't go through. */
  const approveNow = useCallback(async (decision: ReviewDecision) => {
    setNotice(null);
    moveOn(ids, [decision.id]);
    report(await engine.decide(vault, decision), decision, "Approved");
    await settle([decision.id]);
  }, [engine, ids, moveOn, report, settle, vault]);

  /** A decision someone typed something for. The form stays until the server has answered, so a refusal costs them none of it. */
  const decideFromForm = useCallback(async (decision: ReviewDecision, what: string) => {
    setNotice(null);
    setProblem(null);
    setBusy(true);
    const outcome = await engine.decide(vault, decision);
    setBusy(false);
    if (outcome.kind === "invalid") {
      setProblem(outcome.message);
      return;
    }
    if (outcome.kind === "failed") {
      setProblem(outcomeNotice(outcome).detail);
      return;
    }
    moveOn(ids, [decision.id]);
    report(outcome, decision, what);
    await settle([decision.id]);
  }, [engine, ids, moveOn, report, settle, vault]);

  const decideBatch = useCallback(async (kind: "approve" | "reject", reason: string) => {
    const chosen = selectedIn(state, ids);
    setBusy(true);
    setNotice(null);
    const decisions: readonly ReviewDecision[] = chosen.map((id) => (kind === "approve" ? { kind: "approve", id, reason } : { kind: "reject", id, reason }));
    const result = await engine.decideBatch(vault, decisions);
    setBusy(false);
    if (!result.ok && result.failure.kind === "unreachable") {
      setProblem(outcomeNotice({ kind: "failed", failure: result.failure }).detail);
      return;
    }
    moveOn(ids, chosen);
    if (result.ok) {
      const summary = summarizeBatch(result.value);
      setNotice(summary.problems.length === 0
        ? { tone: "info", title: `${kind === "approve" ? "Approved" : "Rejected"} ${String(summary.done)} items.`, detail: "Each is recorded as its own decision." }
        : { tone: "danger", title: `${String(summary.done)} went through and ${String(summary.problems.length)} didn't.`, detail: summary.problems.map((p) => `${p.id}: ${p.message}`).join(" ") });
    } else {
      setNotice({ tone: "danger", ...outcomeNotice({ kind: "failed", failure: result.failure }) });
    }
    await settle(chosen);
  }, [engine, ids, moveOn, settle, state, vault]);

  const run = useCallback((command: QueueCommand) => {
    const view = { ids, canDecide, ready: detail !== null && pending, evidenceCount: detail?.evidence.length ?? 0 };
    const out = queueStep(state, command, view);
    if (out.refused !== null) setNotice({ tone: "warning", title: REFUSALS[out.refused], detail: decisionAccessText(access) ?? "" });
    setState(out.state);
    if (out.approve && current !== null) void approveNow({ kind: "approve", id: current, reason: defaultApproveReason });
    if (out.openSource && detail !== null) {
      const evidence = detail.evidence[state.evidence] ?? detail.evidence[0];
      if (evidence?.sourceId != null) {
        const place = sourcePlace(evidence);
        go(documentPath(vault, sourceRoute(evidence.sourceId, place.kind === "pages" ? { page: place.page } : {})));
      }
    }
    if (out.state.panel.kind === "none" && state.panel.kind !== "none") focusList();
  }, [access, approveNow, canDecide, current, detail, focusList, go, ids, pending, state, vault]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const command = queueCommandFor({ key: e.key, ctrl: e.ctrlKey, meta: e.metaKey, alt: e.altKey, typing: isTyping(e.target, e.key) });
      if (command === null) return;
      e.preventDefault();
      run(command);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [run]);

  useEffect(() => {
    focusList();
  }, [focusList]);

  const onFilter = (next: ReviewFilter) => {
    setPinned(null);
    previous.current = [];
    setState({ ...INITIAL_QUEUE });
    go(reviewPath(vault, { kind: "queue", filter: next, item: null }));
  };

  const chosen = selectedIn(state, ids);
  const evidence = detail?.evidence[Math.min(state.evidence, Math.max(0, detail.evidence.length - 1))] ?? null;
  const accessText = decisionAccessText(access);

  return (
    <div className="flex flex-col gap-3">
      <Counts vault={vault} filter={filter} live={live} onFilter={onFilter} />
      <Filters key={filter.proposer} filter={filter} onFilter={onFilter} />
      {accessText !== null && <div data-testid="no-decisions"><Callout tone="warning" title="You can't decide anything here.">{accessText}</Callout></div>}
      {notice !== null && <div data-testid="queue-notice" data-tone={notice.tone}><Callout tone={notice.tone} title={notice.title}>{notice.detail}</Callout></div>}
      <QueryView query={list} label="the review queue">
        {() => (ids.length === 0 ? (
          <EmptyState title={Object.values(filter).some((v) => v !== "") ? "Nothing waiting matches these filters." : "Nothing is waiting for a decision."}>New proposals appear here by themselves.</EmptyState>
        ) : (
          <div className="grid grid-cols-[16rem_minmax(0,1fr)] items-start gap-3 2xl:grid-cols-[16rem_minmax(0,1fr)_minmax(0,1fr)]">
            <div className="flex max-h-[calc(100vh-14rem)] flex-col overflow-hidden rounded-md border border-border">
              <ul
                ref={listRef}
                role="listbox"
                aria-label="Items waiting for a decision"
                aria-activedescendant={current === null ? undefined : `queue-${current}`}
                aria-multiselectable={true}
                tabIndex={0}
                data-testid="queue-list"
                className="min-h-0 flex-1 overflow-auto focus:outline-2 focus:outline-accent"
              >
                {rows.map((r) => <Row key={r.changeset.id} item={r} current={r.changeset.id === current} selected={state.selected.includes(r.changeset.id)} />)}
              </ul>
              {list.hasNextPage && <Button className="m-1" disabled={list.isFetchingNextPage} onClick={() => void list.fetchNextPage()}>Load more</Button>}
              <p className="border-t border-border p-1 text-xs text-text-muted">Press ? for keys.{chosen.length > 0 ? ` ${String(chosen.length)} selected.` : ""}</p>
            </div>
            <div className="flex min-w-0 flex-col gap-3">
              {state.panel.kind === "help" && <KeyHelp />}
              {pinned !== null && !listIds.includes(pinned) && pinned === current && detail !== null && (
                <p className="text-sm text-text-muted">This item isn't in the list as filtered. <AppLink to={reviewPath(vault, { kind: "queue", filter, item: null })}>Back to the queue</AppLink></p>
              )}
              {state.panel.kind === "approve_note" && current !== null && (
                <ReasonForm label="A note on this approval" confirmLabel="Approve" required={false} initial={defaultApproveReason} busy={busy} problem={problem} onCancel={closePanel} onSubmit={(reason) => void decideFromForm({ kind: "approve", id: current, reason: reason === "" ? defaultApproveReason : reason }, "Approved")} />
              )}
              {state.panel.kind === "reject" && current !== null && (
                <ReasonForm label="Why this is rejected" confirmLabel="Reject" required={true} danger={true} busy={busy} problem={problem} onCancel={closePanel} onSubmit={(reason) => void decideFromForm({ kind: "reject", id: current, reason }, "Rejected")} />
              )}
              {state.panel.kind === "edit" && detail !== null && (
                <EditForm key={detail.changeset.id} detail={detail} problem={problem} busy={busy} onCancel={closePanel} onSubmit={(ops, reason) => void decideFromForm({ kind: "edit_approve", id: detail.changeset.id, ops, reason }, "Approved with your edit")} />
              )}
              {state.panel.kind === "batch_approve" && (
                <ReasonForm label={`Approve ${String(chosen.length)} selected items. A note, recorded on each`} confirmLabel={`Approve ${String(chosen.length)}`} required={false} initial={defaultApproveReason} busy={busy} problem={problem} onCancel={closePanel} onSubmit={(reason) => void decideBatch("approve", reason === "" ? defaultApproveReason : reason)} />
              )}
              {state.panel.kind === "batch_reject" && (
                <ReasonForm label={`Reject ${String(chosen.length)} selected items. The reason, recorded on each`} confirmLabel={`Reject ${String(chosen.length)}`} required={true} danger={true} busy={busy} problem={problem} onCancel={closePanel} onSubmit={(reason) => void decideBatch("reject", reason)} />
              )}
              {canDecide && pending && state.panel.kind === "none" && (
                <div role="group" aria-label="Decide this item" className="flex flex-wrap items-center gap-2">
                  <Button variant="primary" onClick={() => run("approve")}>Approve (a)</Button>
                  <Button onClick={() => run("approve_note")}>With a note (n)</Button>
                  <Button onClick={() => run("reject")}>Reject (r)</Button>
                  <Button onClick={() => run("edit")}>Edit, then approve (e)</Button>
                  <Button onClick={() => run("toggle_select")}>{current !== null && state.selected.includes(current) ? "Unselect (x)" : "Select (x)"}</Button>
                  {chosen.length > 0 && <Button onClick={() => run("batch_approve")}>Approve {chosen.length} selected (Shift+A)</Button>}
                  {chosen.length > 0 && <Button onClick={() => run("batch_reject")}>Reject {chosen.length} selected (Shift+R)</Button>}
                </div>
              )}
              {current !== null && (
                <QueryView query={detailQuery} label="this item">
                  {() => (detail === null ? null : (
                    <div className="grid gap-3 2xl:contents">
                      <ItemView detail={detail} />
                    </div>
                  ))}
                </QueryView>
              )}
            </div>
            <div className="col-span-2 min-w-0 2xl:col-span-1">
              {detail !== null && <SourcePane vault={vault} evidence={evidence} at={Math.min(state.evidence, Math.max(0, detail.evidence.length - 1))} of={detail.evidence.length} />}
            </div>
          </div>
        ))}
      </QueryView>
    </div>
  );
};
