//! Request and response types for operating the server over HTTP (spec 039): operations as jobs, imports, settings, status, LLM usage and MCP's connection details.
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

/// An operation to run on a vault as a job. Its progress and report are on the job.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct OperationBody {
    /// `reindex`, `rollback`, `reconcile`, `export`, `sweep` or `compact`.
    pub op: String,
    /// `keyword` or `vector`, for `reindex` and `rollback`.
    pub index: Option<String>,
    /// `reconcile`: also replay up to this event.
    pub at: Option<u64>,
    /// `reconcile`: re-hash every blob.
    pub deep: Option<bool>,
    /// `reconcile`: rebuild an index that differs.
    pub repair: Option<bool>,
    /// `sweep`: how old an unreferenced blob must be to go. The server's `MEMEX_SWEEP_GRACE_S` when absent.
    pub grace_s: Option<u64>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ImportQuery {
    /// Restore under a new id, keeping the vault the archive came from.
    pub new_id: Option<bool>,
    /// Overwrite the vault that has the archive's id. Without it, restoring over an existing vault is refused.
    pub replace: Option<bool>,
    /// A name for the restored vault, instead of the archive's.
    pub name: Option<String>,
}

/// An import, which restores an archive into a vault. Imports are held in the server's memory, so they're gone after a restart, and the vault they made stays.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ImportDto {
    pub id: String,
    /// `running`, `done` or `failed`.
    pub state: String,
    pub progress: crate::JobProgressDto,
    pub started_at_ms: i64,
    pub finished_at_ms: Option<i64>,
    pub error: Option<String>,
    /// The vault, its events, blobs and embeddings, once done.
    #[schema(value_type = Option<Object>)]
    pub report: Option<serde_json::Value>,
}

/// Whether something the server needs answered, and how fast.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ReachDto {
    /// False when it's not set up, such as an LLM that's off.
    pub configured: bool,
    pub reachable: bool,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ModelStatusDto {
    /// `embedder`, `reranker`, `extraction`, `review`, `audit`, `chat` or `transcriber`.
    pub role: String,
    pub model: Option<String>,
    /// `ready`, `off`, `missing` and so on.
    pub status: String,
}

/// The server's health for operators. It answers within 2 s even with Postgres or the LLM down.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SystemStatusDto {
    pub version: String,
    pub postgres: ReachDto,
    pub llm: ReachDto,
    /// The server process's resident memory, where the platform says.
    pub memory_rss_bytes: Option<u64>,
    /// Handlers left running past their job's time ceiling.
    pub abandoned_handlers: u64,
    #[schema(value_type = Object)]
    pub backends: serde_json::Value,
    pub models: Vec<ModelStatusDto>,
    #[schema(value_type = Object)]
    pub thresholds: serde_json::Value,
    pub caches: Vec<crate::CacheTierDto>,
    /// Each vault's head, index lag, queue depth and sizes. None when they couldn't be read in time, with the reason in `vaults_error`.
    pub vaults: Option<Vec<crate::VaultStatusDto>>,
    pub vaults_error: Option<String>,
    /// True when Postgres couldn't check the caller and a recent check of the same credential was used instead.
    pub caller_from_memory: bool,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ClearedCachesDto {
    pub vaults: u64,
    pub caches: Vec<crate::CacheTierDto>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct UsageQuery {
    /// From this day, `YYYY-MM-DD` in UTC, inclusive. All time when absent.
    pub from: Option<String>,
    /// To this day, `YYYY-MM-DD` in UTC, inclusive. All time when absent.
    pub to: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UsageRowDto {
    /// The Derivation's kind, such as `extraction` or `chat`.
    pub agent: String,
    pub model: String,
    pub vault_id: String,
    /// `YYYY-MM-DD`, UTC.
    pub day: String,
    pub derivations: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub thinking_tokens: u64,
    /// US dollars, only when `MEMEX_LLM_PRICES` gives the model a price. Never guessed.
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UsageDto {
    pub rows: Vec<UsageRowDto>,
    /// The sum over rows with a price.
    pub priced_cost_usd: f64,
    /// Tokens in rows with no price, which the cost leaves out.
    pub unpriced_tokens: u64,
}

impl From<memex_core::UsageReport> for UsageDto {
    fn from(r: memex_core::UsageReport) -> Self {
        Self {
            rows: r
                .rows
                .into_iter()
                .map(|u| UsageRowDto {
                    agent: u.agent,
                    model: u.model,
                    vault_id: u.vault.0,
                    day: u.day,
                    derivations: u.derivations,
                    input_tokens: u.input_tokens,
                    output_tokens: u.output_tokens,
                    thinking_tokens: u.thinking_tokens,
                    cost_usd: u.cost_usd,
                })
                .collect(),
            priced_cost_usd: r.priced_cost_usd,
            unpriced_tokens: r.unpriced_tokens,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct McpToolDto {
    pub name: String,
    pub description: String,
}

/// How to connect an MCP client to this server.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct McpInfoDto {
    /// The streamable HTTP endpoint's path on this server.
    pub endpoint: String,
    /// `streamable-http`, and `stdio` through `memexvault mcp`.
    pub transports: Vec<String>,
    /// The newest protocol version the server speaks.
    pub protocol_version: String,
    /// Every version it accepts, oldest first.
    pub supported_protocol_versions: Vec<String>,
    /// Hosts the HTTP endpoint answers to.
    pub allowed_hosts: Vec<String>,
    pub tools: Vec<McpToolDto>,
}
