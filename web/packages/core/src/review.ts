import type { ApiFailure } from "./api";
import type { AutoCommitChange, AutoCommitEntry, ChangeSet, Identity, ReviewDetail, ReviewEvidence, VaultId } from "@memex/types";
import { scopeIn } from "./navigation";

/** Who may decide here, and if not, why. A scope isn't enough: an agent signed in to a session decides nothing, and the server refuses it too. */
export type DecisionAccess =
  | { readonly kind: "allowed" }
  | { readonly kind: "agent" }
  | { readonly kind: "needs_admin"; readonly have: string };

export const decisionAccess = (identity: Identity, vault: VaultId): DecisionAccess => {
  if (identity.principal.machine) return { kind: "agent" };
  const have = scopeIn(identity, vault);
  return have === "admin" ? { kind: "allowed" } : { kind: "needs_admin", have: have ?? "no" };
};

export const decisionAccessText = (access: DecisionAccess): string | null => {
  switch (access.kind) {
    case "allowed":
      return null;
    case "agent":
      return "You're signed in as an agent, and agents can't approve, reject or revert through a console session. A person has to sign in to decide these.";
    case "needs_admin":
      return `Deciding needs admin scope, and your token has ${access.have === "no" ? "none here" : access.have}. You can read everything here.`;
  }
};

// ---- Keyboard

export type QueueCommand =
  | "next"
  | "previous"
  | "approve"
  | "approve_note"
  | "reject"
  | "edit"
  | "toggle_select"
  | "select_all"
  | "batch_approve"
  | "batch_reject"
  | "open_source"
  | "next_evidence"
  | "help"
  | "cancel";

export interface KeyPress {
  readonly key: string;
  readonly ctrl: boolean;
  readonly meta: boolean;
  readonly alt: boolean;
  /** The key went to something that takes typing. */
  readonly typing: boolean;
}

/** What each key does. Shift shows up as the letter's case, so `A` and `a` differ. */
const QUEUE_KEYS: Readonly<Record<string, QueueCommand>> = {
  j: "next",
  ArrowDown: "next",
  k: "previous",
  ArrowUp: "previous",
  a: "approve",
  n: "approve_note",
  r: "reject",
  e: "edit",
  x: "toggle_select",
  X: "select_all",
  A: "batch_approve",
  R: "batch_reject",
  o: "open_source",
  s: "next_evidence",
  "?": "help",
  Escape: "cancel",
};

/** Null leaves the key alone. Typing and the browser's own shortcuts are never taken. */
export const queueCommandFor = (press: KeyPress): QueueCommand | null => {
  if (press.typing || press.ctrl || press.meta || press.alt) return null;
  return Object.hasOwn(QUEUE_KEYS, press.key) ? (QUEUE_KEYS[press.key] ?? null) : null;
};

export const QUEUE_KEY_HELP: readonly { readonly keys: string; readonly what: string }[] = [
  { keys: "j or ↓", what: "Next item" },
  { keys: "k or ↑", what: "Previous item" },
  { keys: "a", what: "Approve" },
  { keys: "n", what: "Approve with a note" },
  { keys: "r", what: "Reject, with a reason" },
  { keys: "e", what: "Edit, then approve" },
  { keys: "x", what: "Select for a batch" },
  { keys: "Shift+X", what: "Select all shown, or none" },
  { keys: "Shift+A", what: "Approve the selected items" },
  { keys: "Shift+R", what: "Reject the selected items, with a reason" },
  { keys: "s", what: "Next quote of this item" },
  { keys: "o", what: "Open the source in Documents" },
  { keys: "?", what: "Show or hide these keys" },
  { keys: "Esc", what: "Cancel what's open" },
];

// ---- The queue's state

/** What's open over the queue. Each takes typing or an answer, so the single keys stop while it's up. */
export type QueuePanel =
  | { readonly kind: "none" }
  | { readonly kind: "help" }
  | { readonly kind: "approve_note" }
  | { readonly kind: "reject" }
  | { readonly kind: "edit" }
  | { readonly kind: "batch_approve" }
  | { readonly kind: "batch_reject" };

export interface QueueState {
  /** The item in view. Null only when the queue is empty. */
  readonly current: string | null;
  readonly selected: readonly string[];
  readonly panel: QueuePanel;
  /** How far through the item's quotes the source pane is. */
  readonly evidence: number;
}

export const INITIAL_QUEUE: QueueState = { current: null, selected: [], panel: { kind: "none" }, evidence: 0 };

/** Set when a command can't go ahead, for the screen to say why. */
export type QueueRefusal = "cant_decide" | "nothing_selected" | "nothing_here" | "not_loaded" | null;

export interface QueueStep {
  readonly state: QueueState;
  readonly refused: QueueRefusal;
  /** Approve the current item now. */
  readonly approve: boolean;
  /** Open the source in Documents. */
  readonly openSource: boolean;
}

/** The item ids in the order shown, as the queue's list has them now. */
export interface QueueView {
  readonly ids: readonly string[];
  readonly canDecide: boolean;
  /** The current item is on screen, so a key can't decide something nobody has seen. */
  readonly ready: boolean;
  /** How many quotes the current item has. */
  readonly evidenceCount: number;
}

const moved = (state: QueueState, id: string | null): QueueState => (id === state.current ? state : { ...state, current: id, evidence: 0 });

const move = (state: QueueState, ids: readonly string[], by: number): QueueState => {
  if (ids.length === 0) return moved(state, null);
  const at = state.current === null ? -1 : ids.indexOf(state.current);
  if (at === -1) return moved(state, ids[0] ?? null);
  return moved(state, ids[Math.min(ids.length - 1, Math.max(0, at + by))] ?? null);
};

const step = (state: QueueState, over: Partial<QueueStep> = {}): QueueStep => ({ state, refused: null, approve: false, openSource: false, ...over });

/** The one place that says what a command does. A panel that's up only answers to `cancel` and `help`, since its own form takes everything else. */
export const queueStep = (state: QueueState, command: QueueCommand, view: QueueView): QueueStep => {
  if (command === "cancel") {
    if (state.panel.kind !== "none") return step({ ...state, panel: { kind: "none" } });
    return step(state.selected.length > 0 ? { ...state, selected: [] } : state);
  }
  if (command === "help") return step({ ...state, panel: state.panel.kind === "help" ? { kind: "none" } : state.panel.kind === "none" ? { kind: "help" } : state.panel });
  if (state.panel.kind !== "none" && state.panel.kind !== "help") return step(state);
  const closed: QueueState = state.panel.kind === "help" ? { ...state, panel: { kind: "none" } } : state;
  const open = (panel: QueuePanel): QueueStep => {
    if (!view.canDecide) return step(closed, { refused: "cant_decide" });
    if (closed.current === null) return step(closed, { refused: "nothing_here" });
    if (!view.ready) return step(closed, { refused: "not_loaded" });
    return step({ ...closed, panel });
  };
  const openBatch = (panel: QueuePanel): QueueStep => {
    if (!view.canDecide) return step(closed, { refused: "cant_decide" });
    if (selectedIn(closed, view.ids).length === 0) return step(closed, { refused: "nothing_selected" });
    return step({ ...closed, panel });
  };
  switch (command) {
    case "next":
      return step(move(closed, view.ids, 1));
    case "previous":
      return step(move(closed, view.ids, -1));
    case "approve":
      if (!view.canDecide) return step(closed, { refused: "cant_decide" });
      if (closed.current === null) return step(closed, { refused: "nothing_here" });
      if (!view.ready) return step(closed, { refused: "not_loaded" });
      return step(closed, { approve: true });
    case "approve_note":
      return open({ kind: "approve_note" });
    case "reject":
      return open({ kind: "reject" });
    case "edit":
      return open({ kind: "edit" });
    case "toggle_select": {
      const id = closed.current;
      if (id === null) return step(closed, { refused: "nothing_here" });
      return step({ ...closed, selected: closed.selected.includes(id) ? closed.selected.filter((s) => s !== id) : [...closed.selected, id] });
    }
    case "select_all":
      return step({ ...closed, selected: view.ids.length > 0 && selectedIn(closed, view.ids).length === view.ids.length ? [] : [...view.ids] });
    case "batch_approve":
      return openBatch({ kind: "batch_approve" });
    case "batch_reject":
      return openBatch({ kind: "batch_reject" });
    case "open_source":
      return closed.current === null ? step(closed, { refused: "nothing_here" }) : step(closed, { openSource: true });
    case "next_evidence":
      return step(view.evidenceCount < 2 ? closed : { ...closed, evidence: (closed.evidence + 1) % view.evidenceCount });
  }
};

/** What's selected and still in the list, in the list's order, so a batch can't touch what's gone. */
export const selectedIn = (state: QueueState, ids: readonly string[]): readonly string[] => ids.filter((id) => state.selected.includes(id));

/** Where to go once items have left the list: the next one after the first that left, or the last one. A reviewer keeps going forward. */
export const afterRemoving = (ids: readonly string[], removing: readonly string[], current: string | null): string | null => {
  const left = ids.filter((id) => !removing.includes(id));
  if (current !== null && left.includes(current)) return current;
  const at = current === null ? 0 : ids.indexOf(current);
  for (let i = Math.max(0, at); i < ids.length; i += 1) {
    const id = ids[i];
    if (id !== undefined && !removing.includes(id)) return id;
  }
  return left.at(-1) ?? null;
};

/** The list changed under the queue, by a refetch or someone else's decision. Keeps the selection and the place to what's still there. */
export const queueReconciled = (state: QueueState, before: readonly string[], ids: readonly string[]): QueueState => {
  const gone = before.filter((id) => !ids.includes(id));
  const current = state.current !== null && ids.includes(state.current) ? state.current : before.length === 0 || state.current === null ? (ids[0] ?? null) : (afterRemoving(before, gone, state.current) ?? ids[0] ?? null);
  const selected = state.selected.filter((id) => ids.includes(id));
  const next = moved(state, ids.includes(current ?? "") ? current : (ids[0] ?? null));
  return selected.length === state.selected.length ? next : { ...next, selected };
};

// ---- Outcomes of a decision

export type DecisionOutcome =
  | { readonly kind: "done"; readonly changeset: ChangeSet }
  /** Someone else decided first, or the vault moved. The message names them. Nothing was done. */
  | { readonly kind: "conflict"; readonly message: string }
  | { readonly kind: "not_allowed"; readonly message: string }
  | { readonly kind: "invalid"; readonly message: string }
  | { readonly kind: "failed"; readonly failure: ApiFailure };

export const decisionFailure = (failure: ApiFailure): DecisionOutcome => {
  if (failure.kind !== "http") return { kind: "failed", failure };
  if (failure.status === 409) return { kind: "conflict", message: failure.message };
  if (failure.status === 403) return { kind: "not_allowed", message: failure.message };
  if (failure.status === 400) return { kind: "invalid", message: failure.message };
  return { kind: "failed", failure };
};

export interface BatchSummary {
  readonly done: number;
  readonly problems: readonly { readonly id: string; readonly message: string }[];
}

export const summarizeBatch = (results: readonly { readonly id: string; readonly errorCode: string | null; readonly errorMessage: string | null }[]): BatchSummary => ({
  done: results.filter((r) => r.errorCode === null).length,
  problems: results.filter((r) => r.errorCode !== null).map((r) => ({ id: r.id, message: r.errorMessage ?? r.errorCode ?? "It failed." })),
});

/** The server takes at most this many in one request. */
export const BATCH_LIMIT = 200;

/** A long selection goes in several requests. */
export const batchesOf = <T>(items: readonly T[]): readonly (readonly T[])[] => {
  const out: (readonly T[])[] = [];
  for (let at = 0; at < items.length; at += BATCH_LIMIT) out.push(items.slice(at, at + BATCH_LIMIT));
  return out;
};

// ---- Reasons and edits

export const defaultApproveReason = "Approved in the console";

export const reasonProblem = (reason: string): string | null => (reason.trim() === "" ? "This needs a reason." : null);

export type ParsedOps = { readonly ok: true; readonly ops: readonly unknown[] } | { readonly ok: false; readonly message: string };

const isRecord = (value: unknown): value is Readonly<Record<string, unknown>> => typeof value === "object" && value !== null && !Array.isArray(value);

/** What a person typed into the edit box, checked before it goes anywhere. The server checks the rest. */
export const parseEditedOps = (text: string): ParsedOps => {
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    return { ok: false, message: "That isn't valid JSON." };
  }
  if (!Array.isArray(parsed)) return { ok: false, message: "The ops must be a JSON list." };
  if (parsed.length === 0) return { ok: false, message: "There has to be at least one op. To turn this down, reject it." };
  if (!parsed.every((op) => isRecord(op) && typeof op.op === "string")) return { ok: false, message: "Each op must be an object with an `op` name." };
  return { ok: true, ops: parsed as readonly unknown[] };
};

export const opsAsText = (ops: readonly unknown[]): string => JSON.stringify(ops, null, 2);

// ---- Describing ops, always as plain data for plain text

export interface OpSummary {
  readonly kind: string;
  /** What it does, in a few words. */
  readonly title: string;
  readonly fields: readonly { readonly label: string; readonly value: string }[];
  /** A claim's epistemic status, where it has one. */
  readonly epistemic: string | null;
}

const text = (value: unknown): string => {
  if (typeof value === "string") return value;
  if (value === null || value === undefined) return "";
  return JSON.stringify(value);
};

const OP_TITLES: Readonly<Record<string, string>> = {
  add_assertion: "Add a claim",
  supersede_assertion: "Replace a claim",
  retract_assertion: "Retire a claim",
  create_page: "Create a page",
  edit_page: "Edit a page",
  delete_page: "Delete a page",
  rename_page: "Rename a page",
  link_edge: "Add a link",
  unlink_edge: "Remove a link",
};

const opTitle = (kind: string): string => OP_TITLES[kind] ?? kind.replace(/_/g, " ");

const FIELD_LABELS: readonly (readonly [string, string])[] = [
  ["subject", "Subject"],
  ["predicate", "Predicate"],
  ["value", "Value"],
  ["target", "Target"],
  ["id", "Id"],
  ["title", "Title"],
  ["slug", "Slug"],
  ["body", "Body"],
  ["tier", "Tier"],
];

export const describeOp = (op: unknown): OpSummary => {
  if (!isRecord(op) || typeof op.op !== "string") return { kind: "unknown", title: "An op this console doesn't know", fields: [{ label: "Op", value: text(op) }], epistemic: null };
  const replacement = op.op === "supersede_assertion" && isRecord(op.replacement) ? op.replacement : null;
  const from = replacement === null ? op : { target: op.target, ...replacement };
  const fields = FIELD_LABELS.flatMap(([key, label]) => (from[key] === undefined || from[key] === null ? [] : [{ label, value: text(from[key]) }]));
  const epistemic = typeof from.epistemic === "string" ? from.epistemic : null;
  return { kind: op.op, title: opTitle(op.op), fields, epistemic };
};

export const describeMutation = (mutation: unknown): string => {
  if (!isRecord(mutation)) return "A change this console doesn't know.";
  const put = mutation.put;
  if (isRecord(put)) {
    const table = Object.keys(put)[0] ?? "row";
    return `Write back: ${table.replace(/_/g, " ")}`;
  }
  const removed = mutation.delete;
  if (isRecord(removed)) return `Remove: ${text(removed.table).replace(/_/g, " ")} ${text(removed.id)}`;
  return "A change this console doesn't know.";
};

export interface ValidatorLine {
  readonly name: string;
  readonly passed: boolean;
  readonly detail: string;
}

/** The server's validator results are open-ended objects. Anything unreadable is shown as not passed, so a new shape can't read as a pass. */
export const validatorLine = (raw: unknown): ValidatorLine => {
  if (!isRecord(raw)) return { name: "Validator", passed: false, detail: text(raw) };
  const name = text(raw.validator ?? raw.name ?? raw.check) || "Validator";
  const passed = raw.ok === true || raw.passed === true || raw.outcome === "pass" || raw.status === "pass";
  const detail = text(raw.detail ?? raw.message ?? raw.reason);
  return { name, passed, detail };
};

// ---- Plain language for auto-commit. Everything here has to be true, and the server's rules are in docs/architecture.md.

/** The audit agent's principal, which is how a revocation is told from a person's disable. */
export const AUDITOR = "audit-agent";

export type PathTone = "never" | "off" | "on";

export interface AutomaticPath {
  readonly tone: PathTone;
  readonly title: string;
  readonly detail: string;
}

export const isChatProposal = (changeset: ChangeSet): boolean => changeset.requestedBy !== null;

/** Says whether anything but a person could ever have committed this. An item that always needs a person never shows a way round that. */
export const automaticPath = (detail: ReviewDetail): AutomaticPath => {
  const { changeset } = detail;
  if (isChatProposal(changeset)) {
    return {
      tone: "never",
      title: "Only a person can approve this.",
      detail: `It came from a chat asked for by ${changeset.requestedBy ?? "someone"}. A chat's proposals never auto-commit, and deciding one adds no earned samples to any combination.`,
    };
  }
  if (detail.needsAPerson) {
    return { tone: "never", title: "Only a person can approve this.", detail: "This kind of change always needs a person, whatever any combination has earned, and no agent can approve it." };
  }
  if (!changeset.proposerIsMachine) {
    return { tone: "never", title: "Waiting for a person.", detail: "A person proposed this, so auto-commit doesn't apply to it." };
  }
  if (detail.autoCommitBlock !== null) {
    return {
      tone: "never",
      title: "This can't auto-commit.",
      detail: `It won't, however much its combination has earned, because ${detail.autoCommitBlock}. Only sources a person marked trusted can auto-commit.`,
    };
  }
  const on = detail.combinations.filter((c) => c.on);
  const types = changeset.itemTypes.length;
  if (types > 0 && on.length >= types) {
    return { tone: "on", title: "Its combination has since earned auto-commit.", detail: "It was proposed before that, so it waits here. New proposals like it, citing only sources a person marked trusted, commit by themselves." };
  }
  return { tone: "off", title: "Its combination hasn't earned auto-commit.", detail: "So a person decides this. A person's decision here counts as a sample for its combination." };
};

/** The last change, if it was a disable, and who did it. */
const lastDisable = (history: readonly AutoCommitChange[]): AutoCommitChange | null => {
  const last = history.at(-1);
  return last?.kind === "disabled" ? last : null;
};

export const percent = (value: number): string => `${(value * 100).toFixed(1)}%`;

export const autoCommitStateText = (entry: AutoCommitEntry, threshold: number): string => {
  const bound = percent(entry.wilsonBound);
  const needed = percent(threshold);
  if (entry.on) {
    return `On. Its lower bound of ${bound} clears the ${needed} it has to reach, so new proposals of this kind commit by themselves, but only when every source they cite is one a person marked trusted. Everything else still waits here.`;
  }
  const disable = lastDisable(entry.history);
  if (disable !== null && entry.correct + entry.incorrect < 1) {
    if (disable.by === AUDITOR) return `Off, because an audit failed one of its claims. One failed audit turns off auto-commit for its combination, and its counts started again. Only new samples can earn it back, and its lower bound has to reach ${needed}.`;
    return `Off, because ${disable.by} turned it off. Its counts started again. There's no way to turn it on by hand: only new samples can earn it back, and its lower bound has to reach ${needed}.`;
  }
  if (entry.correct + entry.incorrect === 0) return `Off. It starts off and has no samples yet, and its lower bound has to reach ${needed}.`;
  if (disable !== null && disable.by === AUDITOR) return `Off. An audit turned it off, and new samples have brought its lower bound to ${bound}, which has to reach ${needed}.`;
  return `Off. Its lower bound of ${bound} is below the ${needed} it has to reach.`;
};

export const changeKindText = (kind: string): string => {
  switch (kind) {
    case "earned":
      return "Earned";
    case "fell_below":
      return "Fell below";
    case "disabled":
      return "Turned off";
    default:
      return kind;
  }
};

/** A line for the history, naming an audit where one did it. */
export const changeByText = (change: AutoCommitChange): string => (change.by === AUDITOR ? "an audit" : change.by);

export const AUTO_COMMIT_RULES: readonly string[] = [
  "A combination is an extractor, a prompt version and an item type. It starts off, and earns auto-commit only from its record, never from a model's own confidence.",
  "Only sources a person marked trusted can auto-commit. A claim citing anything below that always waits for a person, however good its combination's record.",
  "A chat's proposals never auto-commit, and deciding one adds no earned samples.",
  "One failed audit turns off auto-commit for that combination, and its counts start again.",
  "Merges, deletes, retiring a claim and the other always-reviewed kinds always wait for a person. No agent can approve them.",
  "There's no way to turn a combination on by hand.",
];

// ---- The source pane

export type SourcePlace =
  | { readonly kind: "pages"; readonly source: string; readonly page: number }
  | { readonly kind: "text"; readonly source: string | null };

/** Where a quote is shown: on its page if it has one, or else in its segment's words. */
export const sourcePlace = (evidence: ReviewEvidence): SourcePlace => {
  const page = evidence.rects[0]?.page ?? evidence.pageFrom;
  if (evidence.sourceId !== null && page !== null) return { kind: "pages", source: evidence.sourceId, page };
  return { kind: "text", source: evidence.sourceId };
};

/** A segment's words split around the quote, to mark it without any markup in the text. A quote that isn't there marks nothing rather than guessing. */
export const markQuote = (segment: string, quote: string): { readonly before: string; readonly mark: string; readonly after: string } => {
  const at = quote === "" ? -1 : segment.indexOf(quote);
  if (at === -1) return { before: segment, mark: "", after: "" };
  return { before: segment.slice(0, at), mark: quote, after: segment.slice(at + quote.length) };
};

/** A change to the activity stream's frame that could move the queue. Only a ChangeSet can, and a frame that lists too many to carry says so by `omitted`. */
export const frameTouchesQueue = (frame: { readonly changesets: number; readonly omitted: number; readonly kind: string }): boolean =>
  frame.changesets > 0 || frame.omitted > 0 || frame.kind === "changeset.recorded";
