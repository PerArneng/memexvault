//! The edges the admin routes need (spec 039): archives in a folder, the process's memory, and what the status says about models.
// This module reads and writes archive files and `/proc`.
#![allow(clippy::disallowed_methods)]
use memex_core::{ArchiveStore, ServerError, Settings};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Archives under `<data>/archives`, by name. A name is written beside itself and renamed when whole, so it's only ever a complete archive.
pub struct FsArchives {
    root: PathBuf,
}

impl FsArchives {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join("archives"),
        }
    }

    /// Names come from ids the server made, but a name that could leave the folder is refused anyway.
    fn path(&self, name: &str) -> Result<PathBuf, ServerError> {
        let safe = !name.is_empty()
            && name
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != ".." && part.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')));
        if !safe {
            return Err(ServerError::Invalid(format!("'{name}' isn't an archive name")));
        }
        Ok(self.root.join(name))
    }
}

fn io(e: std::io::Error) -> ServerError {
    ServerError::Unavailable(format!("archive storage: {e}"))
}

impl ArchiveStore for FsArchives {
    fn put(&self, name: &str, write: &mut dyn FnMut(&mut dyn Write) -> Result<(), ServerError>) -> Result<u64, ServerError> {
        let path = self.path(name)?;
        let dir = path.parent().ok_or_else(|| ServerError::Invalid(format!("'{name}' isn't an archive name")))?;
        std::fs::create_dir_all(dir).map_err(io)?;
        let partial = path.with_extension("partial");
        let written = (|| -> Result<u64, ServerError> {
            let file = std::fs::File::create(&partial).map_err(io)?;
            let mut out = std::io::BufWriter::new(file);
            write(&mut out)?;
            let file = out.into_inner().map_err(|e| io(e.into_error()))?;
            file.sync_all().map_err(io)?;
            let size = file.metadata().map_err(io)?.len();
            std::fs::rename(&partial, &path).map_err(io)?;
            if let Ok(d) = std::fs::File::open(dir) {
                let _ = d.sync_all();
            }
            Ok(size)
        })();
        if written.is_err() {
            let _ = std::fs::remove_file(&partial);
        }
        written
    }

    fn open(&self, name: &str) -> Result<Option<memex_core::ArchiveRead>, ServerError> {
        let path = self.path(name)?;
        match std::fs::File::open(&path) {
            Ok(f) => {
                let size = f.metadata().map_err(io)?.len();
                Ok(Some((size, Box::new(std::io::BufReader::new(f)))))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io(e)),
        }
    }

    fn remove(&self, name: &str) -> Result<(), ServerError> {
        match std::fs::remove_file(self.path(name)?) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io(e)),
        }
    }
}

/// Resident memory from `/proc/self/statm`, on Linux. None elsewhere.
pub fn resident_bytes() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = text.split_whitespace().nth(1)?.parse().ok()?;
    // Every Linux the binary is built for uses 4 KiB pages.
    Some(pages * 4096)
}

/// Each model's role and whether it's ready, for the status route. Read once at startup, as the files don't change under a running server.
pub fn model_status(settings: &Settings) -> Vec<memex_api::ModelStatusDto> {
    let files = |models: &[crate::models::ModelFile]| {
        if crate::models::check_present(&settings.models_dir, models).is_ok() {
            "ready"
        } else {
            "missing"
        }
    };
    let llm = |model: Option<&String>, role: &str| memex_api::ModelStatusDto {
        role: role.into(),
        model: model.cloned(),
        status: if model.is_some() { "on" } else { "off" }.into(),
    };
    vec![
        memex_api::ModelStatusDto {
            role: "embedder".into(),
            model: Some(crate::models::EMBED_DIR.into()),
            status: files(&crate::models::EMBED_MODELS).into(),
        },
        memex_api::ModelStatusDto {
            role: "reranker".into(),
            model: Some(crate::models::RERANK_DIR.into()),
            status: files(&crate::models::RERANK_MODELS).into(),
        },
        memex_api::ModelStatusDto {
            role: "ocr".into(),
            model: Some("pp-ocr".into()),
            status: files(&crate::models::OCR_MODELS).into(),
        },
        memex_api::ModelStatusDto {
            role: "transcriber".into(),
            model: Some(match &settings.transcribe.api {
                Some(api) => api.model.clone(),
                None => "whisper".into(),
            }),
            status: if settings.transcribe.api.is_some() {
                "on".into()
            } else {
                files(&crate::models::ASR_MODELS).into()
            },
        },
        llm(settings.llm.as_ref().map(|l| &l.model), "extraction"),
        llm(settings.reviewer_model.as_ref(), "review"),
        llm(settings.audit_model.as_ref(), "audit"),
        llm(settings.chat_model.as_ref(), "chat"),
    ]
}
