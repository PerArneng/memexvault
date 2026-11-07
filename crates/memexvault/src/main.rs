//! The single composition root: the only place that reads the environment or builds an edge.
// Reading env and args and printing to the terminal are this file's job.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]
use memex_blob_fs::{FsBlobs, RealFs};
use memex_core::{SearchEdges, Server, ServerEdges, Settings};
use memex_inference::{OnnxEmbedder, OnnxReranker};
use memex_lexical::TantivyLexical;
use memex_store_postgres::PgStore;
use memex_vector::PgVectors;
use memexvault::edges::{AdkAgentRunner, FsBlobStore, PgVectorIndex, SystemClock, SystemSeed, TantivyIndex, llm_endpoint};
use memexvault::pg_edge::PostgresStore;
use memexvault::registry_edge::PostgresRegistry;
use memexvault::rendition_job::{ChildRasterizer, EXIT_UNPARSEABLE, RenditionHandler, WorkerCommand};
use memexvault::workers::{self, EmbedHandler, ExtractHandler, Pool};
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
}

fn fail(message: impl std::fmt::Display) -> ExitCode {
    eprintln!("memexvault: {message}");
    ExitCode::from(2)
}

fn build(settings: &Settings) -> Result<(Arc<PgStore>, Server<Production>), String> {
    let url = settings.database_url.as_deref().ok_or("MEMEX_DATABASE_URL isn't set")?;
    let pg = Arc::new(PgStore::connect(url, 16).map_err(|e| e.to_string())?);
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

/// `models fetch` and `models check`.
fn models_command(args: &[String], settings: &Settings) -> ExitCode {
    let dir = &settings.models_dir;
    let result = match args.first().map(String::as_str) {
        Some("fetch") => memexvault::models::fetch(dir, &memexvault::models::OCR_MODELS)
            .and_then(|()| memexvault::models::fetch(dir, &memexvault::models::EMBED_MODELS))
            .and_then(|()| memexvault::models::fetch(dir, &memexvault::models::RERANK_MODELS)),
        Some("check") => memexvault::models::verify(dir, &memexvault::models::OCR_MODELS)
            .and_then(|()| memexvault::models::verify(dir, &memexvault::models::EMBED_MODELS))
            .and_then(|()| memexvault::models::verify(dir, &memexvault::models::RERANK_MODELS)),
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

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("pdf-worker") => return pdf_worker(),
        Some("ocr-worker") => return ocr_worker(&std::env::args().skip(2).collect::<Vec<_>>()),
        Some("render-worker") => return render_worker(&std::env::args().skip(2).collect::<Vec<_>>()),
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
    if !matches!(command, "migrate" | "bootstrap" | "serve") {
        return fail("usage: memexvault migrate | bootstrap <name> | serve | models fetch | models check | --version");
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
            match server.open_vaults() {
                Ok(n) => tracing::info!(vaults = n, "writer leases held"),
                Err(e) => return fail(e),
            }
            let server = Arc::new(server);
            let program = match std::env::current_exe() {
                Ok(p) => p,
                Err(e) => return fail(e),
            };
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
            let stop = Arc::new(AtomicBool::new(false));
            // One embedding worker: its CPU use is capped by the session's threads, and identical text can't be embedded twice at once.
            let mut pools = vec![
                Pool {
                    kind: memex_core::RENDITION_JOB.into(),
                    workers: 2,
                    handler: Arc::new(handler),
                },
                Pool {
                    kind: memex_core::EMBED_JOB.into(),
                    workers: 1,
                    handler: Arc::new(EmbedHandler { server: server.clone() }),
                },
            ];
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
                });
            }
            let mut threads = workers::start(server.clone(), pools, stop.clone());
            threads.push(workers::start_projector(server.clone(), stop.clone()));
            threads.push(workers::start_vector_projector(server.clone(), stop.clone()));
            let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
                Ok(r) => r,
                Err(e) => return fail(e),
            };
            let shutdown = async {
                let _ = tokio::signal::ctrl_c().await;
            };
            let served = runtime.block_on(memex_http::serve(server, &settings.http_bind, shutdown));
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
