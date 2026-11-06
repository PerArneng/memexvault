//! The folder store on a real file system, with injected failures, crashes and a large blob.
// These tests create files, read time and sleep briefly while sampling a child process.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]
use memex_blob_fs::{FsBlobs, FsBlobsError, FsOps, RealFs, Step, VerifyResult};
use memex_types::{BlobHash, VaultId};
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("memex-blob-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn vault() -> VaultId {
    VaultId("v1".into())
}

struct Failing {
    at: Step,
    armed: Cell<bool>,
}

impl Failing {
    fn trip(&self, step: Step) -> io::Result<()> {
        if self.at == step && self.armed.get() {
            self.armed.set(false);
            return Err(io::Error::other("injected"));
        }
        Ok(())
    }
}

impl FsOps for Failing {
    fn write(&self, file: &mut File, bytes: &[u8]) -> io::Result<()> {
        self.trip(Step::Write)?;
        RealFs.write(file, bytes)
    }
    fn fsync(&self, file: &File) -> io::Result<()> {
        self.trip(Step::Fsync)?;
        RealFs.fsync(file)
    }
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.trip(Step::Rename)?;
        RealFs.rename(from, to)
    }
    fn fsync_dir(&self, dir: &Path) -> io::Result<()> {
        self.trip(Step::DirFsync)?;
        RealFs.fsync_dir(dir)
    }
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    if let Ok(entries) = fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() { out.extend(files_under(&p)) } else { out.push(p) }
        }
    }
    out
}

#[test]
fn layout_is_plain_files_in_shard_folders() {
    let root = temp("layout");
    let b = FsBlobs::new(&root, RealFs);
    let (hash, _, _) = b.put(&vault(), &mut &b"hello"[..]).unwrap();
    let h = hash.as_str();
    let want = root.join("vaults/v1/blobs").join(&h[..2]).join(&h[2..4]).join(h);
    assert_eq!(fs::read(want).unwrap(), b"hello");
    assert!(files_under(&root.join("vaults/v1/tmp")).is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_failure_at_each_step_leaves_tmp_clean_and_the_retry_succeeds() {
    for step in [Step::Write, Step::Fsync, Step::Rename, Step::DirFsync] {
        let root = temp(&format!("fault-{step:?}"));
        let b = FsBlobs::new(
            &root,
            Failing {
                at: step,
                armed: Cell::new(true),
            },
        );
        assert!(matches!(b.put(&vault(), &mut &b"data"[..]), Err(FsBlobsError::Io(_))), "{step:?}");
        assert!(files_under(&root.join("vaults/v1/tmp")).is_empty(), "{step:?}");
        // Before the rename nothing is visible. A failed directory sync may leave the complete file, which the retry dedups.
        if step != Step::DirFsync {
            assert!(files_under(&root.join("vaults/v1/blobs")).is_empty(), "{step:?}");
        }
        let (hash, size, _) = b.put(&vault(), &mut &b"data"[..]).unwrap();
        assert_eq!(size, 4);
        assert_eq!(b.verify(&vault(), &hash).unwrap(), VerifyResult::Ok);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn disk_full_surfaces_and_cleans_up() {
    struct Full;
    impl FsOps for Full {
        fn write(&self, _: &mut File, _: &[u8]) -> io::Result<()> {
            Err(io::Error::from_raw_os_error(28)) // ENOSPC
        }
        fn fsync(&self, f: &File) -> io::Result<()> {
            RealFs.fsync(f)
        }
        fn rename(&self, a: &Path, b: &Path) -> io::Result<()> {
            RealFs.rename(a, b)
        }
        fn fsync_dir(&self, d: &Path) -> io::Result<()> {
            RealFs.fsync_dir(d)
        }
    }
    let root = temp("full");
    let b = FsBlobs::new(&root, Full);
    let Err(FsBlobsError::Io(e)) = b.put(&vault(), &mut &b"data"[..]) else {
        panic!("expected an IO error")
    };
    assert_eq!(e.raw_os_error(), Some(28));
    assert!(files_under(&root.join("vaults/v1/tmp")).is_empty());
    assert!(files_under(&root.join("vaults/v1/blobs")).is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn traversal_is_refused_by_vault_name_and_by_hash_type() {
    let root = temp("traversal");
    let b = FsBlobs::new(&root, RealFs);
    for bad in ["../up", "..", ".", "", "a/b", "a\\b", "a\0b"] {
        let v = VaultId(bad.into());
        assert!(matches!(b.put(&v, &mut &b"x"[..]), Err(FsBlobsError::InvalidVault)), "{bad:?}");
    }
    assert!(BlobHash::parse("../../../../etc/passwd").is_err());
    assert!(BlobHash::parse(&format!("../{}", "a".repeat(61))).is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn verify_reports_a_corrupted_file() {
    let root = temp("corrupt");
    let b = FsBlobs::new(&root, RealFs);
    let (hash, _, _) = b.put(&vault(), &mut &b"original"[..]).unwrap();
    let h = hash.as_str();
    fs::write(root.join("vaults/v1/blobs").join(&h[..2]).join(&h[2..4]).join(h), b"tampered").unwrap();
    assert!(matches!(b.verify(&vault(), &hash).unwrap(), VerifyResult::Corrupt { actual } if actual != hash));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn listing_ignores_files_that_arent_ours() {
    let root = temp("stray");
    let b = FsBlobs::new(&root, RealFs);
    let (hash, _, _) = b.put(&vault(), &mut &b"x"[..]).unwrap();
    let h = hash.as_str();
    fs::write(root.join("vaults/v1/blobs").join(&h[..2]).join(&h[2..4]).join("notes.txt"), b"stray").unwrap();
    let listed = b.list(&vault(), None, 10).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].hash, hash);
    fs::remove_dir_all(root).unwrap();
}

/// Bytes from a fixed pattern, so a large upload needs no memory or test file.
struct Pattern {
    left: u64,
    seed: u8,
}

impl Read for Pattern {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = (buf.len() as u64).min(self.left) as usize;
        for (i, b) in buf[..n].iter_mut().enumerate() {
            *b = self.seed.wrapping_add((i % 251) as u8);
        }
        self.left -= n as u64;
        Ok(n)
    }
}

fn rss_kb(pid: u32) -> u64 {
    let out = Command::new("ps").args(["-o", "rss=", "-p", &pid.to_string()]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0)
}

/// Runs as a child: writes large blobs until killed, printing each hash once it's durable.
#[test]
#[ignore = "helper: run by the kill harness and the large blob test"]
fn child_writer() {
    let root = std::env::var("MEMEX_BLOB_ROOT").unwrap();
    let size: u64 = std::env::var("MEMEX_BLOB_SIZE").unwrap().parse().unwrap();
    let count: u32 = std::env::var("MEMEX_BLOB_COUNT").unwrap().parse().unwrap();
    let b = FsBlobs::new(root, RealFs);
    b.purge_tmp(&vault()).unwrap();
    let stdout = io::stdout();
    for i in 0..count {
        let (hash, _, _) = b
            .put(
                &vault(),
                &mut Pattern {
                    left: size,
                    seed: (std::process::id() as u8).wrapping_add(i as u8),
                },
            )
            .unwrap();
        writeln!(stdout.lock(), "STORED {hash}").unwrap();
    }
}

fn spawn_child(root: &Path, size: u64, count: u32) -> std::process::Child {
    Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child_writer", "--ignored", "--nocapture", "--test-threads=1"])
        .env("MEMEX_BLOB_ROOT", root)
        .env("MEMEX_BLOB_SIZE", size.to_string())
        .env("MEMEX_BLOB_COUNT", count.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

/// Spec 011 verification 2. Slow, so it's ignored by default.
#[test]
#[ignore = "kill harness, about a minute: cargo test -p memex-blob-fs --release -- --ignored kill_harness"]
fn kill_harness_never_shows_a_partial_blob() {
    let root = temp("kill");
    let mut seed = 0x1234_5678_u64;
    let (mut partial, mut stored_lost, mut total_stored, mut tmp_left) = (0, 0, 0, 0);
    for _ in 0..200 {
        let mut child = spawn_child(&root, 8 * 1024 * 1024, 1000);
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        std::thread::sleep(Duration::from_millis(20 + (seed >> 40) % 300));
        child.kill().unwrap();
        let mut out = String::new();
        child.stdout.take().unwrap().read_to_string(&mut out).unwrap();
        child.wait().unwrap();
        let b = FsBlobs::new(&root, RealFs);
        let acked: Vec<&str> = out.lines().filter_map(|l| l.split("STORED ").nth(1)).collect();
        total_stored += acked.len();
        for a in &acked {
            let hash = BlobHash::parse(a).unwrap();
            if !matches!(b.verify(&vault(), &hash), Ok(VerifyResult::Ok)) {
                stored_lost += 1;
            }
        }
        // Everything visible under blobs/ must be complete, whether acknowledged or not.
        for path in files_under(&root.join("vaults/v1/blobs")) {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let mut h = Sha256::new();
            h.update(fs::read(&path).unwrap());
            let actual: String = h.finalize().iter().map(|x| format!("{x:02x}")).collect();
            if actual != name {
                partial += 1;
            }
        }
        tmp_left += b.purge_tmp(&vault()).unwrap();
        assert!(files_under(&root.join("vaults/v1/tmp")).is_empty());
        // Keep disk use small: this cycle's blobs aren't needed again.
        let _ = fs::remove_dir_all(root.join("vaults/v1/blobs"));
    }
    eprintln!(
        "kill harness: 200 cycles, partial {partial}, acknowledged lost {stored_lost}, acknowledged in total {total_stored}, tmp files purged {tmp_left}"
    );
    assert_eq!((partial, stored_lost), (0, 0));
    assert!(total_stored > 0, "the harness must really store blobs");
    assert!(tmp_left > 0, "the harness must really kill mid-write");
    fs::remove_dir_all(root).unwrap();
}

/// Spec 011 verification 3.
#[test]
#[ignore = "writes 2 GB: cargo test -p memex-blob-fs --release -- --ignored large_blob"]
fn large_blob_streams_with_small_memory_growth() {
    let root = temp("large");
    let mut child = spawn_child(&root, 2 * 1024 * 1024 * 1024, 1);
    let pid = child.id();
    let t = Instant::now();
    let (mut first, mut peak) = (0, 0);
    while child.try_wait().unwrap().is_none() {
        let rss = rss_kb(pid);
        if first == 0 && rss > 0 {
            first = rss;
        }
        peak = peak.max(rss);
        std::thread::sleep(Duration::from_millis(50));
    }
    let growth_mb = peak.saturating_sub(first) as f64 / 1024.0;
    let b = FsBlobs::new(&root, RealFs);
    let listed = b.list(&vault(), None, 10).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].size, 2 * 1024 * 1024 * 1024);
    assert_eq!(b.verify(&vault(), &listed[0].hash).unwrap(), VerifyResult::Ok);
    eprintln!(
        "large blob: 2 GiB in {:.1} s, RSS at start {first} kB, peak {peak} kB, growth {growth_mb:.1} MB",
        t.elapsed().as_secs_f64()
    );
    assert!(growth_mb <= 64.0, "grew {growth_mb} MB");
    fs::remove_dir_all(root).unwrap();
}
