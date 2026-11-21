import { auditAccess, auditAccessText, formatCount, intervalText, MAX_SAMPLE, parseSchedule, scheduleForm } from "@memex/core";
import type { ScheduleForm } from "@memex/core";
import type { AuditSchedule, Identity, VaultId } from "@memex/types";
import { Button, Input } from "@memex/ui";
import { useQuery } from "@tanstack/react-query";
import { useId, useState } from "react";
import { useEngine } from "../engine-context";
import { QUERY_KEYS, unwrap } from "../query";
import { QueryView } from "../shell/query-view";
import { failureText, formatTime } from "../wiki/wiki-common";
import { Fact, useRefreshAudit } from "./audit-common";

const Editor = ({ vault, schedule, onDone }: { readonly vault: VaultId; readonly schedule: AuditSchedule; readonly onDone: () => void }) => {
  const engine = useEngine();
  const refresh = useRefreshAudit(vault);
  const ttlId = useId();
  const [form, setForm] = useState<ScheduleForm>(scheduleForm(schedule));
  const [problems, setProblems] = useState<readonly string[]>([]);
  const [busy, setBusy] = useState(false);
  const save = async () => {
    const parsed = parseSchedule(form);
    if (!parsed.ok) {
      setProblems(parsed.problems);
      return;
    }
    setBusy(true);
    const result = await engine.putAuditSchedule(vault, parsed.value);
    setBusy(false);
    if (!result.ok) {
      setProblems([`The server refused this schedule: ${failureText(result.failure)}`]);
      return;
    }
    setProblems([]);
    await refresh();
    onDone();
  };
  const field = (key: Exclude<keyof ScheduleForm, "enabled">) => ({ value: form[key], onChange: (e: React.ChangeEvent<HTMLInputElement>) => setForm({ ...form, [key]: e.target.value }) });
  return (
    <form
      aria-label="Edit the schedule"
      className="flex flex-col gap-3 rounded-lg border border-border-strong bg-surface-raised p-3"
      onSubmit={(e) => {
        e.preventDefault();
        void save();
      }}
    >
      <label className="flex items-center gap-2 text-sm">
        <input type="checkbox" checked={form.enabled} onChange={(e) => setForm({ ...form, enabled: e.target.checked })} />
        Run on this schedule
      </label>
      <div className="flex flex-wrap gap-3">
        <Input label="Interval, in minutes" id="schedule-interval" inputMode="numeric" {...field("intervalMinutes")} />
        <Input label={`Sample size, 1 to ${String(MAX_SAMPLE)}`} id="schedule-sample" inputMode="numeric" {...field("sampleSize")} />
        <Input label="Token budget per run" id="schedule-run-budget" inputMode="numeric" {...field("runTokenBudget")} />
        <Input label="Token budget per UTC day" id="schedule-day-budget" inputMode="numeric" {...field("dayTokenBudget")} />
      </div>
      <div className="flex flex-col gap-1">
        <label htmlFor={ttlId} className="text-xs text-text-muted">Days until a predicate's claims go stale, one "predicate = days" a line</label>
        <textarea id={ttlId} rows={3} value={form.ttl} onChange={(e) => setForm({ ...form, ttl: e.target.value })} className="rounded-md border border-border-strong bg-surface px-2 py-1 font-mono text-sm text-text" />
      </div>
      {problems.length > 0 && (
        <ul role="alert" className="text-sm text-danger" data-testid="schedule-problems">
          {problems.map((p) => <li key={p}>{p}</li>)}
        </ul>
      )}
      <div className="flex gap-2">
        <Button type="submit" variant="primary" disabled={busy}>Save schedule</Button>
        <Button onClick={onDone}>Cancel</Button>
      </div>
    </form>
  );
};

export const ScheduleView = ({ identity, vault }: { readonly identity: Identity; readonly vault: VaultId }) => {
  const engine = useEngine();
  const access = auditAccess(identity, vault);
  const [editing, setEditing] = useState(false);
  const schedule = useQuery({ queryKey: QUERY_KEYS.auditSchedule(vault), queryFn: async () => unwrap(await engine.getAuditSchedule(vault)) });
  const budget = useQuery({ queryKey: QUERY_KEYS.auditBudget(vault), queryFn: async () => unwrap(await engine.getAuditBudget(vault)) });
  const note = auditAccessText(access);
  return (
    <div className="flex flex-col gap-3">
      <QueryView query={schedule} label="the schedule">
        {(s) => (
          <section aria-label="Schedule" className="flex flex-col gap-2 rounded-lg border border-border p-3">
            <p className="text-sm" data-testid="schedule-summary">
              {s.enabled ? <>Runs <strong>{intervalText(s.intervalMs)}</strong>{s.nextRunAt === null ? "" : `, next at ${formatTime(s.nextRunAt)}`}.</> : <>Off. Nothing runs by itself until someone turns it on.</>}
            </p>
            <dl className="grid grid-cols-2 gap-3 text-sm md:grid-cols-4">
              <Fact label="Sample size" testId="schedule-sample">{formatCount(s.sampleSize)}</Fact>
              <Fact label="Budget per run" testId="schedule-run-budget">{formatCount(s.runTokenBudget)} tokens</Fact>
              <Fact label="Budget per UTC day" testId="schedule-day-budget">{formatCount(s.dayTokenBudget)} tokens</Fact>
              <Fact label="Slots missed">{formatCount(s.totalMissedSlots)}</Fact>
            </dl>
            <div className="text-sm">
              <span className="text-xs text-text-muted">Staleness</span>
              {Object.keys(s.ttlDays).length === 0 ? <p className="text-text-muted">No predicate goes stale by age.</p> : (
                <ul className="font-mono text-xs">{Object.entries(s.ttlDays).map(([p, d]) => <li key={p}>{p}: {String(d)} days</li>)}</ul>
              )}
            </div>
            {access.kind === "allowed" ? (
              editing ? <Editor vault={vault} schedule={s} onDone={() => setEditing(false)} /> : <div><Button onClick={() => setEditing(true)}>Edit schedule</Button></div>
            ) : (
              <p className="text-sm text-text-muted" data-testid="read-only">{note}</p>
            )}
          </section>
        )}
      </QueryView>
      <QueryView query={budget} label="today's budget">
        {(b) => (
          <section aria-label="Today's budget" className="rounded-lg border border-border p-3 text-sm" data-testid="day-budget">
            <h2 className="mb-1 font-medium">Today, in UTC</h2>
            <p>{formatCount(b.daySpent)} of {formatCount(b.dayTokenBudget)} tokens spent, {formatCount(b.dayRemaining)} left. A call that wouldn't fit isn't made, and the run finishes with its deterministic checks.</p>
          </section>
        )}
      </QueryView>
    </div>
  );
};
