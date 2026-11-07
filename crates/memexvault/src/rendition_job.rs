//! The rendition job: reads a source, parses it in a child process when it's a PDF, and hands the rows back to be committed with the job.
// This module spawns and waits on child processes, which needs wall time.
#![allow(clippy::disallowed_methods)]
use crate::workers::{JobFailure, JobHandler};
use memex_core::ocr::{self, OcrPage};
use memex_core::{RenditionInput, Server, ServerEdges, ServerError};
use memex_types::{JobRow, Mutation, ObjectId, PageOutcome, PageRoute, ParsedDocument, VaultId, Word};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The largest file a job reads into memory. Uploads above it stay stored, and their job fails with this reason.
pub const MAX_PARSE_BYTES: u64 = 512 * 1024 * 1024;
/// A worker that prints nothing else exits with this after refusing a file, with the reason on stderr.
pub const EXIT_UNPARSEABLE: i32 = 3;

/// How to start the parser process. In production it's this same binary.
#[derive(Debug, Clone)]
pub struct WorkerCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub timeout: Duration,
}

fn permanent(message: impl Into<String>) -> JobFailure {
    JobFailure {
        message: message.into(),
        retryable: false,
        waiting: false,
    }
}

/// Runs the parser with the bytes on stdin. A crash, a hang or nonsense output fails this one job, and the server carries on.
pub fn run_worker(command: &WorkerCommand, bytes: Vec<u8>) -> Result<ParsedDocument, JobFailure> {
    run_child(command, bytes)
}

/// Runs any worker whose stdout is JSON of type `T`.
pub fn run_any_child<T: serde::de::DeserializeOwned>(command: &WorkerCommand, bytes: Vec<u8>) -> Result<T, JobFailure> {
    run_child(command, bytes)
}

fn run_child<T: serde::de::DeserializeOwned>(command: &WorkerCommand, bytes: Vec<u8>) -> Result<T, JobFailure> {
    let stdout = run_child_raw(command, bytes)?;
    serde_json::from_slice(&stdout).map_err(|e| permanent(format!("the parser's output isn't valid: {e}")))
}

fn run_child_raw(command: &WorkerCommand, bytes: Vec<u8>) -> Result<Vec<u8>, JobFailure> {
    let mut child = Command::new(&command.program)
        .args(&command.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| JobFailure {
            message: format!("can't start the parser: {e}"),
            retryable: true,
            waiting: false,
        })?;
    let mut stdin = child.stdin.take().expect("piped");
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    // Threads on each pipe, so a child that fills one can't deadlock with us. A write error just means it exited early.
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&bytes);
    });
    let out = std::thread::spawn(move || {
        let mut v = vec![];
        let _ = stdout.read_to_end(&mut v);
        v
    });
    let err = std::thread::spawn(move || {
        let mut v = vec![];
        let _ = stderr.read_to_end(&mut v);
        v
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= command.timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(permanent(format!("the parser ran past its limit of {} s", command.timeout.as_secs())));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => {
                return Err(JobFailure {
                    message: format!("can't wait for the parser: {e}"),
                    retryable: true,
                    waiting: false,
                });
            }
        }
    };
    let _ = writer.join();
    let stdout = out.join().unwrap_or_default();
    let stderr = err.join().unwrap_or_default();
    let message = String::from_utf8_lossy(&stderr).chars().take(300).collect::<String>();
    match status.code() {
        Some(0) => Ok(stdout),
        Some(EXIT_UNPARSEABLE) => Err(permanent(message.trim().to_string())),
        Some(code) => Err(permanent(format!("the parser exited with {code}: {}", message.trim()))),
        None => Err(permanent("the parser crashed")),
    }
}

/// One OCR child: `args` plus the page number and resolution, with the PDF on stdin.
fn run_ocr_worker(command: &WorkerCommand, pdf: &[u8], page: u32, dpi: u32) -> Result<OcrPage, JobFailure> {
    let mut c = command.clone();
    c.args.push(page.to_string());
    c.args.push(dpi.to_string());
    run_child(&c, pdf.to_vec())
}

/// Reads each page that needs it, one child per page, so a failure costs a page and not the document. Words go in page order.
pub fn add_ocr(doc: &mut ParsedDocument, pdf: &[u8], ocr: Option<&WorkerCommand>) -> Vec<PageOutcome> {
    let routes = ocr::route_pages(doc);
    let checks = ocr::hidden_check_pages(doc, &routes);
    let mut outcomes = vec![];
    let mut words = vec![];
    for (facts, route) in doc.pages.clone().iter().zip(&routes) {
        let layer: Vec<Word> = doc.words.iter().filter(|w| w.page == facts.index).cloned().collect();
        let refs: Vec<&Word> = layer.iter().collect();
        let wanted = *route == PageRoute::Ocr || checks.contains(&facts.index);
        let result = match (wanted, ocr) {
            (true, Some(c)) => Some(run_ocr_worker(c, pdf, facts.index, ocr::dpi_for(facts.width, facts.height)).map(|p| ocr::words_from_ocr(facts.index, &p))),
            (true, None) if *route == PageRoute::Ocr => Some(Err(JobFailure {
                message: "OCR isn't set up, so run: memexvault models fetch".into(),
                retryable: false,
                waiting: false,
            })),
            (true, None) => None,
            (false, _) => None,
        };
        let (seen, failure) = match &result {
            Some(Ok(w)) => (Some(w.as_slice()), None),
            Some(Err(f)) => (None, Some(f.message.as_str())),
            None => (None, None),
        };
        outcomes.push(ocr::page_outcome(facts.index, *route, &refs, seen, failure));
        // A garbled layer gives way to what OCR read, when it could read anything.
        match (route, seen) {
            (PageRoute::Ocr, Some(w)) => words.extend_from_slice(w),
            (PageRoute::Ocr, None) => {}
            _ => words.extend(layer),
        }
    }
    doc.words = words;
    outcomes
}

/// Renders pages in a child process with a time limit, since pdfium reads untrusted files.
pub struct ChildRasterizer {
    pub program: PathBuf,
    pub timeout: Duration,
}

impl Default for ChildRasterizer {
    fn default() -> Self {
        Self {
            program: std::env::var_os("MEMEX_WORKER_PROGRAM")
                .map(PathBuf::from)
                .or_else(|| std::env::current_exe().ok())
                .unwrap_or_default(),
            timeout: Duration::from_secs(60),
        }
    }
}

impl memex_core::Rasterizer for ChildRasterizer {
    fn renderer(&self) -> String {
        memex_pdf::PARSER.into()
    }

    fn render(&self, pdf: &[u8], page: u32, dpi: u32, highlights: &[memex_types::Rect]) -> Result<memex_core::PageImage, memex_core::RasterError> {
        let rects = serde_json::to_string(highlights).map_err(|e| memex_core::RasterError(e.to_string()))?;
        let command = WorkerCommand {
            program: self.program.clone(),
            args: vec!["render-worker".into(), page.to_string(), dpi.to_string(), rects],
            timeout: self.timeout,
        };
        let png = run_child_raw(&command, pdf.to_vec()).map_err(|f| memex_core::RasterError(f.message))?;
        // The PNG header holds the size, at fixed offsets.
        if png.len() < 24 || &png[..8] != b"\x89PNG\r\n\x1a\n" {
            return Err(memex_core::RasterError("the renderer's output isn't a PNG".into()));
        }
        let be = |at: usize| u32::from_be_bytes([png[at], png[at + 1], png[at + 2], png[at + 3]]);
        let (width, height) = (be(16), be(20));
        Ok(memex_core::PageImage { png, width, height })
    }
}

pub struct RenditionHandler<E: ServerEdges> {
    pub server: Arc<Server<E>>,
    pub worker: WorkerCommand,
    /// How to start an OCR child, once `models fetch` has been run.
    pub ocr: Option<WorkerCommand>,
}

fn from_server(e: ServerError) -> JobFailure {
    let retryable = matches!(e, ServerError::Unavailable(_));
    JobFailure {
        message: e.to_string(),
        retryable,
        waiting: false,
    }
}

impl<E> JobHandler for RenditionHandler<E>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    fn run(&self, vault: &VaultId, job: &JobRow) -> Result<Vec<Mutation>, JobFailure> {
        let source_id = job
            .input
            .get("source_id")
            .and_then(|v| v.as_str())
            .map(|s| ObjectId(s.into()))
            .ok_or_else(|| permanent("the job has no source"))?;
        let (source, bytes) = self.server.read_source_bytes(vault, &source_id, MAX_PARSE_BYTES).map_err(from_server)?;
        if bytes.len() as u64 >= MAX_PARSE_BYTES {
            return Err(permanent(format!("over the parse limit of {MAX_PARSE_BYTES} bytes")));
        }
        let input = if source.media_type == "application/pdf" {
            let mut doc = run_worker(&self.worker, bytes.clone())?;
            let outcomes = add_ocr(&mut doc, &bytes, self.ocr.as_ref());
            RenditionInput::Pdf(doc, outcomes)
        } else {
            let text = String::from_utf8(bytes).map_err(|_| permanent("the text isn't valid UTF-8"))?;
            RenditionInput::Plain {
                text,
                parser: "plain-1".into(),
            }
        };
        self.server.build_rendition(vault, &source_id, input).map_err(from_server)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str, timeout_ms: u64) -> WorkerCommand {
        WorkerCommand {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            timeout: Duration::from_millis(timeout_ms),
        }
    }

    #[test]
    fn a_hanging_parser_is_killed_at_its_limit() {
        let t = Instant::now();
        let e = run_worker(&sh("sleep 30", 150), b"x".to_vec()).unwrap_err();
        assert!(e.message.contains("past its limit"), "{}", e.message);
        assert!(!e.retryable);
        assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    }

    #[test]
    fn a_crash_fails_cleanly() {
        let e = run_worker(&sh("kill -9 $$", 5000), vec![]).unwrap_err();
        assert_eq!(e.message, "the parser crashed");
    }

    #[test]
    fn a_refusal_keeps_its_reason() {
        let e = run_worker(&sh("echo 'the PDF is encrypted with a password' >&2; exit 3", 5000), vec![]).unwrap_err();
        assert_eq!(e.message, "the PDF is encrypted with a password");
        assert!(!e.retryable);
    }

    #[test]
    fn nonsense_output_and_other_exit_codes_fail_cleanly() {
        assert!(run_worker(&sh("echo not json", 5000), vec![]).unwrap_err().message.contains("isn't valid"));
        assert!(
            run_worker(&sh("echo oops >&2; exit 7", 5000), vec![])
                .unwrap_err()
                .message
                .contains("exited with 7")
        );
    }

    #[test]
    fn a_parser_that_ignores_a_large_input_does_not_deadlock() {
        let out = run_worker(&sh(r#"echo '{"parser":"p","pages":[],"words":[]}'"#, 5000), vec![0u8; 8 * 1024 * 1024]).unwrap();
        assert_eq!(out.parser, "p");
    }

    #[test]
    fn a_missing_program_can_be_retried() {
        let c = WorkerCommand {
            program: "/no/such/program".into(),
            args: vec![],
            timeout: Duration::from_secs(1),
        };
        assert!(run_worker(&c, vec![]).unwrap_err().retryable);
    }
}
