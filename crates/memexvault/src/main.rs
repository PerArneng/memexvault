//! The single composition root: the only place that reads the environment or builds an edge.
// Reading env and args and printing to the terminal are this file's job.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]
use memex_blob_fs::{FsBlobs, RealFs};
use memex_core::{SearchEdges, Server, ServerEdges, Settings};
use memex_inference::{OnnxEmbedder, OnnxReranker};
use memex_lexical::TantivyLexical;
use memex_store_postgres::{PgStore, PoolLimits};
use memex_vector::PgVectors;
use memexvault::cache_edge::TieredCaches;
use memexvault::edges::{AdkAgentRunner, FsBlobStore, PgVectorIndex, SystemClock, SystemSeed, TantivyIndex, llm_endpoint};
use memexvault::pg_edge::PostgresStore;
use memexvault::registry_edge::PostgresRegistry;
use memexvault::rendition_job::{ChildRasterizer, EXIT_UNPARSEABLE, RenditionHandler, WorkerCommand};
use memexvault::transcribe_job::{TranscribeCommand, TranscribeHandler};
use memexvault::workers::{self, AuditHandler, EmbedHandler, EraseHandler, ExtractHandler, Pool, ReviewHandler};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The default backends, the only ones so far.
struct Production;

impl ServerEdges for Production {
    type Registry = PostgresRegistry;
    type Store = PostgresStore;
    type Blobs = FsBlobStore<RealFs>;
    type Clock = SystemClock;
    type Seed = SystemSeed;
    type Rasterizer = ChildRasterizer;
    type Lexical = TantivyIndex;
    type Vector = PgVectorIndex;
    type Embedder = OnnxEmbedder;
    type Reranker = OnnxReranker;
    type Caches = TieredCaches;
}

fn fail(message: impl std::fmt::Display) -> ExitCode {
    eprintln!("memexvault: {message}");
    ExitCode::from(2)
}

fn build(settings: &Settings) -> Result<(Arc<PgStore>, Server<Production>), String> {
    let url = settings.database_url.as_deref().ok_or("MEMEX_DATABASE_URL isn't set")?;
    let limits = PoolLimits {
        max_open: settings.pg_max_connections,
        idle: settings.pg_max_connections.min(16),
        wait: Duration::from_millis(settings.pg_wait_ms),
        writers: settings.pg_writer_connections,
    };
    let pg = Arc::new(PgStore::connect_with(url, limits).map_err(|e| e.to_string())?);
    let server = Server::new(
        PostgresRegistry(pg.clone()),
        PostgresStore(pg.clone()),
        FsBlobStore(FsBlobs::new(&settings.data_dir, RealFs)),
        SystemClock::default(),
        SystemSeed,
        SearchEdges {
            lexical: TantivyIndex(TantivyLexical::new(&settings.data_dir)),
            vector: PgVectorIndex(PgVectors::new(pg.clone())),
            embedder: OnnxEmbedder::new(
                settings.models_dir.join(memexvault::models::EMBED_DIR),
                settings.embed_threads,
                settings.embed_query_sessions,
            ),
            reranker: OnnxReranker::new(
                settings.models_dir.join(memexvault::models::RERANK_DIR),
                settings.rerank_threads,
                settings.rerank_sessions,
            ),
        },
    );
    Ok((pg, server))
}

/// Parses a PDF from stdin to JSON on stdout. Run as a child process, since pdfium is C++ reading untrusted files: a crash or hang here costs one job, not the server.
fn pdf_worker() -> ExitCode {
    use std::io::{Read, Write};
    let mut bytes = vec![];
    if let Err(e) = std::io::stdin().read_to_end(&mut bytes) {
        eprintln!("can't read input: {e}");
        return ExitCode::from(EXIT_UNPARSEABLE as u8);
    }
    let parsed = memex_pdf::PdfiumParser::new().and_then(|p| p.parse(bytes));
    match parsed {
        Ok(doc) => match serde_json::to_vec(&doc).map(|json| std::io::stdout().write_all(&json)) {
            Ok(Ok(())) => ExitCode::SUCCESS,
            _ => ExitCode::from(1),
        },
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(EXIT_UNPARSEABLE as u8)
        }
    }
}

/// Loads pdfium and exits, in a child process like the parser. On Linux it's a shared library that can be missing, and a
/// worker that can't load it would fail every PDF for good, so `serve` and `mcp` run this first (spec 091).
fn pdf_check() -> ExitCode {
    match memex_pdf::PdfiumParser::new() {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}

/// Reads one PDF page by OCR: the PDF on stdin, then JSON on stdout. Threads and priority keep reads fast while this runs.
fn ocr_worker(args: &[String]) -> ExitCode {
    use std::io::{Read, Write};
    let refuse = |message: String| {
        eprintln!("{message}");
        ExitCode::from(EXIT_UNPARSEABLE as u8)
    };
    let (Some(dir), Some(Ok(page)), Some(Ok(dpi))) = (args.first(), args.get(1).map(|a| a.parse::<u32>()), args.get(2).map(|a| a.parse::<u32>())) else {
        return refuse("usage: memexvault ocr-worker <models-dir> <page> <dpi>".into());
    };
    let dir = std::path::Path::new(dir);
    if let Err(e) = memexvault::models::verify(dir, &memexvault::models::OCR_MODELS) {
        return refuse(e);
    }
    let mut bytes = vec![];
    if let Err(e) = std::io::stdin().read_to_end(&mut bytes) {
        return refuse(format!("can't read input: {e}"));
    }
    let rendered = memex_pdf::PdfiumParser::new().and_then(|p| p.render_page(bytes, page, dpi));
    let (width, height, rgb) = match rendered {
        Ok(r) => r,
        Err(e) => return refuse(e.to_string()),
    };
    let result = memex_ocr::Engine::new(dir, OCR_THREADS).and_then(|engine| engine.read(width, height, rgb, dpi));
    match result {
        Ok(out) => match serde_json::to_vec(&out).map(|json| std::io::stdout().write_all(&json)) {
            Ok(Ok(())) => ExitCode::SUCCESS,
            _ => ExitCode::from(1),
        },
        Err(e) => refuse(e),
    }
}

/// Two workers at two threads each use 4 cores, and read about as fast as one at four. Spec 016 measured reads staying within target at this setting.
const OCR_THREADS: usize = 2;

/// Renders a PDF page with highlights: the PDF on stdin, a PNG on stdout. A child process, for the same reason as `pdf_worker`.
fn render_worker(args: &[String]) -> ExitCode {
    use std::io::{Read, Write};
    let refuse = |message: String| {
        eprintln!("{message}");
        ExitCode::from(EXIT_UNPARSEABLE as u8)
    };
    let (Some(Ok(page)), Some(Ok(dpi)), Some(Ok(rects))) = (
        args.first().map(|a| a.parse::<u32>()),
        args.get(1).map(|a| a.parse::<u32>()),
        args.get(2).map(|a| serde_json::from_str::<Vec<memex_types::Rect>>(a)),
    ) else {
        return refuse("usage: memexvault render-worker <page> <dpi> <rects-json>".into());
    };
    let mut bytes = vec![];
    if let Err(e) = std::io::stdin().read_to_end(&mut bytes) {
        return refuse(format!("can't read input: {e}"));
    }
    match memex_pdf::PdfiumParser::new().and_then(|p| p.render_highlighted(bytes, page, dpi, &rects)) {
        Ok(png) => match std::io::stdout().write_all(&png) {
            Ok(()) => ExitCode::SUCCESS,
            Err(_) => ExitCode::from(1),
        },
        Err(e) => refuse(e.to_string()),
    }
}

/// Transcribes a recording in the folder named, carrying on from what the folder holds. A child process, since whisper.cpp can abort the process and the decoders read untrusted files: a crash here loses at most one stretch of speech. The API is built here only when settings name it, and nothing else can send audio anywhere.
fn transcribe_worker(args: &[String]) -> ExitCode {
    use memex_asr::worker::{self, ApiPart, WorkerFail};
    let finish = |result: Result<memex_types::TimedDocument, WorkerFail>| match result {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}", e.message());
            ExitCode::from(e.exit_code())
        }
    };
    let Some(dir) = args.first().map(std::path::Path::new) else {
        return finish(Err(WorkerFail::Refused("usage: memexvault transcribe-worker <folder> [language]".into())));
    };
    let language = args.get(1).map(String::as_str);
    let settings = match Settings::from_vars(|name| std::env::var(name).ok()) {
        Ok(s) => s,
        Err(e) => return finish(Err(WorkerFail::Refused(e.to_string()))),
    };
    if let Err(e) = memexvault::models::check_present(&settings.models_dir, &memexvault::models::ASR_MODELS) {
        return finish(Err(WorkerFail::Refused(e)));
    }
    whisper_rs::install_logging_hooks();
    let t = &settings.transcribe;
    let ears = memex_asr::whisper::WhisperEars {
        model: settings.models_dir.join(memex_asr::whisper::WHISPER_FILE),
        vad: settings.models_dir.join(memex_asr::whisper::VAD_FILE),
        threads: t.threads,
    };
    let parser = format!("whisper-rs-0.16.0+{}+silero-v6.2.0", memex_asr::whisper::MODEL);
    let api = t.api.as_ref().map(|a| memex_asr::GeminiApi {
        key: a.api_key.clone(),
        model: a.model.clone(),
        base_url: a.base_url.clone(),
        retry_after: Duration::from_secs(10),
    });
    let part = api.as_ref().zip(t.api.as_ref()).map(|(transcriber, a)| ApiPart {
        transcriber,
        max_tokens: a.max_tokens,
    });
    finish(worker::run(dir, &ears, part, language, t.max_seconds, &parser))
}

/// `models fetch` and `models check`.
fn models_command(args: &[String], settings: &Settings) -> ExitCode {
    let dir = &settings.models_dir;
    let result = match args.first().map(String::as_str) {
        Some("fetch") => memexvault::models::fetch(dir, &memexvault::models::OCR_MODELS)
            .and_then(|()| memexvault::models::fetch(dir, &memexvault::models::EMBED_MODELS))
            .and_then(|()| memexvault::models::fetch(dir, &memexvault::models::RERANK_MODELS))
            .and_then(|()| memexvault::models::fetch(dir, &memexvault::models::ASR_MODELS)),
        Some("check") => memexvault::models::verify(dir, &memexvault::models::OCR_MODELS)
            .and_then(|()| memexvault::models::verify(dir, &memexvault::models::EMBED_MODELS))
            .and_then(|()| memexvault::models::verify(dir, &memexvault::models::RERANK_MODELS))
            .and_then(|()| memexvault::models::verify(dir, &memexvault::models::ASR_MODELS)),
        _ => Err("usage: memexvault models fetch | check".to_string()),
    };
    match result {
        Ok(()) => {
            println!("models are in {}", dir.display());
            ExitCode::SUCCESS
        }
        Err(e) => fail(e),
    }
}

/// `backup`, `restore` and `reconcile`, which need nothing but the server's own edges. Backup and reconcile take no lease, so they run beside `serve`. Restore takes the new vault's lease, which a serving server doesn't hold yet.
fn ops_command(server: &Server<Production>, command: &str, args: &[String]) -> ExitCode {
    let flag = |name: &str| args.iter().any(|a| a == name);
    let value = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let positional: Vec<&String> = {
        let mut out = vec![];
        let mut skip = false;
        for a in args {
            if skip {
                skip = false;
            } else if a == "--name" || a == "--at" {
                skip = true;
            } else if !a.starts_with("--") {
                out.push(a);
            }
        }
        out
    };
    match command {
        "backup" => {
            let (Some(vault), Some(path)) = (positional.first(), positional.get(1)) else {
                return fail("usage: memexvault backup <vault> <file>");
            };
            let path = std::path::Path::new(path.as_str());
            if path.exists() {
                return fail(format!("{} exists, and a backup never overwrites a file", path.display()));
            }
            // Written beside its final name and renamed when whole, so a file under that name is always a complete backup.
            let partial = path.with_extension("partial");
            let written = (|| -> Result<_, String> {
                let file = std::fs::File::create_new(&partial).map_err(|e| format!("{}: {e}", partial.display()))?;
                let mut out = std::io::BufWriter::new(file);
                let result = server
                    .backup_vault(&memex_types::VaultId(vault.to_string()), &mut out)
                    .map_err(|e| e.to_string())?;
                let file = out.into_inner().map_err(|e| e.to_string())?;
                file.sync_all().map_err(|e| e.to_string())?;
                std::fs::rename(&partial, path).map_err(|e| e.to_string())?;
                Ok(result)
            })();
            match written {
                Ok((header, summary)) => {
                    println!(
                        "backed up vault {} at event {}: {} events, {} blobs ({} bytes), {} embeddings, {} bytes, sha256 {}, tables {}",
                        header.vault_id,
                        header.head.map_or("none".into(), |h| h.to_string()),
                        summary.events,
                        summary.blobs,
                        summary.blob_bytes,
                        summary.embeddings,
                        summary.bytes,
                        summary.sha256,
                        header.dump_sha256
                    );
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&partial);
                    fail(e)
                }
            }
        }
        "restore" => {
            let Some(path) = positional.first().map(|p| std::path::PathBuf::from(p.as_str())) else {
                return fail("usage: memexvault restore <file> [--new-id | --replace] [--name <name>]");
            };
            let target = match (flag("--new-id"), flag("--replace")) {
                (true, true) => return fail("--new-id and --replace can't go together"),
                (true, false) => memex_core::RestoreTarget::NewId,
                (false, true) => memex_core::RestoreTarget::Replace,
                (false, false) => memex_core::RestoreTarget::Original,
            };
            let name = value("--name");
            let mut open = || -> std::io::Result<Box<dyn std::io::Read>> { Ok(Box::new(std::fs::File::open(&path)?)) };
            match server.restore_vault("operator", &mut open, target, name.as_deref()) {
                Ok((row, summary)) => {
                    println!(
                        "restored vault {} ({}): {} events, {} blobs, {} embeddings, tables {}. Its indexes build when `serve` next starts",
                        row.id, row.name, summary.events, summary.blobs, summary.embeddings, summary.sha256
                    );
                    ExitCode::SUCCESS
                }
                Err(e) => fail(e),
            }
        }
        _ => {
            let Some(vault) = positional.first() else {
                return fail("usage: memexvault reconcile <vault> [--at <seq>] [--deep]");
            };
            let at = match value("--at").map(|a| a.parse::<u64>()) {
                None => None,
                Some(Ok(a)) => Some(a),
                Some(Err(_)) => return fail("--at takes an event's sequence number"),
            };
            match server.reconcile_store(&memex_types::VaultId(vault.to_string()), at, flag("--deep")) {
                Ok(r) => {
                    let log = &r.log;
                    println!("head {}", log.head.map_or("none".into(), |h| h.to_string()));
                    println!("events {} rows {}", log.events, log.rows);
                    println!("tables {}", log.tables_sha256);
                    println!("replayed {}", log.replayed_sha256);
                    if let Some((seq, sha)) = &log.at {
                        println!("at {seq} {sha}");
                    }
                    if let Some(e) = &log.replay_error {
                        println!("replay failed: {e}");
                    }
                    println!("differing rows {}", log.differing_count);
                    for k in &log.differing {
                        println!("  {:?} {}", k.table, k.id);
                    }
                    let b = &r.blobs;
                    println!(
                        "blobs referenced {} stored {} ({} bytes) missing {} orphans {} corrupt {}",
                        b.referenced,
                        b.stored,
                        b.stored_bytes,
                        b.missing.len(),
                        b.orphans,
                        b.corrupt.len()
                    );
                    for h in b.missing.iter().chain(&b.corrupt) {
                        println!("  {h}");
                    }
                    println!("indexes: ask the server, at POST /api/v1/vaults/{vault}/reconcile");
                    if r.clean() {
                        println!("no differences");
                        ExitCode::SUCCESS
                    } else {
                        println!("differences found");
                        ExitCode::from(1)
                    }
                }
                Err(e) => fail(e),
            }
        }
    }
}

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("pdf-worker") => return pdf_worker(),
        Some("pdf-check") => return pdf_check(),
        Some("ocr-worker") => return ocr_worker(&std::env::args().skip(2).collect::<Vec<_>>()),
        Some("render-worker") => return render_worker(&std::env::args().skip(2).collect::<Vec<_>>()),
        Some("transcribe-worker") => return transcribe_worker(&std::env::args().skip(2).collect::<Vec<_>>()),
        _ => {}
    }
    tracing_subscriber::fmt().with_writer(std::io::stderr).init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map_or("", String::as_str);
    if command == "--version" || command == "-V" {
        println!("memexvault {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    let settings = match Settings::from_vars(|name| std::env::var(name).ok()) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    if command == "models" {
        return models_command(&args[1..], &settings);
    }
    if !matches!(command, "migrate" | "bootstrap" | "serve" | "mcp" | "backup" | "restore" | "reconcile") {
        return fail(
            "usage: memexvault migrate | bootstrap <name> | serve | mcp | backup <vault> <file> | restore <file> [--new-id | --replace] [--name <name>] | reconcile <vault> [--at <seq>] [--deep] | models fetch | models check | --version",
        );
    }
    let (pg, server) = match build(&settings) {
        Ok(b) => b,
        Err(e) => return fail(e),
    };
    match command {
        "migrate" => match pg.migrate() {
            Ok(()) => {
                println!("schema is up to date");
                ExitCode::SUCCESS
            }
            Err(e) => fail(e),
        },
        "backup" | "restore" | "reconcile" => {
            if let Err(e) = pg.check_server() {
                return fail(e);
            }
            ops_command(&server, command, &args[1..])
        }
        "bootstrap" => {
            let name = args.get(1).map_or("admin", String::as_str);
            if let Err(e) = pg.check_server() {
                return fail(e);
            }
            match server.bootstrap(name) {
                Ok(created) => {
                    // Shown once, to the terminal that asked. It isn't a log line.
                    println!("{}", created.token);
                    ExitCode::SUCCESS
                }
                Err(e) => fail(e),
            }
        }
        _ => {
            if let Err(e) = pg.check_server() {
                return fail(e);
            }
            // The MCP stdio server is this whole server, for one local client. A vault has one writer, so it can't run beside `serve` on the same vaults.
            let stdio_token = if command == "mcp" {
                let Some(token) = settings.mcp_token.clone() else {
                    return fail("memexvault mcp needs MEMEX_MCP_TOKEN");
                };
                if let Err(e) = server.authenticate(&token) {
                    return fail(format!("MEMEX_MCP_TOKEN: {e}"));
                }
                Some(token)
            } else {
                None
            };
            match server.open_vaults() {
                Ok(n) => tracing::info!(vaults = n, "writer leases held"),
                Err(e) if stdio_token.is_some() => return fail(format!("{e}. Is `memexvault serve` running? If so, use MCP over its HTTP route instead")),
                Err(e) => return fail(e),
            }
            let caches = match TieredCaches::open(settings.caches, settings.data_dir.join("vaults")) {
                Ok(c) => c,
                Err(e) => return fail(e),
            };
            let server = Arc::new(server.with_caches(caches));
            let program = match std::env::current_exe() {
                Ok(p) => p,
                Err(e) => return fail(e),
            };
            match std::process::Command::new(&program).arg("pdf-check").output() {
                Ok(out) if out.status.success() => {}
                Ok(out) => return fail(String::from_utf8_lossy(&out.stderr).trim()),
                Err(e) => return fail(format!("can't run the pdfium check: {e}")),
            }
            if let Err(e) = memexvault::models::check_present(&settings.models_dir, &memexvault::models::OCR_MODELS) {
                return fail(e);
            }
            if let Err(e) = memexvault::models::check_present(&settings.models_dir, &memexvault::models::EMBED_MODELS) {
                return fail(e);
            }
            if let Err(e) = server.check_embedding_models() {
                return fail(e);
            }
            if let Err(e) = server.embedder().load() {
                return fail(e);
            }
            let worker = WorkerCommand {
                program: program.clone(),
                args: vec!["pdf-worker".into()],
                timeout: Duration::from_secs(300),
            };
            // Lower priority, so OCR takes what reads leave over.
            let ocr = WorkerCommand {
                program: "nice".into(),
                args: vec![
                    "-n".into(),
                    "10".into(),
                    program.to_string_lossy().into_owned(),
                    "ocr-worker".into(),
                    settings.models_dir.to_string_lossy().into_owned(),
                ],
                timeout: Duration::from_secs(120),
            };
            let handler = RenditionHandler {
                server: server.clone(),
                worker,
                ocr: Some(ocr),
            };
            server.set_auto_commit_threshold(f64::from(settings.auto_commit_threshold_permille) / 1000.0);
            server.set_activity_limits(settings.activity);
            server.set_sweep(settings.sweep);
            let stop = Arc::new(AtomicBool::new(false));
            let transcribe = TranscribeHandler {
                server: server.clone(),
                command: TranscribeCommand {
                    // Lower priority, so transcribing takes what reads leave over.
                    program: "nice".into(),
                    args: vec!["-n".into(), "10".into(), program.to_string_lossy().into_owned(), "transcribe-worker".into()],
                    envs: vec![],
                    // Off means the worker never even sees the key.
                    env_remove: if settings.transcribe.api.is_none() {
                        vec!["MEMEX_TRANSCRIBE_API_KEY".into()]
                    } else {
                        vec![]
                    },
                    max_rss_mb: settings.transcribe.max_rss_mb,
                    stall: Duration::from_secs(settings.transcribe.stall_seconds),
                },
                root: settings.data_dir.join("vaults"),
                stop: stop.clone(),
            };
            if settings.transcribe.api.is_some() {
                tracing::info!("the transcription API is on, so recordings go to it");
            }
            // One embedding worker: its CPU use is capped by the session's threads, and identical text can't be embedded twice at once.
            let mut pools = vec![
                Pool {
                    kind: memex_core::RENDITION_JOB.into(),
                    workers: 2,
                    handler: Arc::new(handler),
                    ceiling: settings.job_ceilings.for_kind(memex_core::RENDITION_JOB),
                },
                // One at a time, since each holds a large model in memory.
                Pool {
                    kind: memex_core::TRANSCRIBE_JOB.into(),
                    workers: 1,
                    handler: Arc::new(transcribe),
                    ceiling: settings.job_ceilings.for_kind(memex_core::TRANSCRIBE_JOB),
                },
                Pool {
                    kind: memex_core::EMBED_JOB.into(),
                    workers: 1,
                    handler: Arc::new(EmbedHandler { server: server.clone() }),
                    ceiling: settings.job_ceilings.for_kind(memex_core::EMBED_JOB),
                },
            ];
            // One at a time, as each compacts the vault's whole store.
            pools.push(Pool {
                kind: memex_types::ERASE_JOB.into(),
                workers: 1,
                handler: Arc::new(EraseHandler { server: server.clone() }),
                ceiling: settings.job_ceilings.for_kind(memex_types::ERASE_JOB),
            });
            // One at a time per server, as each reads or rewrites a whole vault.
            let archives = Arc::new(memexvault::admin_edge::FsArchives::new(&settings.data_dir));
            pools.push(Pool {
                kind: memex_types::OPERATION_JOB.into(),
                workers: 1,
                handler: Arc::new(memexvault::workers::OperationHandler {
                    server: server.clone(),
                    archives: archives.clone(),
                }),
                ceiling: settings.job_ceilings.for_kind(memex_types::OPERATION_JOB),
            });
            // Audits run with no LLM too, doing what costs no tokens. Their LLM checks are asked for by name, as each sampled claim is two calls.
            let auditor = match (&settings.llm, &settings.audit_model) {
                (Some(llm), Some(model)) => {
                    let audit = memex_core::LlmSettings {
                        model: model.clone(),
                        ..llm.clone()
                    };
                    match llm_endpoint(&audit).and_then(memex_agent_adk::AdkAgent::new) {
                        Ok(a) => {
                            server.set_auditor(Some(settings.audit));
                            tracing::info!(model = %model, "the audit agent's LLM checks are on");
                            Some(AdkAgentRunner(a))
                        }
                        Err(e) => return fail(e),
                    }
                }
                _ => None,
            };
            pools.push(Pool {
                kind: memex_core::AUDIT_JOB.into(),
                workers: 1,
                handler: Arc::new(AuditHandler {
                    server: server.clone(),
                    agent: auditor,
                }),
                ceiling: settings.job_ceilings.for_kind(memex_core::AUDIT_JOB),
            });
            // Only with an LLM set up does anything leave this server.
            if let Some(llm) = &settings.llm {
                let agent = match llm_endpoint(llm).and_then(memex_agent_adk::AdkAgent::new) {
                    Ok(a) => AdkAgentRunner(a),
                    Err(e) => return fail(e),
                };
                server.set_extraction(Some(settings.extraction));
                tracing::info!(provider = %llm.provider, model = %llm.model, "extraction is on");
                // One at a time per server, so a document's budget and the provider's rate limit are easy to reason about.
                pools.push(Pool {
                    kind: memex_core::EXTRACT_JOB.into(),
                    workers: 1,
                    handler: Arc::new(ExtractHandler { server: server.clone(), agent }),
                    ceiling: settings.job_ceilings.for_kind(memex_core::EXTRACT_JOB),
                });
                // Each queued claim is a call, so this is asked for by name. It sees claims and quotes only, and nothing it says can commit anything.
                if let Some(model) = &settings.reviewer_model {
                    let reviewer = memex_core::LlmSettings {
                        model: model.clone(),
                        ..llm.clone()
                    };
                    let agent = match llm_endpoint(&reviewer).and_then(memex_agent_adk::AdkAgent::new) {
                        Ok(a) => AdkAgentRunner(a),
                        Err(e) => return fail(e),
                    };
                    server.set_reviewer(Some(settings.review));
                    tracing::info!(model = %model, "the reviewer agent is on");
                    pools.push(Pool {
                        kind: memex_core::REVIEW_JOB.into(),
                        workers: 1,
                        handler: Arc::new(ReviewHandler { server: server.clone(), agent }),
                        ceiling: settings.job_ceilings.for_kind(memex_core::REVIEW_JOB),
                    });
                }
            }
            // Asked for by name, like the reviewer. A vault's content goes to this model only once an admin of that vault has turned chat on.
            let chat_agent = match (&settings.llm, &settings.chat_model) {
                (Some(llm), Some(model)) => {
                    let chat = memex_core::LlmSettings {
                        model: model.clone(),
                        ..llm.clone()
                    };
                    match llm_endpoint(&chat).and_then(memex_agent_adk::AdkAgent::new) {
                        Ok(a) => {
                            server.set_chat(Some(settings.chat));
                            let endpoint = match chat.provider.as_str() {
                                "gemini" => Some(memex_core::chat::GEMINI_ENDPOINT.to_string()),
                                _ => chat.base_url.as_deref().and_then(memex_core::chat::endpoint_origin),
                            };
                            server.set_chat_model(Some(memex_core::chat::ChatModel {
                                model: model.clone(),
                                endpoint: endpoint.unwrap_or_else(|| "unknown".into()),
                            }));
                            tracing::info!(model = %model, "the chat agent is available");
                            Some(Arc::new(AdkAgentRunner(a)))
                        }
                        Err(e) => return fail(e),
                    }
                }
                _ => None,
            };
            let mut threads = workers::start(server.clone(), pools, stop.clone());
            threads.push(workers::start_audit_scheduler(server.clone(), stop.clone()));
            threads.push(workers::start_sweeper(server.clone(), stop.clone(), settings.sweep));
            threads.push(workers::start_projector(server.clone(), stop.clone()));
            threads.push(workers::start_vector_projector(server.clone(), stop.clone()));
            let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
                Ok(r) => r,
                Err(e) => return fail(e),
            };
            let shutdown = async {
                let _ = tokio::signal::ctrl_c().await;
            };
            let served = match stdio_token {
                Some(token) => runtime.block_on(async {
                    tokio::select! {
                        done = memex_mcp::serve_stdio(server, token) => done.map_err(std::io::Error::other),
                        () = shutdown => Ok(()),
                    }
                }),
                None => {
                    let mcp = memex_mcp::routes(
                        server.clone(),
                        memex_mcp::McpHttp {
                            allowed_hosts: settings.mcp_allowed_hosts.clone(),
                            max_response_bytes: memex_mcp::DEFAULT_MAX_RESPONSE_BYTES,
                        },
                    );
                    let admin = memex_http::admin_routes(memex_http::AdminConfig {
                        archives,
                        settings: settings.effective(),
                        prices: settings.llm_prices.clone(),
                        llm_endpoint: settings.llm.as_ref().and_then(memex_core::llm_probe_target),
                        models: memexvault::admin_edge::model_status(&settings),
                        mcp: memex_mcp::connection_info(&settings.mcp_allowed_hosts),
                        memory: memexvault::admin_edge::resident_bytes,
                        abandoned: memexvault::workers::abandoned_handlers,
                    });
                    let extra = mcp.merge(memex_http::chat_routes(chat_agent)).merge(admin);
                    runtime.block_on(memex_http::serve_with(server, extra, &settings.http_bind, shutdown))
                }
            };
            stop.store(true, Ordering::Relaxed);
            for t in threads {
                let _ = t.join();
            }
            match served {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => fail(e),
            }
        }
    }
}
