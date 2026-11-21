import type { AgentUsageReport, AuditBudget, AuditCheck, AuditRun, AuditRunDetail, AuditSchedule, AuditScheduleInput, AuditTrigger, Page, RecallReport, VaultId } from "@memex/types";
import { failed, ok } from "./api";
import type { ApiResult } from "./api";

/** The audit routes, and the server's usage report, which the agents' view reads. */
export interface AuditApi {
  listAuditRuns(vault: VaultId, cursor: string | null): Promise<ApiResult<Page<AuditRun>>>;
  getAuditRun(vault: VaultId, id: string): Promise<ApiResult<AuditRunDetail>>;
  listAuditChecks(vault: VaultId, run: string, findingsOnly: boolean, cursor: string | null): Promise<ApiResult<Page<AuditCheck>>>;
  triggerAudit(vault: VaultId, trigger: AuditTrigger): Promise<ApiResult<AuditRun>>;
  getAuditSchedule(vault: VaultId): Promise<ApiResult<AuditSchedule>>;
  putAuditSchedule(vault: VaultId, schedule: AuditScheduleInput): Promise<ApiResult<AuditSchedule>>;
  getAuditBudget(vault: VaultId): Promise<ApiResult<AuditBudget>>;
  getAuditRecall(vault: VaultId): Promise<ApiResult<RecallReport>>;
  /** Days are `YYYY-MM-DD` in UTC, inclusive. */
  getAgentUsage(from: string, to: string): Promise<ApiResult<AgentUsageReport>>;
}

export interface AuditCases {
  /** Every run, newest first, as the server lists them oldest first. */
  listAuditRuns(vault: VaultId): Promise<ApiResult<readonly AuditRun[]>>;
  getAuditRun(vault: VaultId, id: string): Promise<ApiResult<AuditRunDetail>>;
  /** Every check of a run, page by page, as a sample has at most 1,000. */
  listAuditChecks(vault: VaultId, run: string, findingsOnly: boolean): Promise<ApiResult<readonly AuditCheck[]>>;
  triggerAudit(vault: VaultId, trigger: AuditTrigger): Promise<ApiResult<AuditRun>>;
  getAuditSchedule(vault: VaultId): Promise<ApiResult<AuditSchedule>>;
  putAuditSchedule(vault: VaultId, schedule: AuditScheduleInput): Promise<ApiResult<AuditSchedule>>;
  getAuditBudget(vault: VaultId): Promise<ApiResult<AuditBudget>>;
  getAuditRecall(vault: VaultId): Promise<ApiResult<RecallReport>>;
  getAgentUsage(from: string, to: string): Promise<ApiResult<AgentUsageReport>>;
}

interface AuditDeps {
  readonly edges: { readonly api: AuditApi };
  readonly watched: <T>(call: Promise<ApiResult<T>>) => Promise<ApiResult<T>>;
}

/** 20 pages of 200 is 4,000 runs, ten years of a daily schedule. */
const MAX_RUN_PAGES = 20;

/** 1,000 checks at the server's 200 a page is 5 pages, so 10 leaves room and still can't loop for ever on a server handing back cursors. */
const MAX_CHECK_PAGES = 10;

export const createAuditCases = ({ edges, watched }: AuditDeps): AuditCases => {
  const { api } = edges;
  return {
    listAuditRuns: async (vault) => {
      const all: AuditRun[] = [];
      let cursor: string | null = null;
      for (let page = 0; page < MAX_RUN_PAGES; page += 1) {
        const result: ApiResult<Page<AuditRun>> = await watched(api.listAuditRuns(vault, cursor));
        if (!result.ok) return failed(result.failure);
        all.push(...result.value.items);
        cursor = result.value.nextCursor;
        if (cursor === null) break;
      }
      return ok(all.reverse());
    },
    getAuditRun: (vault, id) => watched(api.getAuditRun(vault, id)),
    listAuditChecks: async (vault, run, findingsOnly) => {
      const all: AuditCheck[] = [];
      let cursor: string | null = null;
      for (let page = 0; page < MAX_CHECK_PAGES; page += 1) {
        const result: ApiResult<Page<AuditCheck>> = await watched(api.listAuditChecks(vault, run, findingsOnly, cursor));
        if (!result.ok) return failed(result.failure);
        all.push(...result.value.items);
        cursor = result.value.nextCursor;
        if (cursor === null) break;
      }
      return ok(all);
    },
    triggerAudit: (vault, trigger) => watched(api.triggerAudit(vault, trigger)),
    getAuditSchedule: (vault) => watched(api.getAuditSchedule(vault)),
    putAuditSchedule: (vault, schedule) => watched(api.putAuditSchedule(vault, schedule)),
    getAuditBudget: (vault) => watched(api.getAuditBudget(vault)),
    getAuditRecall: (vault) => watched(api.getAuditRecall(vault)),
    getAgentUsage: (from, to) => watched(api.getAgentUsage(from, to)),
  };
};
