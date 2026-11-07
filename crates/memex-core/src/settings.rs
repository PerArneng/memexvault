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

#[derive(Clone, PartialEq, Eq)]
pub struct Settings {
    pub database_url: Option<String>,
    pub data_dir: PathBuf,
    /// Where `models fetch` puts model files.
    pub models_dir: PathBuf,
    pub http_bind: String,
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
            backends: Backends::default(),
            embed_threads: 4,
            embed_query_sessions: 2,
            rerank_threads: 4,
            rerank_sessions: 2,
            llm: None,
            extraction: crate::ExtractionLimits::default(),
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
            .field("backends", &self.backends)
            .field("embed_threads", &self.embed_threads)
            .field("embed_query_sessions", &self.embed_query_sessions)
            .field("rerank_threads", &self.rerank_threads)
            .field("rerank_sessions", &self.rerank_sessions)
            .field("llm", &self.llm)
            .field("extraction", &self.extraction)
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
        s.embed_threads = count("MEMEX_EMBED_THREADS", text("MEMEX_EMBED_THREADS")?, s.embed_threads)?;
        s.embed_query_sessions = count("MEMEX_EMBED_QUERY_SESSIONS", text("MEMEX_EMBED_QUERY_SESSIONS")?, s.embed_query_sessions)?;
        s.rerank_threads = count("MEMEX_RERANK_THREADS", text("MEMEX_RERANK_THREADS")?, s.rerank_threads)?;
        s.rerank_sessions = count("MEMEX_RERANK_SESSIONS", text("MEMEX_RERANK_SESSIONS")?, s.rerank_sessions)?;
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
        pick("MEMEX_BACKEND_TRANSCRIBER", "Transcriber", &["local"], |b| &mut b.transcriber)?;
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
    fn debug_does_not_show_the_llm_key() {
        let s = from(&[
            ("MEMEX_LLM_PROVIDER", "gemini"),
            ("MEMEX_LLM_MODEL", "m"),
            ("MEMEX_LLM_API_KEY", "sekret-value"),
        ])
        .unwrap();
        assert!(!format!("{s:?}").contains("sekret-value"));
    }
}
