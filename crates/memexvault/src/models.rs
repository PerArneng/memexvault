//! Model files: pinned URLs and hashes, an explicit fetch, and a check at startup. The server never downloads at runtime.
// This module reads and writes model files and talks to the network, and only when asked to fetch.
#![allow(clippy::disallowed_methods)]
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;

#[derive(Debug, Clone, Copy)]
pub struct ModelFile {
    pub file: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub size: u64,
}

/// What embedding needs. Spec 020 adds the reranker.
pub const EMBED_DIR: &str = "multilingual-e5-small";
pub const EMBED_MODELS: [ModelFile; 2] = [
    ModelFile {
        file: "multilingual-e5-small/model.onnx",
        url: "https://huggingface.co/intfloat/multilingual-e5-small/resolve/614241f622f53c4eeff9890bdc4f31cfecc418b3/onnx/model.onnx",
        sha256: "ca456c06b3a9505ddfd9131408916dd79290368331e7d76bb621f1cba6bc8665",
        size: 470268510,
    },
    ModelFile {
        file: "multilingual-e5-small/tokenizer.json",
        url: "https://huggingface.co/intfloat/multilingual-e5-small/resolve/614241f622f53c4eeff9890bdc4f31cfecc418b3/tokenizer.json",
        sha256: "0b44a9d7b51c3c62626640cda0e2c2f70fdacdc25bbbd68038369d14ebdf4c39",
        size: 17082730,
    },
];

/// What OCR needs.
pub const OCR_MODELS: [ModelFile; 3] = [
    ModelFile {
        file: memex_ocr::DETECTOR,
        url: "https://github.com/GreatV/oar-ocr/releases/download/v0.7.0/pp-ocrv6_small_det.onnx",
        sha256: "d73e0058b7a8086bbd57f3d10b8bcd4ff95363f67e06e2762b5e814fe9c9410e",
        size: 9880512,
    },
    ModelFile {
        file: memex_ocr::RECOGNIZER,
        url: "https://github.com/GreatV/oar-ocr/releases/download/v0.3.0/latin_pp-ocrv5_mobile_rec.onnx",
        sha256: "e3a6bfeea1c8a01d6fccfd480a0bd363fd907f8c65931e228bb2736f5c3e142f",
        size: 8069614,
    },
    ModelFile {
        file: memex_ocr::DICTIONARY,
        url: "https://github.com/GreatV/oar-ocr/releases/download/v0.3.0/ppocrv5_latin_dict.txt",
        sha256: "ccbcc45730b3fbbd9050c5bc74db6a99067141ef1035e3d14889a84a6b9b1aff",
        size: 2616,
    },
];

fn hash_file(path: &Path) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let (mut hasher, mut buffer) = (Sha256::new(), vec![0u8; 1 << 16]);
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    let mut out = String::with_capacity(64);
    for b in hasher.finalize() {
        let _ = write!(out, "{b:02x}");
    }
    Ok(out)
}

/// A quick check for startup: each file is there at the right size. It tells the operator what to run.
pub fn check_present(dir: &Path, models: &[ModelFile]) -> Result<(), String> {
    for m in models {
        match fs::metadata(dir.join(m.file)) {
            Ok(meta) if meta.len() == m.size => {}
            Ok(meta) => {
                return Err(format!(
                    "model {} is {} bytes, expected {}, so run: memexvault models fetch",
                    m.file,
                    meta.len(),
                    m.size
                ));
            }
            Err(_) => return Err(format!("model {} isn't in {}, so run: memexvault models fetch", m.file, dir.display())),
        }
    }
    Ok(())
}

/// The full check, by hash. OCR's files are small, so its workers run this before loading them.
pub fn verify(dir: &Path, models: &[ModelFile]) -> Result<(), String> {
    check_present(dir, models)?;
    for m in models {
        let actual = hash_file(&dir.join(m.file)).map_err(|e| format!("can't read {}: {e}", m.file))?;
        if actual != m.sha256 {
            return Err(format!(
                "model {} has the wrong hash, so it's corrupted or replaced. Delete it and run: memexvault models fetch",
                m.file
            ));
        }
    }
    Ok(())
}

/// Downloads what's missing or wrong, to a temporary name, and moves it into place only once its hash matches.
pub fn fetch(dir: &Path, models: &[ModelFile]) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    for m in models {
        let target = dir.join(m.file);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        if hash_file(&target).is_ok_and(|h| h == m.sha256) {
            continue;
        }
        let temporary = dir.join(format!("{}.part", m.file));
        let mut response = ureq::get(m.url).call().map_err(|e| format!("{}: {e}", m.url))?;
        let mut reader = response.body_mut().with_config().limit(m.size + 1).reader();
        let mut out = File::create(&temporary).map_err(|e| e.to_string())?;
        std::io::copy(&mut reader, &mut out).map_err(|e| e.to_string())?;
        out.flush().map_err(|e| e.to_string())?;
        out.sync_all().map_err(|e| e.to_string())?;
        drop(out);
        let actual = hash_file(&temporary).map_err(|e| e.to_string())?;
        if actual != m.sha256 {
            let _ = fs::remove_file(&temporary);
            return Err(format!("{} downloaded with the wrong hash", m.file));
        }
        fs::rename(&temporary, &target).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("memex-models-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    const FILE: ModelFile = ModelFile {
        file: "m.bin",
        url: "https://invalid.example/m.bin",
        sha256: "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
        size: 5,
    };

    #[test]
    fn missing_says_what_to_run() {
        let d = temp("missing");
        let e = check_present(&d, &[FILE]).unwrap_err();
        assert!(e.contains("models fetch"), "{e}");
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn right_size_passes_the_quick_check_but_a_wrong_hash_is_refused() {
        let d = temp("corrupt");
        fs::write(d.join("m.bin"), b"hellO").unwrap();
        check_present(&d, &[FILE]).unwrap();
        let e = verify(&d, &[FILE]).unwrap_err();
        assert!(e.contains("wrong hash"), "{e}");
        fs::write(d.join("m.bin"), b"hello").unwrap();
        verify(&d, &[FILE]).unwrap();
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn a_wrong_size_is_refused_at_once() {
        let d = temp("size");
        fs::write(d.join("m.bin"), b"hi").unwrap();
        assert!(check_present(&d, &[FILE]).unwrap_err().contains("expected 5"));
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn fetch_skips_what_is_already_right() {
        let d = temp("skip");
        fs::write(d.join("m.bin"), b"hello").unwrap();
        // The URL can't be reached, so this only passes if it doesn't try.
        fetch(&d, &[FILE]).unwrap();
        fs::remove_dir_all(d).unwrap();
    }
}
