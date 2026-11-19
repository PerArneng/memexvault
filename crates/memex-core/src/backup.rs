//! Backup and restore of one vault (spec 030), through the traits, so it works the same with any store or blob backend and moves a vault between them.
//!
//! The archive is a stream of records, each a text line naming it and its length, then that many bytes and a newline:
//!
//! ```text
//! memexvault-backup 1
//! header <len>      a JSON header: the vault, the log's head and the canonical dump hash at that head
//! event <len>       one event as JSON, in sequence order, from 0 to the head
//! blob <sha256> <len>  the bytes of a blob a row refers to
//! embedding <len>   a stored embedding as JSON: [model, content hash, vector]
//! end <sha256>      the SHA-256 of every byte before this line
//! ```
//!
//! A restore reads the archive twice: once to check it whole, every blob against its name and the trailer against the bytes, and only
//! then to write anything. Replaying the events gives the tables, which must match the header's dump hash. Indexes aren't in it, as
//! they rebuild from the tables, and embeddings are, so a rebuild doesn't have to embed again. Chat threads stay out, as they're off
//! the log by design (spec 049).
use crate::{BlobStore, CommitItem, Pending, Snapshot, Store, StoreError, VectorIndex, WriterLease, reduce, upcast};
use memex_types::{BlobHash, Event, VaultId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::io::{BufRead, Read, Write};

pub const MAGIC: &str = "memexvault-backup 1";
pub const FORMAT: u32 = 1;
/// Events replayed per transaction on restore.
const RESTORE_BATCH: usize = 500;
/// The longest record line, so a damaged file can't make a restore read without end.
const MAX_LINE: usize = 512;
/// The largest JSON record. A blob's length has no such limit, as it streams.
const MAX_JSON: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupHeader {
    pub format: u32,
    pub vault_id: VaultId,
    pub vault_name: String,
    /// The last event in the archive, and none for an empty log.
    pub head: Option<u64>,
    /// `dump_store` of the tables at `head`. A restore must arrive at exactly this.
    pub dump_sha256: String,
    pub created_at_ms: i64,
    pub server_version: String,
}

/// What an archive holds, as counted while writing or checking it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BackupSummary {
    pub events: u64,
    pub blobs: u64,
    pub blob_bytes: u64,
    pub embeddings: u64,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug)]
pub enum BackupError {
    /// The archive is damaged, truncated or not one at all. Nothing was written.
    Corrupt(String),
    /// The target vault isn't empty.
    NotEmpty(VaultId),
    Io(String),
    Store(StoreError),
    Other(String),
}

impl fmt::Display for BackupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Corrupt(m) => write!(f, "the backup is damaged or isn't one: {m}"),
            Self::NotEmpty(v) => write!(f, "vault {v} isn't empty, so nothing was restored into it"),
            Self::Io(m) => write!(f, "{m}"),
            Self::Store(e) => write!(f, "{e}"),
            Self::Other(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for BackupError {}

impl From<StoreError> for BackupError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

impl From<std::io::Error> for BackupError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Counts and hashes what passes through.
struct HashingWriter<'a> {
    inner: &'a mut dyn Write,
    hash: Sha256,
    bytes: u64,
}

impl Write for HashingWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hash.update(&buf[..n]);
        self.bytes += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn json_record(out: &mut HashingWriter<'_>, tag: &str, value: &impl Serialize) -> Result<(), BackupError> {
    let bytes = serde_json::to_vec(value).map_err(|e| BackupError::Other(e.to_string()))?;
    writeln!(out, "{tag} {}", bytes.len())?;
    out.write_all(&bytes)?;
    out.write_all(b"\n")?;
    Ok(())
}

/// Writes a backup of the vault as of one snapshot, while reads and writes go on. Only blobs a row of that snapshot refers to go in. A blob deleted meanwhile fails the backup, which can simply run again.
pub fn export<S: Store, B: BlobStore, V: VectorIndex>(
    store: &S,
    blobs: &B,
    vector: &V,
    vault: &VaultId,
    vault_name: &str,
    now_ms: i64,
    out: &mut dyn Write,
) -> Result<(BackupHeader, BackupSummary), BackupError> {
    let mut w = HashingWriter {
        inner: out,
        hash: Sha256::new(),
        bytes: 0,
    };
    let mut summary = BackupSummary::default();
    let snap = store.snapshot(vault)?;
    let head = snap.head()?;
    let header = BackupHeader {
        format: FORMAT,
        vault_id: vault.clone(),
        vault_name: vault_name.into(),
        head,
        dump_sha256: crate::dump_store(&snap)?.sha256,
        created_at_ms: now_ms,
        server_version: env!("CARGO_PKG_VERSION").into(),
    };
    writeln!(w, "{MAGIC}")?;
    json_record(&mut w, "header", &header)?;
    let mut after = None;
    loop {
        let page = snap.events(after)?;
        let Some(last) = page.last() else { break };
        after = Some(last.seq);
        for e in &page {
            if head.is_some_and(|h| e.seq > h) {
                break;
            }
            json_record(&mut w, "event", &StoredEvent::from(e))?;
            summary.events += 1;
        }
    }
    let mut referenced = crate::erasure::referenced_blobs(&snap)?;
    referenced.remove("");
    drop(snap);
    // The listing gives each blob's length, which goes before its bytes.
    let mut sized: Vec<(BlobHash, u64)> = vec![];
    let mut after: Option<BlobHash> = None;
    loop {
        let page = blobs
            .list(vault, after.as_ref(), crate::PAGE_LIMIT)
            .map_err(|e| BackupError::Other(e.to_string()))?;
        let Some(last) = page.last() else { break };
        after = Some(last.hash.clone());
        for b in page {
            if referenced.remove(b.hash.as_str()) {
                sized.push((b.hash, b.size));
            }
        }
    }
    if let Some(missing) = referenced.into_iter().next() {
        return Err(BackupError::Other(format!("blob {missing} is referred to and isn't stored")));
    }
    for (parsed, size) in sized {
        let hash = parsed.as_str().to_string();
        writeln!(w, "blob {hash} {size}")?;
        let mut reader = blobs
            .read(vault, &parsed, 0, None)
            .map_err(|e| BackupError::Other(format!("blob {hash}: {e}")))?;
        let copied = std::io::copy(&mut reader, &mut w)?;
        if copied != size {
            return Err(BackupError::Other(format!("blob {hash} changed while it was read")));
        }
        w.write_all(b"\n")?;
        summary.blobs += 1;
        summary.blob_bytes += size;
    }
    let mut cursor: Option<(String, String)> = None;
    loop {
        let page = vector
            .stored_embeddings(vault, cursor.as_ref().map(|(m, h)| (m.as_str(), h.as_str())), crate::PAGE_LIMIT)
            .map_err(|e| BackupError::Other(e.to_string()))?;
        let Some(last) = page.last() else { break };
        cursor = Some((last.0.clone(), last.1.clone()));
        for row in &page {
            json_record(&mut w, "embedding", row)?;
            summary.embeddings += 1;
        }
    }
    let sha = hex(&w.hash.clone().finalize());
    let bytes = w.bytes;
    writeln!(w.inner, "end {sha}")?;
    w.inner.flush()?;
    summary.bytes = bytes + format!("end {sha}\n").len() as u64;
    summary.sha256 = sha;
    Ok((header, summary))
}

/// An event without its vault, which a restore names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct StoredEvent {
    seq: u64,
    actor: String,
    time_ms: i64,
    kind: String,
    schema_version: u32,
    payload: serde_json::Value,
}

impl From<&Event> for StoredEvent {
    fn from(e: &Event) -> Self {
        Self {
            seq: e.seq,
            actor: e.actor.clone(),
            time_ms: e.time_ms,
            kind: e.kind.clone(),
            schema_version: e.schema_version,
            payload: e.payload.clone(),
        }
    }
}

impl StoredEvent {
    fn into_event(self, vault: &VaultId) -> Event {
        Event {
            seq: self.seq,
            vault: vault.clone(),
            actor: self.actor,
            time_ms: self.time_ms,
            kind: self.kind,
            schema_version: self.schema_version,
            payload: self.payload,
        }
    }
}

/// One record as read.
enum Record {
    Header(BackupHeader),
    Event(StoredEvent),
    /// The blob's bytes have been handed to the sink by the time this is returned.
    Blob {
        size: u64,
    },
    Embedding((String, String, Vec<f32>)),
    End,
}

/// Reads records, hashing every byte before the trailer.
struct Reader<R: BufRead> {
    inner: R,
    hash: Sha256,
    seen_magic: bool,
}

impl<R: BufRead> Reader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            hash: Sha256::new(),
            seen_magic: false,
        }
    }

    fn line(&mut self) -> Result<Option<String>, BackupError> {
        let mut buf = vec![];
        let n = (&mut self.inner).take(MAX_LINE as u64 + 1).read_until(b'\n', &mut buf)?;
        if n == 0 {
            return Ok(None);
        }
        if buf.last() != Some(&b'\n') {
            return Err(BackupError::Corrupt("a record line is cut short or too long".into()));
        }
        buf.pop();
        String::from_utf8(buf)
            .map(Some)
            .map_err(|_| BackupError::Corrupt("a record line isn't text".into()))
    }

    fn exact(&mut self, len: u64, sink: &mut dyn Write) -> Result<(), BackupError> {
        let mut hasher = HashingSink {
            hash: &mut self.hash,
            inner: sink,
        };
        let copied = std::io::copy(&mut (&mut self.inner).take(len), &mut hasher)?;
        if copied != len {
            return Err(BackupError::Corrupt("the archive ends inside a record".into()));
        }
        let mut nl = [0u8; 1];
        self.inner
            .read_exact(&mut nl)
            .map_err(|_| BackupError::Corrupt("the archive ends inside a record".into()))?;
        if nl[0] != b'\n' {
            return Err(BackupError::Corrupt("a record is longer than it says".into()));
        }
        self.hash.update(nl);
        Ok(())
    }

    fn json<T: serde::de::DeserializeOwned>(&mut self, len: u64) -> Result<T, BackupError> {
        if len > MAX_JSON {
            return Err(BackupError::Corrupt("a record is too large".into()));
        }
        let mut buf = Vec::with_capacity(len as usize);
        self.exact(len, &mut buf)?;
        serde_json::from_slice(&buf).map_err(|e| BackupError::Corrupt(format!("a record isn't valid: {e}")))
    }

    /// The next record. A blob's bytes go to `blob_sink`, which gets the hash first and checks them.
    fn next(&mut self, blob_sink: &mut dyn FnMut(&str, &mut dyn Read) -> Result<(), BackupError>) -> Result<Record, BackupError> {
        if !self.seen_magic {
            let line = self.line()?.ok_or_else(|| BackupError::Corrupt("the file is empty".into()))?;
            if line != MAGIC {
                return Err(BackupError::Corrupt("it doesn't start as a memexvault backup".into()));
            }
            self.hash.update(format!("{line}\n").as_bytes());
            self.seen_magic = true;
        }
        let before = self.hash.clone();
        let line = self
            .line()?
            .ok_or_else(|| BackupError::Corrupt("the archive ends without its trailer".into()))?;
        let parts: Vec<&str> = line.split(' ').collect();
        let number = |s: &str| s.parse::<u64>().map_err(|_| BackupError::Corrupt(format!("a bad length in {line:?}")));
        match parts.as_slice() {
            ["end", sha] => {
                let actual = hex(&before.finalize());
                if &actual != sha {
                    return Err(BackupError::Corrupt("its checksum doesn't match its contents".into()));
                }
                let mut rest = [0u8; 1];
                if self.inner.read(&mut rest)? != 0 {
                    return Err(BackupError::Corrupt("there's more after its trailer".into()));
                }
                Ok(Record::End)
            }
            [tag, len] => {
                self.hash.update(format!("{line}\n").as_bytes());
                let len = number(len)?;
                match *tag {
                    "header" => Ok(Record::Header(self.json(len)?)),
                    "event" => Ok(Record::Event(self.json(len)?)),
                    "embedding" => Ok(Record::Embedding(self.json(len)?)),
                    _ => Err(BackupError::Corrupt(format!("an unknown record {tag:?}"))),
                }
            }
            ["blob", hash, len] => {
                self.hash.update(format!("{line}\n").as_bytes());
                let size = number(len)?;
                let hash = hash.to_string();
                let mut body = HashingRead {
                    inner: (&mut self.inner).take(size),
                    hash: &mut self.hash,
                    read: 0,
                };
                blob_sink(&hash, &mut body)?;
                // Whatever the sink left unread still counts, for the trailer and the framing.
                std::io::copy(&mut body, &mut std::io::sink())?;
                if body.read != size {
                    return Err(BackupError::Corrupt("the archive ends inside a blob".into()));
                }
                let mut nl = [0u8; 1];
                self.inner
                    .read_exact(&mut nl)
                    .map_err(|_| BackupError::Corrupt("the archive ends inside a blob".into()))?;
                if nl[0] != b'\n' {
                    return Err(BackupError::Corrupt("a blob is longer than it says".into()));
                }
                self.hash.update(nl);
                Ok(Record::Blob { size })
            }
            _ => Err(BackupError::Corrupt(format!("an unknown record line {line:?}"))),
        }
    }
}

/// Hashes what's read through it, for the trailer.
struct HashingRead<'a, R: Read> {
    inner: R,
    hash: &'a mut Sha256,
    read: u64,
}

impl<R: Read> Read for HashingRead<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hash.update(&buf[..n]);
        self.read += n as u64;
        Ok(n)
    }
}

struct HashingSink<'a> {
    hash: &'a mut Sha256,
    inner: &'a mut dyn Write,
}

impl Write for HashingSink<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hash.update(&buf[..n]);
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Hashes what's written, for checking a blob against its name.
struct Sha256Sink(Sha256);

impl Write for Sha256Sink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.update(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Reads a whole archive and checks it, writing nothing: its framing, its trailer, each blob against its name and the events' order. A restore runs this first.
pub fn verify(read: &mut dyn Read) -> Result<(BackupHeader, BackupSummary), BackupError> {
    let mut r = Reader::new(std::io::BufReader::new(read));
    let mut header = None;
    let mut summary = BackupSummary::default();
    let mut next_seq = 0u64;
    loop {
        let mut blob_ok = Ok(());
        let record = r.next(&mut |hash, body| {
            let mut sink = Sha256Sink(Sha256::new());
            std::io::copy(body, &mut sink)?;
            if hex(&sink.0.finalize()) != hash {
                blob_ok = Err(BackupError::Corrupt(format!("blob {hash} doesn't hash to its name")));
            }
            Ok(())
        })?;
        blob_ok?;
        match record {
            Record::Header(h) => {
                if header.is_some() || summary.events > 0 {
                    return Err(BackupError::Corrupt("the header isn't first".into()));
                }
                if h.format != FORMAT {
                    return Err(BackupError::Corrupt(format!("format {} isn't one this server reads", h.format)));
                }
                header = Some(h);
            }
            Record::Event(e) => {
                if header.is_none() {
                    return Err(BackupError::Corrupt("an event comes before the header".into()));
                }
                if e.seq != next_seq {
                    return Err(BackupError::Corrupt(format!("event {} where {next_seq} should be", e.seq)));
                }
                next_seq += 1;
                summary.events += 1;
            }
            Record::Blob { size, .. } => {
                summary.blobs += 1;
                summary.blob_bytes += size;
            }
            Record::Embedding(_) => summary.embeddings += 1,
            Record::End => break,
        }
    }
    let header = header.ok_or_else(|| BackupError::Corrupt("it has no header".into()))?;
    let last = next_seq.checked_sub(1);
    if last != header.head {
        return Err(BackupError::Corrupt(format!(
            "it ends at event {last:?}, and its header says {:?}",
            header.head
        )));
    }
    Ok((header, summary))
}

/// Writes a checked archive into an empty vault the caller holds the lease of: blobs first, then the events replayed in batches, then the stored embeddings. Fails unless the tables come out at the header's dump hash. Run `verify` on the same bytes first.
pub fn import<S: Store, B: BlobStore, V: VectorIndex>(
    store: &S,
    blobs: &B,
    vector: &V,
    lease: &WriterLease,
    read: &mut dyn Read,
) -> Result<BackupSummary, BackupError> {
    let vault = &lease.vault;
    if store.snapshot(vault)?.head()?.is_some() {
        return Err(BackupError::NotEmpty(vault.clone()));
    }
    let mut r = Reader::new(std::io::BufReader::new(read));
    let mut header = None;
    let mut summary = BackupSummary::default();
    let mut batch: Vec<Event> = vec![];
    let mut embeddings: Vec<(String, String, Vec<f32>)> = vec![];
    loop {
        let mut stored = Ok(());
        let record = r.next(&mut |hash, body| {
            // Streamed straight into the blob store, which hashes as it writes, so a recording of any length takes no more memory.
            match blobs.put(vault, body) {
                Ok(s) if s.hash.as_str() == hash => {}
                Ok(_) => stored = Err(BackupError::Corrupt(format!("blob {hash} doesn't hash to its name"))),
                Err(e) => stored = Err(BackupError::Other(format!("blob {hash}: {e}"))),
            }
            Ok(())
        })?;
        stored?;
        match record {
            Record::Header(h) => header = Some(h),
            Record::Event(e) => {
                batch.push(e.into_event(vault));
                if batch.len() == RESTORE_BATCH {
                    summary.events += replay_batch(store, lease, std::mem::take(&mut batch))?;
                }
            }
            Record::Blob { size, .. } => {
                summary.blobs += 1;
                summary.blob_bytes += size;
            }
            Record::Embedding(e) => {
                embeddings.push(e);
                if embeddings.len() == crate::PAGE_LIMIT {
                    summary.embeddings += store_embeddings(vector, vault, std::mem::take(&mut embeddings))?;
                }
            }
            Record::End => break,
        }
    }
    if !batch.is_empty() {
        summary.events += replay_batch(store, lease, batch)?;
    }
    summary.embeddings += store_embeddings(vector, vault, embeddings)?;
    let header = header.ok_or_else(|| BackupError::Corrupt("it has no header".into()))?;
    let dump = crate::dump_store(&store.snapshot(vault)?)?;
    if dump.sha256 != header.dump_sha256 {
        return Err(BackupError::Other(format!(
            "the restored tables hash to {}, and the backup's to {}",
            dump.sha256, header.dump_sha256
        )));
    }
    summary.sha256 = header.dump_sha256;
    Ok(summary)
}

fn store_embeddings<V: VectorIndex>(vector: &V, vault: &VaultId, rows: Vec<(String, String, Vec<f32>)>) -> Result<u64, BackupError> {
    let n = rows.len() as u64;
    let mut by_model: std::collections::BTreeMap<String, Vec<(String, Vec<f32>)>> = std::collections::BTreeMap::new();
    for (model, hash, v) in rows {
        by_model.entry(model).or_default().push((hash, v));
    }
    for (model, rows) in by_model {
        vector.store_embeddings(vault, &model, &rows).map_err(|e| BackupError::Other(e.to_string()))?;
    }
    Ok(n)
}

/// Reduces each event on top of the ones before it and commits the batch in one transaction, keeping each event's own sequence and time.
fn replay_batch<S: Store>(store: &S, lease: &WriterLease, events: Vec<Event>) -> Result<u64, BackupError> {
    let snap = store.snapshot(&lease.vault)?;
    let mut pending = Pending::over(&snap)?;
    let mut accepted = vec![];
    for e in events {
        let current = upcast(&e).map_err(|err| BackupError::Other(err.to_string()))?;
        let mutations = reduce(&pending, &current).map_err(|err| BackupError::Other(format!("event {}: {err}", e.seq)))?;
        pending.apply(e.seq, &mutations);
        accepted.push((e, mutations));
    }
    drop(pending);
    drop(snap);
    let items: Vec<CommitItem<'_>> = accepted.iter().map(|(event, mutations)| CommitItem { event, mutations }).collect();
    store.commit_batch(lease, &items)?;
    Ok(accepted.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_damaged_or_foreign_file_is_refused() {
        assert!(matches!(verify(&mut "".as_bytes()), Err(BackupError::Corrupt(_))));
        assert!(matches!(verify(&mut "PK\u{3}\u{4} a zip\n".as_bytes()), Err(BackupError::Corrupt(_))));
        let no_trailer = format!("{MAGIC}\n");
        assert!(matches!(verify(&mut no_trailer.as_bytes()), Err(BackupError::Corrupt(_))));
        let long = format!("{MAGIC}\nevent {}\n", "9".repeat(600));
        assert!(matches!(verify(&mut long.as_bytes()), Err(BackupError::Corrupt(_))));
    }
}
