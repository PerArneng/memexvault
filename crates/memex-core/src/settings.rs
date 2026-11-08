use std::fmt;
use std::path::PathBuf;

/// The backend named for each edge trait. Only the composition root turns a name into a type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backends {
    pub store: String,
    pub blob: String,
    pub lexical: String,
    pub vector: String,
    pub embedder: String,
    pub reranker: String,
    pub ocr: String,
    pub transcriber: String,
}

impl Default for Backends {
    fn default() -> Self {
        Self {
            store: "postgres".into(),
            blob: "fs".into(),
            lexical: "tantivy".into(),
            vector: "pgvector".into(),
            embedder: "local".into(),
            reranker: "local".into(),
            ocr: "local".into(),
            transcriber: "local".into(),
        }
    }
}

/// Each cache tier's cap in bytes. 0 turns a tier off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheSettings {
    pub query_bytes: u64,
    pub inference_bytes: u64,
    pub object_bytes: u64,
    pub disk_bytes: u64,
}

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;
/// Below this a tier refuses almost every entry, as an entry can be at most a twentieth of its cap, so a smaller cap is a mistake rather than a choice.
pub const MIN_CACHE_BYTES: u64 = MIB;
/// More than the working assumptions could use, so a larger value is a typing error. The reference machine has 32 GB.
pub const MAX_RAM_CACHE_BYTES: u64 = 256 * GIB;
pub const MAX_DISK_CACHE_BYTES: u64 = 64 * 1024 * GIB;

/// Together the RAM tiers take 576 MiB of the 4 GB memory target.
impl Default for CacheSettings {
    fn default() -> Self {
        Self {
            query_bytes: 256 * MIB,
            inference_bytes: 64 * MIB,
            object_bytes: 256 * MIB,
            disk_bytes: 2 * GIB,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct Settings {
    pub database_url: Option<String>,
    pub data_dir: PathBuf,
    /// Where `models fetch` puts model files.
    pub models_dir: PathBuf,
    pub http_bind: String,
    /// The token `memexvault mcp` acts as, over stdio.
    pub mcp_token: Option<String>,
    /// Host names MCP over HTTP answers to. Only this machine's, unless a proxy in front needs others.
    pub mcp_allowed_hosts: Vec<String>,
    pub backends: Backends,
    /// CPU threads for each embedding run, so background embedding can't take the whole machine.
    pub embed_threads: usize,
    /// How many queries can be embedded at once. Each one holds a copy of the model in memory.
    pub embed_query_sessions: usize,
    /// CPU threads for each rerank.
    pub rerank_threads: usize,
    /// How many reranks can run at once. Each one holds a copy of the model in memory.
    pub rerank_sessions: usize,
    /// The LLM that extraction sends documents to. Without one, nothing leaves the server and no extraction is queued.
    pub llm: Option<LlmSettings>,
    pub extraction: crate::ExtractionLimits,
    pub caches: CacheSettings,
    /// The model the reviewer agent calls, on the same LLM. Without one, queued items wait for people with no verdict, and no extra tokens are spent.
    pub reviewer_model: Option<String>,
    pub review: crate::review::ReviewLimits,
    /// The model the audit's LLM checks call, on the same LLM. Without one, audits run only their deterministic checks, and no tokens are spent.
    pub audit_model: Option<String>,
    pub audit: crate::audit::AuditLimits,
    /// The model the chat agent calls, on the same LLM. Without one, nothing can answer a chat, whatever a vault's setting.
    pub chat_model: Option<String>,
    pub chat: crate::ChatLimits,
    /// What a combination's Wilson bound has to reach to earn auto-commit, in thousandths.
    pub auto_commit_threshold_permille: u16,
    pub transcribe: TranscribeSettings,
    pub activity: crate::ActivityLimits,
}

/// How recordings are transcribed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscribeSettings {
    /// CPU threads for whisper.
    pub threads: usize,
    /// A longer recording fails its job.
    pub max_seconds: u64,
    /// The worker is killed above this, and the job fails.
    pub max_rss_mb: u64,
    /// The worker is killed after this long with no progress.
    pub stall_seconds: u64,
    /// The API that hears a recording as well as the local model. Without it, no audio leaves this server, whatever else is set. Only `MEMEX_BACKEND_TRANSCRIBER=gemini` fills this.
    pub api: Option<TranscribeApi>,
}

impl Default for TranscribeSettings {
    fn default() -> Self {
        Self {
            threads: 4,
            max_seconds: 6 * 3600,
            max_rss_mb: 4096,
            stall_seconds: 1800,
            api: None,
        }
    }
}

/// The transcription API, which recordings go to when it's on.
#[derive(Clone, PartialEq, Eq)]
pub struct TranscribeApi {
    pub model: String,
    pub api_key: String,
    /// In place of the provider's own address.
    pub base_url: Option<String>,
    /// Spending for one recording stops here, counting input and output.
    pub max_tokens: u64,
}

/// The key is a secret, so Debug never shows it.
impl fmt::Debug for TranscribeApi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TranscribeApi")
            .field("model", &self.model)
            .field("api_key", &"<redacted>")
            .field("base_url", &self.base_url)
            .field("max_tokens", &self.max_tokens)
            .finish()
    }
}

/// Which model answers extraction's calls.
#[derive(Clone, PartialEq, Eq)]
pub struct LlmSettings {
    /// `gemini`, or `openai` for any OpenAI-compatible server.
    pub provider: String,
    pub model: String,
    /// Needed with `openai`.
    pub base_url: Option<String>,
    /// Needed with `gemini`.
    pub api_key: Option<String>,
}

/// The key is a secret, so Debug never shows it.
impl fmt::Debug for LlmSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LlmSettings")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            database_url: None,
            data_dir: PathBuf::from("./memex-data"),
            models_dir: PathBuf::from("./memex-models"),
            http_bind: "127.0.0.1:8080".into(),
            mcp_token: None,
            mcp_allowed_hosts: vec!["localhost".into(), "127.0.0.1".into(), "::1".into()],
            backends: Backends::default(),
            embed_threads: 4,
            embed_query_sessions: 2,
            rerank_threads: 4,
            rerank_sessions: 2,
            llm: None,
            extraction: crate::ExtractionLimits::default(),
            caches: CacheSettings::default(),
            reviewer_model: None,
            review: crate::review::ReviewLimits::default(),
            audit_model: None,
            chat_model: None,
            chat: crate::ChatLimits::default(),
            audit: crate::audit::AuditLimits::default(),
            auto_commit_threshold_permille: (crate::review::DEFAULT_THRESHOLD * 1000.0) as u16,
            transcribe: TranscribeSettings::default(),
            activity: crate::ActivityLimits::default(),
        }
    }
}

/// The connection URL holds a password, so Debug never shows it.
impl fmt::Debug for Settings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Settings")
            .field("database_url", &self.database_url.as_ref().map(|_| "<redacted>"))
            .field("data_dir", &self.data_dir)
            .field("models_dir", &self.models_dir)
            .field("http_bind", &self.http_bind)
            .field("mcp_token", &self.mcp_token.as_ref().map(|_| "<redacted>"))
            .field("mcp_allowed_hosts", &self.mcp_allowed_hosts)
            .field("backends", &self.backends)
            .field("embed_threads", &self.embed_threads)
            .field("embed_query_sessions", &self.embed_query_sessions)
            .field("rerank_threads", &self.rerank_threads)
            .field("rerank_sessions", &self.rerank_sessions)
            .field("llm", &self.llm)
            .field("extraction", &self.extraction)
            .field("caches", &self.caches)
            .field("reviewer_model", &self.reviewer_model)
            .field("review", &self.review)
            .field("audit_model", &self.audit_model)
            .field("chat_model", &self.chat_model)
            .field("chat", &self.chat)
            .field("audit", &self.audit)
            .field("auto_commit_threshold_permille", &self.auto_commit_threshold_permille)
            .field("transcribe", &self.transcribe)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsError {
    UnknownBackend {
        trait_name: &'static str,
        name: String,
        known: &'static [&'static str],
    },
    Empty {
        name: &'static str,
    },
    InvalidBind {
        value: String,
    },
    InvalidNumber {
        name: &'static str,
        value: String,
    },
    /// A cache cap that isn't a number of bytes, or is outside what's useful.
    InvalidCacheSize {
        name: &'static str,
        value: String,
        max: u64,
    },
    InvalidThreshold {
        value: String,
    },
    /// One setting needs another that isn't set.
    Missing {
        name: &'static str,
        because: String,
    },
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownBackend { trait_name, name, known } => write!(f, "unknown {trait_name} backend '{name}', known: {}", known.join(", ")),
            Self::Empty { name } => write!(f, "{name} is set but empty"),
            Self::InvalidBind { value } => write!(f, "MEMEX_HTTP_BIND must be host:port, got '{value}'"),
            Self::InvalidNumber { name, value } => write!(f, "{name} must be a whole number of at least 1, got '{value}'"),
            Self::InvalidCacheSize { name, value, max } => write!(
                f,
                "{name} must be a number of bytes, either 0 to turn the tier off or from {MIN_CACHE_BYTES} to {max}, got '{value}'"
            ),
            Self::InvalidThreshold { value } => write!(f, "MEMEX_AUTO_COMMIT_THRESHOLD must be above 0 and at most 1, got '{value}'"),
            Self::Missing { name, because } => write!(f, "{name} is needed {because}"),
        }
    }
}

impl std::error::Error for SettingsError {}

impl Settings {
    /// Parses settings from a lookup, so it's pure. `main.rs` passes the environment.
    pub fn from_vars(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, SettingsError> {
        let mut s = Self::default();
        let text = |name: &'static str| -> Result<Option<String>, SettingsError> {
            match lookup(name) {
                Some(v) if v.trim().is_empty() => Err(SettingsError::Empty { name }),
                other => Ok(other),
            }
        };
        s.database_url = text("MEMEX_DATABASE_URL")?;
        if let Some(v) = text("MEMEX_DATA_DIR")? {
            s.data_dir = PathBuf::from(v);
        }
        if let Some(v) = text("MEMEX_MODELS_DIR")? {
            s.models_dir = PathBuf::from(v);
        }
        let count = |name: &'static str, value: Option<String>, default: usize| -> Result<usize, SettingsError> {
            match value {
                None => Ok(default),
                Some(v) => v.parse().ok().filter(|n| *n >= 1).ok_or(SettingsError::InvalidNumber { name, value: v }),
            }
        };
        s.llm = match text("MEMEX_LLM_PROVIDER")? {
            None => None,
            Some(provider) => {
                let because = format!("with MEMEX_LLM_PROVIDER={provider}");
                let need = |name: &'static str, value: Option<String>| {
                    value.ok_or(SettingsError::Missing {
                        name,
                        because: because.clone(),
                    })
                };
                let model = need("MEMEX_LLM_MODEL", text("MEMEX_LLM_MODEL")?)?;
                let (base_url, api_key) = (text("MEMEX_LLM_BASE_URL")?, text("MEMEX_LLM_API_KEY")?);
                match provider.as_str() {
                    "gemini" => {
                        need("MEMEX_LLM_API_KEY", api_key.clone())?;
                    }
                    "openai" => {
                        need("MEMEX_LLM_BASE_URL", base_url.clone())?;
                    }
                    _ => {
                        return Err(SettingsError::UnknownBackend {
                            trait_name: "AgentRunner",
                            name: provider,
                            known: &["gemini", "openai"],
                        });
                    }
                }
                Some(LlmSettings {
                    provider,
                    model,
                    base_url,
                    api_key,
                })
            }
        };
        let e = &mut s.extraction;
        e.document_budget = count("MEMEX_EXTRACT_TOKEN_BUDGET", text("MEMEX_EXTRACT_TOKEN_BUDGET")?, e.document_budget as usize)? as u64;
        e.batch_tokens = count("MEMEX_EXTRACT_BATCH_TOKENS", text("MEMEX_EXTRACT_BATCH_TOKENS")?, e.batch_tokens as usize)? as u64;
        e.output_tokens = count("MEMEX_EXTRACT_OUTPUT_TOKENS", text("MEMEX_EXTRACT_OUTPUT_TOKENS")?, e.output_tokens as usize)? as u32;
        e.timeout_ms = count("MEMEX_EXTRACT_TIMEOUT_S", text("MEMEX_EXTRACT_TIMEOUT_S")?, (e.timeout_ms / 1000) as usize)? as u64 * 1000;
        let cache_size = |name: &'static str, value: Option<String>, default: u64, max: u64| -> Result<u64, SettingsError> {
            let Some(v) = value else { return Ok(default) };
            v.parse::<u64>()
                .ok()
                .filter(|n| *n == 0 || (MIN_CACHE_BYTES..=max).contains(n))
                .ok_or(SettingsError::InvalidCacheSize { name, value: v, max })
        };
        let c = &mut s.caches;
        c.query_bytes = cache_size("MEMEX_CACHE_QUERY_BYTES", text("MEMEX_CACHE_QUERY_BYTES")?, c.query_bytes, MAX_RAM_CACHE_BYTES)?;
        c.inference_bytes = cache_size(
            "MEMEX_CACHE_INFERENCE_BYTES",
            text("MEMEX_CACHE_INFERENCE_BYTES")?,
            c.inference_bytes,
            MAX_RAM_CACHE_BYTES,
        )?;
        c.object_bytes = cache_size(
            "MEMEX_CACHE_OBJECT_BYTES",
            text("MEMEX_CACHE_OBJECT_BYTES")?,
            c.object_bytes,
            MAX_RAM_CACHE_BYTES,
        )?;
        c.disk_bytes = cache_size("MEMEX_CACHE_DISK_BYTES", text("MEMEX_CACHE_DISK_BYTES")?, c.disk_bytes, MAX_DISK_CACHE_BYTES)?;
        s.reviewer_model = text("MEMEX_REVIEW_MODEL")?;
        if s.reviewer_model.is_some() && s.llm.is_none() {
            return Err(SettingsError::Missing {
                name: "MEMEX_LLM_PROVIDER",
                because: "with MEMEX_REVIEW_MODEL".into(),
            });
        }
        s.review.output_tokens = count(
            "MEMEX_REVIEW_OUTPUT_TOKENS",
            text("MEMEX_REVIEW_OUTPUT_TOKENS")?,
            s.review.output_tokens as usize,
        )? as u32;
        s.review.timeout_ms = count("MEMEX_REVIEW_TIMEOUT_S", text("MEMEX_REVIEW_TIMEOUT_S")?, (s.review.timeout_ms / 1000) as usize)? as u64 * 1000;
        s.audit_model = text("MEMEX_AUDIT_MODEL")?;
        if s.audit_model.is_some() && s.llm.is_none() {
            return Err(SettingsError::Missing {
                name: "MEMEX_LLM_PROVIDER",
                because: "with MEMEX_AUDIT_MODEL".into(),
            });
        }
        s.chat_model = text("MEMEX_CHAT_MODEL")?;
        if s.chat_model.is_some() && s.llm.is_none() {
            return Err(SettingsError::Missing {
                name: "MEMEX_LLM_PROVIDER",
                because: "with MEMEX_CHAT_MODEL".into(),
            });
        }
        let c = &mut s.chat;
        c.output_tokens = count("MEMEX_CHAT_OUTPUT_TOKENS", text("MEMEX_CHAT_OUTPUT_TOKENS")?, c.output_tokens as usize)? as u32;
        c.thread_tokens = count("MEMEX_CHAT_THREAD_TOKENS", text("MEMEX_CHAT_THREAD_TOKENS")?, c.thread_tokens as usize)? as u64;
        c.day_tokens = count("MEMEX_CHAT_DAY_TOKENS", text("MEMEX_CHAT_DAY_TOKENS")?, c.day_tokens as usize)? as u64;
        c.timeout_ms = count("MEMEX_CHAT_TIMEOUT_S", text("MEMEX_CHAT_TIMEOUT_S")?, (c.timeout_ms / 1000) as usize)? as u64 * 1000;
        c.max_model_calls = count("MEMEX_CHAT_MODEL_CALLS", text("MEMEX_CHAT_MODEL_CALLS")?, c.max_model_calls as usize)? as u32;
        c.max_runs = count("MEMEX_CHAT_MAX_RUNS", text("MEMEX_CHAT_MAX_RUNS")?, c.max_runs)?;
        s.audit.output_tokens = count("MEMEX_AUDIT_OUTPUT_TOKENS", text("MEMEX_AUDIT_OUTPUT_TOKENS")?, s.audit.output_tokens as usize)? as u32;
        s.audit.timeout_ms = count("MEMEX_AUDIT_TIMEOUT_S", text("MEMEX_AUDIT_TIMEOUT_S")?, (s.audit.timeout_ms / 1000) as usize)? as u64 * 1000;
        if let Some(v) = text("MEMEX_AUTO_COMMIT_THRESHOLD")? {
            // Zero would let a combination with one lucky sample through, and rounding must not turn a small number into it.
            let permille = v
                .parse::<f64>()
                .ok()
                .filter(|t| *t > 0.0 && *t <= 1.0)
                .map(|t| (t * 1000.0).round() as u16)
                .filter(|p| *p >= 1);
            s.auto_commit_threshold_permille = permille.ok_or(SettingsError::InvalidThreshold { value: v })?;
        }
        s.embed_threads = count("MEMEX_EMBED_THREADS", text("MEMEX_EMBED_THREADS")?, s.embed_threads)?;
        s.embed_query_sessions = count("MEMEX_EMBED_QUERY_SESSIONS", text("MEMEX_EMBED_QUERY_SESSIONS")?, s.embed_query_sessions)?;
        s.rerank_threads = count("MEMEX_RERANK_THREADS", text("MEMEX_RERANK_THREADS")?, s.rerank_threads)?;
        s.rerank_sessions = count("MEMEX_RERANK_SESSIONS", text("MEMEX_RERANK_SESSIONS")?, s.rerank_sessions)?;
        s.mcp_token = text("MEMEX_MCP_TOKEN")?;
        if let Some(v) = text("MEMEX_MCP_ALLOWED_HOSTS")? {
            s.mcp_allowed_hosts = v.split(',').map(str::trim).filter(|h| !h.is_empty()).map(String::from).collect();
        }
        if let Some(v) = text("MEMEX_HTTP_BIND")? {
            let port_ok = v.rsplit_once(':').is_some_and(|(host, port)| !host.is_empty() && port.parse::<u16>().is_ok());
            if !port_ok {
                return Err(SettingsError::InvalidBind { value: v });
            }
            s.http_bind = v;
        }
        // A new adapter is added to its list when its crate exists, so an unknown or uncompiled name stops startup.
        let mut pick = |var: &'static str,
                        trait_name: &'static str,
                        known: &'static [&'static str],
                        slot: fn(&mut Backends) -> &mut String|
         -> Result<(), SettingsError> {
            if let Some(name) = text(var)? {
                if !known.contains(&name.as_str()) {
                    return Err(SettingsError::UnknownBackend { trait_name, name, known });
                }
                *slot(&mut s.backends) = name;
            }
            Ok(())
        };
        pick("MEMEX_BACKEND_STORE", "Store", &["postgres"], |b| &mut b.store)?;
        pick("MEMEX_BACKEND_BLOB", "BlobStore", &["fs"], |b| &mut b.blob)?;
        pick("MEMEX_BACKEND_LEXICAL", "LexicalIndex", &["tantivy"], |b| &mut b.lexical)?;
        pick("MEMEX_BACKEND_VECTOR", "VectorIndex", &["pgvector"], |b| &mut b.vector)?;
        pick("MEMEX_BACKEND_EMBEDDER", "Embedder", &["local"], |b| &mut b.embedder)?;
        pick("MEMEX_BACKEND_RERANKER", "Reranker", &["local"], |b| &mut b.reranker)?;
        pick("MEMEX_BACKEND_OCR", "Ocr", &["local"], |b| &mut b.ocr)?;
        pick("MEMEX_BACKEND_TRANSCRIBER", "Transcriber", &["local", "gemini"], |b| &mut b.transcriber)?;
        let t = &mut s.transcribe;
        t.threads = count("MEMEX_TRANSCRIBE_THREADS", text("MEMEX_TRANSCRIBE_THREADS")?, t.threads)?;
        t.max_seconds = count("MEMEX_TRANSCRIBE_MAX_SECONDS", text("MEMEX_TRANSCRIBE_MAX_SECONDS")?, t.max_seconds as usize)? as u64;
        t.max_rss_mb = count("MEMEX_TRANSCRIBE_MAX_RSS_MB", text("MEMEX_TRANSCRIBE_MAX_RSS_MB")?, t.max_rss_mb as usize)? as u64;
        t.stall_seconds = count("MEMEX_TRANSCRIBE_STALL_S", text("MEMEX_TRANSCRIBE_STALL_S")?, t.stall_seconds as usize)? as u64;
        let a = &mut s.activity;
        a.max_streams = count("MEMEX_ACTIVITY_MAX_STREAMS", text("MEMEX_ACTIVITY_MAX_STREAMS")?, a.max_streams)?;
        a.buffer.max_frames = count("MEMEX_ACTIVITY_BUFFER_FRAMES", text("MEMEX_ACTIVITY_BUFFER_FRAMES")?, a.buffer.max_frames)?;
        a.buffer.max_bytes = count("MEMEX_ACTIVITY_BUFFER_BYTES", text("MEMEX_ACTIVITY_BUFFER_BYTES")?, a.buffer.max_bytes)?;
        a.poll_ms = count("MEMEX_ACTIVITY_POLL_MS", text("MEMEX_ACTIVITY_POLL_MS")?, a.poll_ms as usize)? as u64;
        a.heartbeat_ms = count(
            "MEMEX_ACTIVITY_HEARTBEAT_S",
            text("MEMEX_ACTIVITY_HEARTBEAT_S")?,
            (a.heartbeat_ms / 1000) as usize,
        )? as u64
            * 1000;
        a.recheck_ms = count("MEMEX_ACTIVITY_RECHECK_MS", text("MEMEX_ACTIVITY_RECHECK_MS")?, a.recheck_ms as usize)? as u64;
        a.stall_ms = count("MEMEX_ACTIVITY_STALL_S", text("MEMEX_ACTIVITY_STALL_S")?, (a.stall_ms / 1000) as usize)? as u64 * 1000;
        // Naming the API is the only thing that turns it on. A key on its own does nothing.
        if s.backends.transcriber == "gemini" {
            let because = "with MEMEX_BACKEND_TRANSCRIBER=gemini".to_string();
            let need = |name: &'static str, value: Option<String>| {
                value.ok_or_else(|| SettingsError::Missing {
                    name,
                    because: because.clone(),
                })
            };
            t.api = Some(TranscribeApi {
                model: need("MEMEX_TRANSCRIBE_API_MODEL", text("MEMEX_TRANSCRIBE_API_MODEL")?)?,
                api_key: need("MEMEX_TRANSCRIBE_API_KEY", text("MEMEX_TRANSCRIBE_API_KEY")?)?,
                base_url: text("MEMEX_TRANSCRIBE_API_BASE_URL")?,
                max_tokens: count("MEMEX_TRANSCRIBE_API_MAX_TOKENS", text("MEMEX_TRANSCRIBE_API_MAX_TOKENS")?, 1_000_000)? as u64,
            });
        }
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn from(vars: &[(&str, &str)]) -> Result<Settings, SettingsError> {
        let map: HashMap<String, String> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        Settings::from_vars(|name| map.get(name).cloned())
    }

    #[test]
    fn the_mcp_token_is_redacted_and_hosts_stay_local_unless_named() {
        let s = from(&[
            ("MEMEX_MCP_TOKEN", "mxt_secret"),
            ("MEMEX_MCP_ALLOWED_HOSTS", "kb.example.org, kb.example.org:443"),
        ])
        .unwrap();
        assert_eq!(s.mcp_token.as_deref(), Some("mxt_secret"));
        assert_eq!(s.mcp_allowed_hosts, ["kb.example.org", "kb.example.org:443"]);
        assert!(!format!("{s:?}").contains("mxt_secret"));
        assert!(from(&[]).unwrap().mcp_allowed_hosts.contains(&"127.0.0.1".to_string()));
    }

    #[test]
    fn defaults_when_nothing_is_set() {
        assert_eq!(from(&[]).unwrap(), Settings::default());
    }

    #[test]
    fn valid_values_are_read() {
        let s = from(&[
            ("MEMEX_DATA_DIR", "/srv/memex"),
            ("MEMEX_HTTP_BIND", "0.0.0.0:9000"),
            ("MEMEX_BACKEND_STORE", "postgres"),
            ("MEMEX_DATABASE_URL", "postgres://u:p@h/d"),
        ])
        .unwrap();
        assert_eq!(s.data_dir, PathBuf::from("/srv/memex"));
        assert_eq!(s.http_bind, "0.0.0.0:9000");
        assert_eq!(s.database_url.as_deref(), Some("postgres://u:p@h/d"));
    }

    #[test]
    fn unknown_backend_is_refused_and_names_the_choices() {
        let e = from(&[("MEMEX_BACKEND_STORE", "redb")]).unwrap_err();
        assert_eq!(
            e,
            SettingsError::UnknownBackend {
                trait_name: "Store",
                name: "redb".into(),
                known: &["postgres"]
            }
        );
        assert!(e.to_string().contains("known: postgres"));
    }

    #[test]
    fn every_backend_setting_is_checked() {
        for var in ["BLOB", "LEXICAL", "VECTOR", "EMBEDDER", "RERANKER", "OCR", "TRANSCRIBER"] {
            let name = format!("MEMEX_BACKEND_{var}");
            assert!(matches!(from(&[(&name, "nope")]), Err(SettingsError::UnknownBackend { .. })), "{name}");
        }
    }

    #[test]
    fn empty_values_are_refused() {
        assert_eq!(from(&[("MEMEX_DATA_DIR", "  ")]).unwrap_err(), SettingsError::Empty { name: "MEMEX_DATA_DIR" });
    }

    #[test]
    fn bad_bind_is_refused() {
        for bad in ["8080", ":8080", "host:notaport", "host:70000"] {
            assert!(matches!(from(&[("MEMEX_HTTP_BIND", bad)]), Err(SettingsError::InvalidBind { .. })), "{bad}");
        }
    }

    #[test]
    fn debug_never_shows_the_password() {
        let s = from(&[("MEMEX_DATABASE_URL", "postgres://u:hunter2@h/d")]).unwrap();
        assert!(!format!("{s:?}").contains("hunter2"));
    }

    #[test]
    fn embedding_limits_parse_and_refuse_nonsense() {
        let s = from(&[("MEMEX_EMBED_THREADS", "2"), ("MEMEX_EMBED_QUERY_SESSIONS", "3")]).unwrap();
        assert_eq!((s.embed_threads, s.embed_query_sessions), (2, 3));
        assert_eq!(from(&[]).unwrap().embed_threads, 4);
        for bad in ["0", "many", "-1"] {
            assert!(
                matches!(from(&[("MEMEX_EMBED_THREADS", bad)]), Err(SettingsError::InvalidNumber { .. })),
                "{bad}"
            );
        }
    }

    #[test]
    fn extraction_is_off_without_a_provider_and_its_knobs_parse() {
        let s = from(&[]).unwrap();
        assert_eq!(s.llm, None);
        assert_eq!(s.extraction, crate::ExtractionLimits::default());
        let s = from(&[("MEMEX_EXTRACT_TOKEN_BUDGET", "1000"), ("MEMEX_EXTRACT_TIMEOUT_S", "9")]).unwrap();
        assert_eq!((s.extraction.document_budget, s.extraction.timeout_ms), (1000, 9000));
        assert!(matches!(from(&[("MEMEX_EXTRACT_BATCH_TOKENS", "0")]), Err(SettingsError::InvalidNumber { .. })));
    }

    #[test]
    fn each_provider_needs_what_it_needs() {
        assert!(matches!(
            from(&[("MEMEX_LLM_PROVIDER", "gemini")]),
            Err(SettingsError::Missing { name: "MEMEX_LLM_MODEL", .. })
        ));
        assert!(matches!(
            from(&[("MEMEX_LLM_PROVIDER", "gemini"), ("MEMEX_LLM_MODEL", "m")]),
            Err(SettingsError::Missing { name: "MEMEX_LLM_API_KEY", .. })
        ));
        assert!(matches!(
            from(&[("MEMEX_LLM_PROVIDER", "openai"), ("MEMEX_LLM_MODEL", "m")]),
            Err(SettingsError::Missing {
                name: "MEMEX_LLM_BASE_URL",
                ..
            })
        ));
        assert!(matches!(
            from(&[("MEMEX_LLM_PROVIDER", "nope"), ("MEMEX_LLM_MODEL", "m")]),
            Err(SettingsError::UnknownBackend { .. })
        ));
        let s = from(&[
            ("MEMEX_LLM_PROVIDER", "openai"),
            ("MEMEX_LLM_MODEL", "m"),
            ("MEMEX_LLM_BASE_URL", "http://h/v1"),
        ])
        .unwrap();
        assert_eq!(s.llm.unwrap().base_url.as_deref(), Some("http://h/v1"));
    }

    #[test]
    fn chat_is_off_unless_a_model_is_named_and_its_limits_parse() {
        assert_eq!(from(&[]).unwrap().chat_model, None);
        assert_eq!(from(&[]).unwrap().chat, crate::ChatLimits::default());
        assert!(matches!(from(&[("MEMEX_CHAT_MODEL", "m")]), Err(SettingsError::Missing { .. })));
        let s = from(&[
            ("MEMEX_LLM_PROVIDER", "openai"),
            ("MEMEX_LLM_MODEL", "m"),
            ("MEMEX_LLM_BASE_URL", "http://h/v1"),
            ("MEMEX_CHAT_MODEL", "chatty"),
            ("MEMEX_CHAT_OUTPUT_TOKENS", "100"),
            ("MEMEX_CHAT_THREAD_TOKENS", "200"),
            ("MEMEX_CHAT_DAY_TOKENS", "300"),
            ("MEMEX_CHAT_TIMEOUT_S", "4"),
            ("MEMEX_CHAT_MODEL_CALLS", "5"),
            ("MEMEX_CHAT_MAX_RUNS", "6"),
        ])
        .unwrap();
        assert_eq!(s.chat_model.as_deref(), Some("chatty"));
        assert_eq!(
            s.chat,
            crate::ChatLimits {
                output_tokens: 100,
                thread_tokens: 200,
                day_tokens: 300,
                timeout_ms: 4_000,
                max_model_calls: 5,
                max_runs: 6,
            }
        );
    }

    #[test]
    fn cache_sizes_parse_and_zero_turns_a_tier_off() {
        assert_eq!(from(&[]).unwrap().caches, CacheSettings::default());
        let s = from(&[
            ("MEMEX_CACHE_QUERY_BYTES", "0"),
            ("MEMEX_CACHE_INFERENCE_BYTES", "1048576"),
            ("MEMEX_CACHE_OBJECT_BYTES", "2097152"),
            ("MEMEX_CACHE_DISK_BYTES", "0"),
        ])
        .unwrap();
        assert_eq!(
            s.caches,
            CacheSettings {
                query_bytes: 0,
                inference_bytes: 1 << 20,
                object_bytes: 2 << 20,
                disk_bytes: 0
            }
        );
    }

    #[test]
    fn the_audit_s_llm_checks_are_off_by_default_and_need_an_llm() {
        assert_eq!(from(&[]).unwrap().audit_model, None);
        assert!(matches!(from(&[("MEMEX_AUDIT_MODEL", "m")]), Err(SettingsError::Missing { .. })));
        let s = from(&[
            ("MEMEX_LLM_PROVIDER", "openai"),
            ("MEMEX_LLM_MODEL", "m"),
            ("MEMEX_LLM_BASE_URL", "http://h/v1"),
            ("MEMEX_AUDIT_MODEL", "small"),
            ("MEMEX_AUDIT_OUTPUT_TOKENS", "700"),
            ("MEMEX_AUDIT_TIMEOUT_S", "9"),
        ])
        .unwrap();
        assert_eq!(
            (s.audit_model.as_deref(), s.audit.output_tokens, s.audit.timeout_ms),
            (Some("small"), 700, 9000)
        );
    }

    #[test]
    fn the_reviewer_is_off_by_default_and_needs_an_llm() {
        let s = from(&[]).unwrap();
        assert_eq!((s.reviewer_model.as_deref(), s.auto_commit_threshold_permille), (None, 950));
        assert!(matches!(from(&[("MEMEX_REVIEW_MODEL", "m")]), Err(SettingsError::Missing { .. })));
        let s = from(&[
            ("MEMEX_LLM_PROVIDER", "openai"),
            ("MEMEX_LLM_MODEL", "m"),
            ("MEMEX_LLM_BASE_URL", "http://h/v1"),
            ("MEMEX_REVIEW_MODEL", "small"),
            ("MEMEX_REVIEW_TIMEOUT_S", "9"),
            ("MEMEX_AUTO_COMMIT_THRESHOLD", "0.9"),
        ])
        .unwrap();
        assert_eq!(
            (s.reviewer_model.as_deref(), s.review.timeout_ms, s.auto_commit_threshold_permille),
            (Some("small"), 9000, 900)
        );
    }

    #[test]
    fn invalid_cache_sizes_are_refused_for_every_tier() {
        for var in ["QUERY", "INFERENCE", "OBJECT", "DISK"] {
            let name = format!("MEMEX_CACHE_{var}_BYTES");
            for bad in ["-1", "many", "1.5", "64MB", "1", "1048575", "99999999999999999999", "18446744073709551615"] {
                let e = from(&[(&name, bad)]).unwrap_err();
                assert!(matches!(&e, SettingsError::InvalidCacheSize { value, .. } if value == bad), "{name}={bad}: {e}");
                assert!(e.to_string().contains(&name), "{e}");
            }
            assert!(matches!(from(&[(&name, " ")]), Err(SettingsError::Empty { .. })));
        }
        let over_ram = (MAX_RAM_CACHE_BYTES + 1).to_string();
        assert!(matches!(
            from(&[("MEMEX_CACHE_QUERY_BYTES", &over_ram)]),
            Err(SettingsError::InvalidCacheSize { .. })
        ));
        assert!(from(&[("MEMEX_CACHE_DISK_BYTES", &over_ram)]).is_ok(), "the disk has a higher limit");
    }

    #[test]
    fn a_threshold_must_be_above_zero_and_at_most_one() {
        for bad in ["0", "-0.5", "1.5", "NaN", "many", "0.0001"] {
            assert!(
                matches!(from(&[("MEMEX_AUTO_COMMIT_THRESHOLD", bad)]), Err(SettingsError::InvalidThreshold { .. })),
                "{bad}"
            );
        }
        assert_eq!(from(&[("MEMEX_AUTO_COMMIT_THRESHOLD", "1")]).unwrap().auto_commit_threshold_permille, 1000);
    }

    #[test]
    fn debug_does_not_show_the_llm_key() {
        let s = from(&[
            ("MEMEX_LLM_PROVIDER", "gemini"),
            ("MEMEX_LLM_MODEL", "m"),
            ("MEMEX_LLM_API_KEY", "sekret-value"),
        ])
        .unwrap();
        assert!(!format!("{s:?}").contains("sekret-value"));
    }

    #[test]
    fn the_transcription_api_is_off_unless_named_whatever_else_is_set() {
        let key_only = from(&[
            ("MEMEX_TRANSCRIBE_API_KEY", "secret"),
            ("MEMEX_TRANSCRIBE_API_MODEL", "m"),
            ("MEMEX_TRANSCRIBE_API_BASE_URL", "http://127.0.0.1:1"),
        ])
        .unwrap();
        assert_eq!(key_only.transcribe.api, None);
        let local = from(&[
            ("MEMEX_BACKEND_TRANSCRIBER", "local"),
            ("MEMEX_TRANSCRIBE_API_KEY", "secret"),
            ("MEMEX_TRANSCRIBE_API_MODEL", "m"),
        ])
        .unwrap();
        assert_eq!(local.transcribe.api, None);
        assert_eq!(Settings::default().transcribe.api, None);
    }

    #[test]
    fn naming_the_transcription_api_needs_its_model_and_key() {
        assert!(matches!(
            from(&[("MEMEX_BACKEND_TRANSCRIBER", "gemini"), ("MEMEX_TRANSCRIBE_API_KEY", "k")]),
            Err(SettingsError::Missing {
                name: "MEMEX_TRANSCRIBE_API_MODEL",
                ..
            })
        ));
        assert!(matches!(
            from(&[("MEMEX_BACKEND_TRANSCRIBER", "gemini"), ("MEMEX_TRANSCRIBE_API_MODEL", "m")]),
            Err(SettingsError::Missing {
                name: "MEMEX_TRANSCRIBE_API_KEY",
                ..
            })
        ));
        let on = from(&[
            ("MEMEX_BACKEND_TRANSCRIBER", "gemini"),
            ("MEMEX_TRANSCRIBE_API_KEY", "k3y"),
            ("MEMEX_TRANSCRIBE_API_MODEL", "m"),
        ])
        .unwrap();
        let api = on.transcribe.api.clone().unwrap();
        assert_eq!((api.model.as_str(), api.max_tokens), ("m", 1_000_000));
        assert!(!format!("{on:?}").contains("k3y"), "the key stays out of Debug");
    }

    #[test]
    fn transcription_limits_can_be_set() {
        let s = from(&[
            ("MEMEX_TRANSCRIBE_THREADS", "2"),
            ("MEMEX_TRANSCRIBE_MAX_SECONDS", "60"),
            ("MEMEX_TRANSCRIBE_MAX_RSS_MB", "900"),
            ("MEMEX_TRANSCRIBE_STALL_S", "5"),
            ("MEMEX_ACTIVITY_MAX_STREAMS", "3"),
            ("MEMEX_ACTIVITY_STALL_S", "7"),
        ])
        .unwrap();
        assert_eq!((s.activity.max_streams, s.activity.stall_ms), (3, 7000));
        assert_eq!(
            (
                s.transcribe.threads,
                s.transcribe.max_seconds,
                s.transcribe.max_rss_mb,
                s.transcribe.stall_seconds
            ),
            (2, 60, 900, 5)
        );
        assert!(matches!(from(&[("MEMEX_TRANSCRIBE_THREADS", "0")]), Err(SettingsError::InvalidNumber { .. })));
    }
}
