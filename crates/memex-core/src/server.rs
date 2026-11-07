mod entities;
mod extraction;
mod pages;
pub use entities::{
    ClaimEvidence, EntityCandidate, EntityClaim, EntityEdit, EntityResolution, EntityView, FieldInput, NewEntity, Redirect, SplitRequest, TypeDecision,
};
pub use extraction::{ExtractJobError, ExtractionReport};
pub use pages::{
    BacklinkView, ClaimRefStatus, ClaimReference, ImportedPage, NewPage, OutgoingLink, PageChange, PageDiff, PageRedirect, PageRevisionView, PageView,
    UnresolvedLink,
};

use crate::auth::{Caller, Decision, Operation, authorize};
use crate::governance::{self, Author, NotReversible, PlanError};
use crate::session::{LoginLimiter, SESSION_TTL_MS};
use crate::vector::{EMBED_ATTEMPTS, EmbedDocRef, EmbedInput, EmbedRequest, check_query_text, embed_missing, embeddable};
use crate::{
    ALLOWED_DPI, ChangeSetRecorded, PageImage, Rasterizer, Snapshot, StateRecorded, THUMBNAIL_DPI, UploadLimits, UploadOutcome, UploadRequest, jobs, rendition,
    sources,
};
use crate::{ApplyError, BlobStore, BlobStoreError, Clock, Engine, Receipt, Registry, RegistryError, Seed, Store, StoreError, WriterLease};
use crate::{EMBED_JOB, EmbedJobError, Embedder, LexicalIndex, LexicalProjector, ProjectError, StepOutcome, VectorIndex, VectorProjector};
use memex_types::{
    Audit, ChangeSetRow, ChangeSetState, Event, EventDraft, Grant, Header, JobRow, JobState, LexicalError, LexicalFilter, LexicalResults, Mutation, ObjectId,
    Principal, PrincipalKind, Rect, RenditionRow, RiskDecision, Row, RowKey, SegmentRow, SessionRow, SourceRow, Status, Table, TokenRow, ValidatorResult,
    VaultId, VaultRow,
};
use serde_json::Value;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt::{self, Write as _};
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The most items a list returns at once.
pub const RESULT_LIMIT: usize = 200;

/// How many `as_of` searches run at once. Each one replays the log into memory, so more would risk the memory target.
const AS_OF_SLOTS: u64 = 2;

struct AsOfSlot<'a>(&'a AtomicU64);

impl<'a> AsOfSlot<'a> {
    fn take(running: &'a AtomicU64) -> Result<Self, ServerError> {
        if running.fetch_add(1, Ordering::SeqCst) >= AS_OF_SLOTS {
            running.fetch_sub(1, Ordering::SeqCst);
            return Err(ServerError::Busy);
        }
        Ok(Self(running))
    }
}

impl Drop for AsOfSlot<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

const TOKEN_PREFIX: &str = "mvx_";
const SESSION_PREFIX: &str = "mxs_";
const TOUCH_EVERY_MS: i64 = 60_000;

/// A new browser session. `session_id` is shown once, in the cookie, and only its hash is stored.
#[derive(Debug)]
pub struct CreatedSession {
    pub session_id: String,
    pub caller: Caller,
    pub max_age_s: i64,
}

/// The server's edges, grouped so its type stays readable.
pub trait ServerEdges {
    type Registry: Registry;
    type Store: Store;
    type Blobs: BlobStore;
    type Clock: Clock;
    type Seed: Seed;
    type Rasterizer: Rasterizer + Default;
    type Lexical: LexicalIndex + Sync;
    type Vector: VectorIndex + Sync;
    type Embedder: Embedder + Sync;
    type Reranker: crate::Reranker;
}

/// The edges that answer searches, which travel together.
pub struct SearchEdges<L, V, Em, R> {
    pub lexical: L,
    pub vector: V,
    pub embedder: Em,
    pub reranker: R,
}

#[derive(Debug)]
pub enum ServerError {
    /// A missing, malformed, revoked or expired token. It doesn't say which.
    Unauthenticated,
    Forbidden,
    /// Missing, or hidden from this caller. The two look the same on purpose.
    NotFound,
    Invalid(String),
    Conflict(String),
    /// A base version is stale. It carries the current row so the caller can rebase.
    StaleVersion {
        key: RowKey,
        current: Box<Row>,
        base: u32,
    },
    /// This can't be reverted, and nothing changed.
    NotReversible(String),
    /// The upload is over the limit.
    TooLarge {
        limit: u64,
    },
    /// The bytes aren't a type the pipeline handles.
    UnsupportedMedia(String),
    /// Too much unfinished work. Try again later.
    Busy,
    /// Too many failed sign-ins from this client.
    RateLimited {
        retry_after_s: u64,
    },
    /// A backend is down or failed. Safe to retry.
    Unavailable(String),
    /// An index hasn't yet seen the event a reader asked for. Safe to retry.
    NotCaughtUp {
        min_seq: u64,
        reached: Option<u64>,
    },
    AlreadyBootstrapped,
}

impl fmt::Display for ServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unauthenticated => f.write_str("authentication required"),
            Self::Forbidden => f.write_str("this token can't do that"),
            Self::NotFound => f.write_str("not found"),
            Self::Invalid(m) => write!(f, "{m}"),
            Self::Conflict(m) => write!(f, "{m}"),
            Self::StaleVersion { key, base, .. } => write!(f, "{:?} {} has moved on from version {base}", key.table, key.id),
            Self::NotReversible(m) => write!(f, "can't revert: {m}"),
            Self::TooLarge { limit } => write!(f, "the upload is over the limit of {limit} bytes"),
            Self::UnsupportedMedia(m) => write!(f, "unsupported file type: {m}"),
            Self::Busy => f.write_str("too much unfinished work, so try again later"),
            Self::RateLimited { retry_after_s } => write!(f, "too many failed sign-ins, so try again in {retry_after_s} seconds"),
            Self::Unavailable(m) => write!(f, "{m}"),
            Self::NotCaughtUp { min_seq, reached } => write!(f, "the search index hasn't reached event {min_seq} yet, and is at {reached:?}"),
            Self::AlreadyBootstrapped => f.write_str("this server already has principals"),
        }
    }
}

impl std::error::Error for ServerError {}

impl From<RegistryError> for ServerError {
    fn from(e: RegistryError) -> Self {
        match e {
            RegistryError::Conflict(m) => Self::Conflict(m),
            RegistryError::NotFound => Self::NotFound,
            RegistryError::Backend(m) => Self::Unavailable(m),
        }
    }
}

impl From<StoreError> for ServerError {
    fn from(e: StoreError) -> Self {
        Self::Unavailable(e.to_string())
    }
}

impl From<memex_types::VectorError> for ServerError {
    fn from(e: memex_types::VectorError) -> Self {
        Self::Unavailable(e.to_string())
    }
}

impl From<memex_types::EmbedError> for ServerError {
    fn from(e: memex_types::EmbedError) -> Self {
        Self::Unavailable(e.to_string())
    }
}

impl From<LexicalError> for ServerError {
    fn from(e: LexicalError) -> Self {
        Self::Unavailable(e.to_string())
    }
}

impl From<BlobStoreError> for ServerError {
    fn from(e: BlobStoreError) -> Self {
        Self::Unavailable(e.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewToken {
    pub principal_id: Option<String>,
    pub name: String,
    pub grants: Vec<Grant>,
    pub expires_at_ms: Option<i64>,
}

/// The only time the token itself exists outside its holder's hands.
pub struct CreatedToken {
    pub token: String,
    pub row: TokenRow,
}

/// The token is sensitive, so its Debug form leaves it out.
impl fmt::Debug for CreatedToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreatedToken").field("token", &"<redacted>").field("row", &self.row.id).finish()
    }
}

/// What a rendition job hands over: a parsed PDF, or plain text with no boxes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenditionInput {
    Pdf(memex_types::ParsedDocument, Vec<memex_types::PageOutcome>),
    Plain { text: String, parser: String },
}

/// One piece of evidence, followed all the way to its source and region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceLineage {
    pub evidence: memex_types::EvidenceRow,
    pub segment: SegmentRow,
    pub rendition: RenditionRow,
    pub source: SourceRow,
    /// The rendition has a newer one. The object still resolves, and says so.
    pub superseded: bool,
    /// Where the quote is now. None means it no longer resolves, and that's reported rather than guessed.
    pub anchor: Option<memex_types::Anchor>,
    pub broken: bool,
}

/// From a claim to everything behind it.
#[derive(Debug, Clone, PartialEq)]
pub struct Lineage {
    pub assertion: memex_types::AssertionRow,
    pub changeset: Option<ChangeSetRow>,
    pub evidence: Vec<EvidenceLineage>,
}

/// A claim that cites something, for reverse lookup.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaimRef {
    pub assertion: memex_types::AssertionRow,
    pub evidence_id: ObjectId,
}

/// What a client needs to draw a source's pages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageList {
    pub rendition_id: ObjectId,
    pub pages: Vec<memex_types::PageFacts>,
}

/// What to do with a proposal, from `Server::propose`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    pub ops: Vec<memex_types::Op>,
    pub rationale: String,
    pub idempotency_key: String,
}

/// Validator results, and the pairs of a new claim and an existing one it contradicts.
type Validated = (Vec<ValidatorResult>, Vec<(ObjectId, ObjectId)>);

fn mark_machine(op: &mut memex_types::Op, is_machine: bool) {
    match op {
        memex_types::Op::AddAssertion { machine, .. } => *machine = is_machine,
        memex_types::Op::SupersedeAssertion { replacement, .. } => mark_machine(replacement, is_machine),
        _ => {}
    }
}

/// The outcome of proposing, including a dry run.
#[derive(Debug, Clone, PartialEq)]
pub struct ProposalOutcome {
    pub changeset: Option<ChangeSetRow>,
    pub validators: Vec<ValidatorResult>,
    pub risk: RiskDecision,
    pub contradictions: Vec<(ObjectId, ObjectId)>,
    /// A retry with a key seen before gets the first result.
    pub replayed: bool,
    pub dry_run: bool,
}

/// The facade for what's above the vault level, and the holder of each vault's writer lease.
pub struct Server<E: ServerEdges> {
    registry: E::Registry,
    engine: Engine<E::Store, E::Clock>,
    blobs: E::Blobs,
    seed: E::Seed,
    rasterizer: E::Rasterizer,
    lexical: E::Lexical,
    projector: Mutex<LexicalProjector>,
    vector: E::Vector,
    embedder: E::Embedder,
    reranker: E::Reranker,
    /// How many `as_of` searches are running. Each one holds the vault's past in memory.
    as_of_running: AtomicU64,
    /// Lives in memory: a restart forgets it, and one server serves each database.
    login_limiter: Mutex<LoginLimiter>,
    vector_projector: Mutex<VectorProjector>,
    leases: Mutex<HashMap<VaultId, WriterLease>>,
    /// Decisions read state and then commit, so they run one at a time per vault. The writer lease stops other processes.
    write_locks: Mutex<HashMap<VaultId, Arc<Mutex<()>>>>,
    machine_auto_commit: AtomicBool,
    id_counter: AtomicU64,
    /// None until the composition root has an LLM to send documents to.
    extraction: Mutex<Option<crate::extraction::ExtractionLimits>>,
}

fn sha256_hex(text: &str) -> String {
    let mut out = String::with_capacity(64);
    for b in Sha256::digest(text.as_bytes()) {
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn check_name(name: &str) -> Result<(), ServerError> {
    if name.trim().is_empty() || name.len() > 200 {
        return Err(ServerError::Invalid("a name must have 1 to 200 characters".into()));
    }
    Ok(())
}

impl<E: ServerEdges> Server<E> {
    pub fn new(
        registry: E::Registry,
        store: E::Store,
        blobs: E::Blobs,
        clock: E::Clock,
        seed: E::Seed,
        search: SearchEdges<E::Lexical, E::Vector, E::Embedder, E::Reranker>,
    ) -> Self {
        Self {
            registry,
            engine: Engine::new(store, clock),
            blobs,
            seed,
            rasterizer: E::Rasterizer::default(),
            lexical: search.lexical,
            projector: Mutex::new(LexicalProjector::default()),
            vector: search.vector,
            embedder: search.embedder,
            reranker: search.reranker,
            as_of_running: AtomicU64::new(0),
            login_limiter: Mutex::new(LoginLimiter::default()),
            vector_projector: Mutex::new(VectorProjector::default()),
            leases: Mutex::new(HashMap::new()),
            write_locks: Mutex::new(HashMap::new()),
            machine_auto_commit: AtomicBool::new(false),
            id_counter: AtomicU64::new(0),
            extraction: Mutex::new(None),
        }
    }

    pub fn blobs(&self) -> &E::Blobs {
        &self.blobs
    }

    pub fn registry(&self) -> &E::Registry {
        &self.registry
    }

    pub fn engine(&self) -> &Engine<E::Store, E::Clock> {
        &self.engine
    }

    fn now(&self) -> i64 {
        self.engine.clock().now_ms()
    }

    /// Time first, like a UUIDv7, then a counter, so ordering by id is ordering by creation even within a millisecond. Hex only, so it's safe as a folder or schema name.
    fn new_id(&self) -> String {
        let time = self.now().max(0) as u64 & 0xffff_ffff_ffff;
        let count = self.id_counter.fetch_add(1, Ordering::Relaxed) & 0xffff_ffff;
        format!("{time:012x}{count:08x}{:012x}", self.seed.next_u64() & 0xffff_ffff_ffff)
    }

    pub fn new_request_id(&self) -> String {
        format!("{:016x}", self.seed.next_u64())
    }

    /// True when the registry and through it the database answer.
    pub fn healthy(&self) -> bool {
        self.registry.principal_count().is_ok()
    }

    fn new_secret(&self) -> String {
        let mut s = String::from(TOKEN_PREFIX);
        for _ in 0..4 {
            let _ = write!(s, "{:016x}", self.seed.next_u64());
        }
        s
    }

    fn audit(&self, actor: &str, action: &str, target: &str) -> Audit {
        Audit {
            actor: actor.into(),
            time_ms: self.now(),
            action: action.into(),
            target: target.into(),
        }
    }

    fn allow(&self, caller: &Caller, op: Operation<'_>) -> Result<(), ServerError> {
        match authorize(caller, op) {
            Decision::Allow => Ok(()),
            Decision::Forbidden => Err(ServerError::Forbidden),
            Decision::Hidden => Err(ServerError::NotFound),
        }
    }

    /// Takes the writer lease of every known vault and cleans up after any crash. Call once at startup.
    pub fn open_vaults(&self) -> Result<usize, ServerError> {
        let (mut after, mut count) = (None, 0);
        loop {
            let page = self.registry.vaults(None, after.as_ref(), RESULT_LIMIT)?;
            let Some(last) = page.last() else { return Ok(count) };
            after = Some(last.id.clone());
            for v in &page {
                self.lease(&v.id)?;
                count += 1;
            }
        }
    }

    fn lease(&self, vault: &VaultId) -> Result<WriterLease, ServerError> {
        let mut leases = self.leases.lock().expect("lock");
        if let Some(l) = leases.get(vault) {
            return Ok(l.clone());
        }
        let lease = self.engine.store().open_vault(vault)?;
        self.blobs.purge_incomplete(vault)?;
        leases.insert(vault.clone(), lease.clone());
        Ok(lease)
    }

    /// The first-run step. Creates a server admin and returns its token once. Refused if any principal exists.
    pub fn bootstrap(&self, name: &str) -> Result<CreatedToken, ServerError> {
        check_name(name)?;
        if self.registry.principal_count()? > 0 {
            return Err(ServerError::AlreadyBootstrapped);
        }
        let id = self.new_id();
        let principal = Principal {
            id: id.clone(),
            name: name.into(),
            kind: PrincipalKind::Human,
            server_admin: true,
            disabled: false,
            created_by: "bootstrap".into(),
            created_at_ms: self.now(),
        };
        self.registry.insert_principal(&principal, &self.audit("bootstrap", "principal.create", &id))?;
        self.issue_token(
            &id,
            "bootstrap",
            NewToken {
                principal_id: None,
                name: "bootstrap".into(),
                grants: vec![],
                expires_at_ms: None,
            },
        )
    }

    fn issue_token(&self, principal_id: &str, actor: &str, new: NewToken) -> Result<CreatedToken, ServerError> {
        let token = self.new_secret();
        let row = TokenRow {
            id: self.new_id(),
            principal_id: principal_id.into(),
            hash: sha256_hex(&token),
            name: new.name,
            grants: new.grants,
            created_by: actor.into(),
            created_at_ms: self.now(),
            expires_at_ms: new.expires_at_ms,
            last_used_at_ms: None,
            revoked_at_ms: None,
        };
        self.registry.insert_token(&row, &self.audit(actor, "token.create", &row.id))?;
        Ok(CreatedToken { token, row })
    }

    /// Turns a bearer token into a caller. Every way of failing gives the same error.
    pub fn authenticate(&self, raw: &str) -> Result<Caller, ServerError> {
        if !raw.starts_with(TOKEN_PREFIX) || raw.len() > 256 {
            return Err(ServerError::Unauthenticated);
        }
        let row = self.registry.token_by_hash(&sha256_hex(raw))?.ok_or(ServerError::Unauthenticated)?;
        self.caller_from_token(row)
    }

    /// The checks a token has to pass, for bearer tokens and sessions alike. A session therefore ends the moment its token is revoked or its principal disabled.
    fn caller_from_token(&self, row: TokenRow) -> Result<Caller, ServerError> {
        let now = self.now();
        if row.revoked_at_ms.is_some() || row.expires_at_ms.is_some_and(|e| now >= e) {
            return Err(ServerError::Unauthenticated);
        }
        let principal = self.registry.principal(&row.principal_id)?.ok_or(ServerError::Unauthenticated)?;
        if principal.disabled {
            return Err(ServerError::Unauthenticated);
        }
        // Written at most once a minute, so reads don't all turn into writes.
        if row.last_used_at_ms.is_none_or(|t| now - t >= TOUCH_EVERY_MS) {
            self.registry.touch_token(&row.id, now)?;
        }
        Ok(Caller {
            principal,
            token_id: row.id,
            grants: row.grants,
        })
    }

    /// Exchanges a token for a browser session. `client` names who is asking, for the limit on failed attempts, and the limit holds even for a right token.
    pub fn login(&self, raw_token: &str, client: &str) -> Result<CreatedSession, ServerError> {
        let now = self.now();
        self.limiter()
            .check(client, now)
            .map_err(|retry_after_s| ServerError::RateLimited { retry_after_s })?;
        let caller = match self.authenticate(raw_token) {
            Ok(c) => c,
            Err(ServerError::Unauthenticated) => {
                self.limiter().failed(client, now);
                return Err(ServerError::Unauthenticated);
            }
            Err(e) => return Err(e),
        };
        self.registry.delete_expired_sessions(now)?;
        let mut session_id = String::from(SESSION_PREFIX);
        for _ in 0..4 {
            let _ = write!(session_id, "{:016x}", self.seed.next_u64());
        }
        let row = SessionRow {
            hash: sha256_hex(&session_id),
            token_id: caller.token_id.clone(),
            principal_id: caller.principal.id.clone(),
            created_at_ms: now,
            expires_at_ms: now + SESSION_TTL_MS,
        };
        self.registry
            .insert_session(&row, &self.audit(&caller.principal.id, "session.create", &caller.token_id))?;
        Ok(CreatedSession {
            session_id,
            caller,
            max_age_s: SESSION_TTL_MS / 1000,
        })
    }

    pub fn authenticate_session(&self, raw: &str) -> Result<Caller, ServerError> {
        if !raw.starts_with(SESSION_PREFIX) || raw.len() > 256 {
            return Err(ServerError::Unauthenticated);
        }
        let session = self.registry.session_by_hash(&sha256_hex(raw))?.ok_or(ServerError::Unauthenticated)?;
        if self.now() >= session.expires_at_ms {
            return Err(ServerError::Unauthenticated);
        }
        let token = self.registry.token(&session.token_id)?.ok_or(ServerError::Unauthenticated)?;
        self.caller_from_token(token)
    }

    /// Ends a session. One that is unknown or already gone is fine, since the result is the same.
    pub fn logout(&self, raw: &str) -> Result<(), ServerError> {
        let hash = sha256_hex(raw);
        let Some(session) = self.registry.session_by_hash(&hash)? else {
            return Ok(());
        };
        match self
            .registry
            .delete_session(&hash, &self.audit(&session.principal_id, "session.end", &session.token_id))
        {
            Err(RegistryError::NotFound) => Ok(()),
            other => Ok(other?),
        }
    }

    fn limiter(&self) -> std::sync::MutexGuard<'_, LoginLimiter> {
        self.login_limiter.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn create_vault(&self, caller: &Caller, name: &str) -> Result<VaultRow, ServerError> {
        self.allow(caller, Operation::VaultCreate)?;
        check_name(name)?;
        let row = VaultRow {
            id: VaultId(self.new_id()),
            name: name.into(),
            created_by: caller.principal.id.clone(),
            created_at_ms: self.now(),
        };
        self.registry.insert_vault(&row, &self.audit(&caller.principal.id, "vault.create", &row.id.0))?;
        self.lease(&row.id)?;
        Ok(row)
    }

    pub fn list_vaults(&self, caller: &Caller, after: Option<&VaultId>, limit: usize) -> Result<Vec<VaultRow>, ServerError> {
        self.allow(caller, Operation::VaultList)?;
        if caller.principal.server_admin {
            return Ok(self.registry.vaults(None, after, limit)?);
        }
        let only: Vec<VaultId> = caller.grants.iter().map(|g| g.vault.clone()).collect();
        Ok(self.registry.vaults(Some(&only), after, limit)?)
    }

    pub fn get_vault(&self, caller: &Caller, id: &VaultId) -> Result<VaultRow, ServerError> {
        self.allow(caller, Operation::VaultRead(id))?;
        self.registry.vault(id)?.ok_or(ServerError::NotFound)
    }

    pub fn rename_vault(&self, caller: &Caller, id: &VaultId, name: &str) -> Result<VaultRow, ServerError> {
        self.allow(caller, Operation::VaultRename(id))?;
        check_name(name)?;
        self.registry.rename_vault(id, name, &self.audit(&caller.principal.id, "vault.rename", &id.0))?;
        self.registry.vault(id)?.ok_or(ServerError::NotFound)
    }

    /// Removes the registry row first, so a failure part way leaves data nobody can reach rather than a vault with no data. Spec 030's sweep can clear what's left.
    pub fn delete_vault(&self, caller: &Caller, id: &VaultId) -> Result<(), ServerError> {
        self.allow(caller, Operation::VaultDelete(id))?;
        let lease = self.lease(id)?;
        self.registry.delete_vault(id, &self.audit(&caller.principal.id, "vault.delete", &id.0))?;
        self.leases.lock().expect("lock").remove(id);
        self.engine.store().delete_vault(&lease)?;
        self.blobs.delete_vault(id)?;
        self.projector.lock().expect("lock").forget(id);
        self.lexical.delete_vault(id)?;
        self.vector_projector.lock().expect("lock").forget(id);
        self.vector.delete_vault(id)?;
        Ok(())
    }

    /// Words and quoted phrases, with hits current as of this call even if the index hasn't caught up. Filters are the typed ones: there's no query syntax for them.
    pub fn search_lexical(&self, caller: &Caller, vault: &VaultId, text: &str, filter: LexicalFilter, limit: usize) -> Result<LexicalResults, ServerError> {
        self.allow(caller, Operation::VaultContentRead(vault))?;
        self.vault_exists(vault)?;
        let query = crate::lexical::build_query(text, filter, limit).map_err(|e| ServerError::Invalid(e.to_string()))?;
        let results = self.lexical.search(vault, &query)?;
        let snap = self.engine.store().snapshot(vault)?;
        Ok(crate::lexical::fence_hits(&snap, results)?)
    }

    /// One bounded piece of keyword index work for this vault. Only the projector thread calls it.
    pub fn project_lexical(&self, vault: &VaultId) -> Result<StepOutcome, ProjectError> {
        self.projector
            .lock()
            .expect("lock")
            .step(self.engine.store(), &self.lexical, self.engine.clock(), vault)
    }

    /// Builds a new keyword index generation. The current one keeps serving until it's ready.
    pub fn rebuild_lexical(&self, vault: &VaultId) {
        self.projector.lock().expect("lock").request_rebuild(vault);
    }

    pub fn lexical(&self) -> &E::Lexical {
        &self.lexical
    }

    /// Finds text by meaning, with hits current as of this call. The query is embedded by the model of the generation it runs against, so scores are always comparable.
    pub fn search_vector(&self, caller: &Caller, vault: &VaultId, text: &str, filter: LexicalFilter, limit: usize) -> Result<LexicalResults, ServerError> {
        self.allow(caller, Operation::VaultContentRead(vault))?;
        self.vault_exists(vault)?;
        check_query_text(text).map_err(|e| ServerError::Invalid(e.to_string()))?;
        let results = crate::search::vector_candidates(&self.vector, &self.embedder, vault, text, &filter, limit)?;
        let snap = self.engine.store().snapshot(vault)?;
        Ok(crate::lexical::fence_hits(&snap, results)?)
    }

    /// The search: an evidence package of what's current, or of a past state, ranked from every signal asked for.
    pub fn search(&self, caller: &Caller, vault: &VaultId, request: &memex_types::SearchRequest) -> Result<memex_types::EvidencePackage, ServerError> {
        self.allow(caller, Operation::VaultContentRead(vault))?;
        self.vault_exists(vault)?;
        let _slot = match request.as_of {
            Some(_) => Some(AsOfSlot::take(&self.as_of_running)?),
            None => None,
        };
        crate::search::run_search(
            &crate::search::SearchEdgesRef {
                store: self.engine.store(),
                lexical: &self.lexical,
                vector: &self.vector,
                embedder: &self.embedder,
                reranker: &self.reranker,
                clock: self.engine.clock(),
            },
            vault,
            request,
        )
    }

    /// One bounded piece of vector index work for this vault. Only the projector thread calls it.
    pub fn project_vector(&self, vault: &VaultId) -> Result<StepOutcome, ProjectError> {
        let mut enqueue = |requests: &[EmbedRequest]| self.enqueue_embeds(vault, requests).map_err(|e| e.to_string());
        self.vector_projector
            .lock()
            .expect("lock")
            .step(self.engine.store(), &self.vector, &self.embedder, vault, &mut enqueue)
    }

    /// Creates the jobs that don't exist yet, in one event.
    fn enqueue_embeds(&self, vault: &VaultId, requests: &[EmbedRequest]) -> Result<(), ServerError> {
        let lease = self.lease(vault)?;
        let lock = self.vault_lock(vault);
        let _guard = lock.lock().expect("lock");
        let now = self.now();
        let mut mutations = vec![];
        {
            let snap = self.engine.store().snapshot(vault)?;
            for r in requests {
                if snap.job_by_dedupe(&r.dedupe_key)?.is_none() {
                    let input = serde_json::to_value(&r.input).map_err(|e| ServerError::Invalid(e.to_string()))?;
                    let job = jobs::new_job(ObjectId(self.new_id()), "embedder", EMBED_JOB, input, r.dedupe_key.clone(), EMBED_ATTEMPTS, now);
                    mutations.push(Mutation::Put(Row::Job(job)));
                }
            }
        }
        if !mutations.is_empty() {
            self.state_event(&lease, "embedder", mutations)?;
        }
        Ok(())
    }

    /// An embed job's work. It reads the objects' current text and keeps its results in the embedding store, so the job's completion only has to announce them.
    pub fn run_embed_job(&self, vault: &VaultId, job: &JobRow) -> Result<usize, EmbedJobError> {
        let input: EmbedInput = serde_json::from_value(job.input.clone()).map_err(EmbedJobError::permanent)?;
        let snap = self.engine.store().snapshot(vault).map_err(EmbedJobError::retry)?;
        let ids_of =
            |kind: memex_types::DocKind| -> Vec<ObjectId> { input.docs.iter().filter(|d: &&EmbedDocRef| d.kind == kind).map(|d| d.id.clone()).collect() };
        let mut ops = crate::lexical::segment_ops(&snap, &ids_of(memex_types::DocKind::Segment)).map_err(EmbedJobError::retry)?;
        ops.extend(crate::lexical::assertion_ops(&snap, &ids_of(memex_types::DocKind::Claim)).map_err(EmbedJobError::retry)?);
        ops.extend(crate::entity::entity_ops(&snap, &ids_of(memex_types::DocKind::Entity)).map_err(EmbedJobError::retry)?);
        ops.extend(crate::page::page_ops(&snap, &ids_of(memex_types::DocKind::Page)).map_err(EmbedJobError::retry)?);
        ops.extend(crate::page::block_ops(&snap, &ids_of(memex_types::DocKind::Block)).map_err(EmbedJobError::retry)?);
        let docs: Vec<memex_types::IndexDoc> = ops
            .into_iter()
            .filter_map(|op| match op {
                memex_types::IndexOp::Upsert(d) => Some(d),
                memex_types::IndexOp::Delete(_) => None,
            })
            .collect();
        let docs = embeddable(&snap, docs).map_err(EmbedJobError::retry)?;
        drop(snap);
        embed_missing(&self.vector, &self.embedder, vault, &input.model, docs)
    }

    /// Refuses to start on a generation the embedder can't serve: a model that's gone, or one whose dimension changed under the same name.
    pub fn check_embedding_models(&self) -> Result<(), ServerError> {
        for vault in self.vault_ids()? {
            if let memex_types::VectorHealth::Ready { model, .. } = self.vector.health(&vault)? {
                match self.embedder.model(&model.name) {
                    None => {
                        return Err(ServerError::Invalid(format!(
                            "vault {vault} is indexed with embedding model {}, which isn't available",
                            model.name
                        )));
                    }
                    Some(offered) if offered.dimension != model.dimension => {
                        return Err(ServerError::Invalid(format!(
                            "vault {vault} is indexed with {} at {} dimensions, and the model now has {}",
                            model.name, model.dimension, offered.dimension
                        )));
                    }
                    Some(_) => {}
                }
            }
        }
        Ok(())
    }

    /// Builds a new vector index generation from stored embeddings. The current one keeps serving until it's ready.
    pub fn rebuild_vector(&self, vault: &VaultId) {
        self.vector_projector.lock().expect("lock").request_rebuild(vault, self.embedder.active());
    }

    /// Forgets the vector projector's progress, as a restart would.
    pub fn reset_vector_projector_for_tests(&self) {
        *self.vector_projector.lock().expect("lock") = VectorProjector::default();
    }

    /// The writer lease this server holds on a vault, for tests that write directly.
    pub fn vault_lease_for_tests(&self, vault: &VaultId) -> Result<WriterLease, ServerError> {
        self.lease(vault)
    }

    pub fn vector(&self) -> &E::Vector {
        &self.vector
    }

    pub fn embedder(&self) -> &E::Embedder {
        &self.embedder
    }

    pub fn create_principal(&self, caller: &Caller, name: &str, kind: PrincipalKind, server_admin: bool) -> Result<Principal, ServerError> {
        self.allow(caller, Operation::PrincipalCreate)?;
        check_name(name)?;
        let p = Principal {
            id: self.new_id(),
            name: name.into(),
            kind,
            server_admin,
            disabled: false,
            created_by: caller.principal.id.clone(),
            created_at_ms: self.now(),
        };
        self.registry
            .insert_principal(&p, &self.audit(&caller.principal.id, "principal.create", &p.id))?;
        Ok(p)
    }

    pub fn list_principals(&self, caller: &Caller, after: Option<&str>, limit: usize) -> Result<Vec<Principal>, ServerError> {
        self.allow(caller, Operation::PrincipalList)?;
        Ok(self.registry.principals(after, limit)?)
    }

    pub fn disable_principal(&self, caller: &Caller, id: &str) -> Result<(), ServerError> {
        self.allow(caller, Operation::PrincipalDisable)?;
        if id == caller.principal.id {
            return Err(ServerError::Invalid("you can't disable yourself, as that could leave no server admin".into()));
        }
        Ok(self
            .registry
            .set_principal_disabled(id, true, &self.audit(&caller.principal.id, "principal.disable", id))?)
    }

    pub fn create_token(&self, caller: &Caller, new: NewToken) -> Result<CreatedToken, ServerError> {
        self.allow(caller, Operation::TokenCreate { grants: &new.grants })?;
        check_name(&new.name)?;
        if new.expires_at_ms.is_some_and(|e| e <= self.now()) {
            return Err(ServerError::Invalid("the token would already be expired".into()));
        }
        for g in &new.grants {
            if self.registry.vault(&g.vault)?.is_none() {
                return Err(ServerError::NotFound);
            }
        }
        let owner = match &new.principal_id {
            Some(id) => {
                let target = self.registry.principal(id)?.ok_or(ServerError::NotFound)?;
                // A token carries its owner's server-admin rights, so minting one for an admin would be an escalation.
                if target.server_admin && !caller.principal.server_admin {
                    return Err(ServerError::Forbidden);
                }
                id.clone()
            }
            None => caller.principal.id.clone(),
        };
        self.issue_token(&owner, &caller.principal.id, new)
    }

    pub fn list_tokens(&self, caller: &Caller, all: bool, after: Option<&str>, limit: usize) -> Result<Vec<TokenRow>, ServerError> {
        self.allow(caller, Operation::TokenList { all })?;
        let owner = if all { None } else { Some(caller.principal.id.as_str()) };
        Ok(self.registry.tokens(owner, after, limit)?)
    }

    pub fn revoke_token(&self, caller: &Caller, id: &str) -> Result<(), ServerError> {
        let row = self.registry.token(id)?;
        let owner = row.as_ref().map_or("", |r| r.principal_id.as_str());
        self.allow(caller, Operation::TokenRevoke { owner })?;
        if row.is_none() {
            return Err(ServerError::NotFound);
        }
        Ok(self
            .registry
            .revoke_token(id, self.now(), &self.audit(&caller.principal.id, "token.revoke", id))?)
    }

    /// The one way for content writes to reach a vault. The actor comes from the caller, never from a request body, so no event lacks one.
    pub fn apply_as(&self, caller: &Caller, vault: &VaultId, kind: &str, schema_version: u32, payload: Value) -> Result<Receipt, ServerError> {
        self.allow(caller, Operation::VaultContentPropose(vault))?;
        if self.registry.vault(vault)?.is_none() {
            return Err(ServerError::NotFound);
        }
        let lease = self.lease(vault)?;
        let draft = EventDraft {
            actor: caller.principal.id.clone(),
            kind: kind.into(),
            schema_version,
            payload,
        };
        let vault_lock = self.vault_lock(vault);
        let _one_at_a_time = vault_lock.lock().expect("lock");
        self.engine.apply(&lease, draft).map_err(|e| match e {
            ApplyError::Store(s) => s.into(),
            other => ServerError::Invalid(other.to_string()),
        })
    }

    /// Events of a vault, for callers with read scope.
    pub fn read_events(&self, caller: &Caller, vault: &VaultId, after: Option<u64>) -> Result<Vec<Event>, ServerError> {
        use crate::Snapshot;
        self.allow(caller, Operation::VaultContentRead(vault))?;
        if self.registry.vault(vault)?.is_none() {
            return Err(ServerError::NotFound);
        }
        Ok(self.engine.store().snapshot(vault)?.events(after)?)
    }

    /// Lets machine proposals auto-commit. Spec 025 earns this per extractor, and until then it stays off.
    pub fn set_machine_auto_commit(&self, on: bool) {
        self.machine_auto_commit.store(on, Ordering::SeqCst);
    }

    fn vault_lock(&self, vault: &VaultId) -> Arc<Mutex<()>> {
        self.write_locks.lock().expect("lock").entry(vault.clone()).or_default().clone()
    }

    fn vault_exists(&self, vault: &VaultId) -> Result<(), ServerError> {
        self.registry.vault(vault)?.map(|_| ()).ok_or(ServerError::NotFound)
    }

    fn validate<S: Snapshot>(&self, snap: &S, p: &Proposal) -> Result<Validated, ServerError> {
        let schema = governance::validate_schema(&p.ops, &p.rationale, &p.idempotency_key);
        let evidence = governance::validate_evidence_required(&p.ops);
        if !schema.passed {
            return Ok((vec![schema, evidence], vec![]));
        }
        let quote = governance::validate_quotes(snap, &p.ops)?;
        let (dedup, check) = governance::validate_dedup(snap, &p.ops)?;
        let entity = governance::validate_entities(snap, &p.ops)?;
        let page = governance::validate_pages(snap, &p.ops)?;
        Ok((vec![schema, evidence, quote, dedup, entity, page], check.contradictions))
    }

    fn commit_changeset(&self, lease: &WriterLease, actor: &str, changeset: ChangeSetRow, mutations: Vec<Mutation>) -> Result<(), ServerError> {
        let payload = serde_json::to_value(ChangeSetRecorded { changeset, mutations }).map_err(|e| ServerError::Invalid(e.to_string()))?;
        let draft = EventDraft {
            actor: actor.into(),
            kind: "changeset.recorded".into(),
            schema_version: 1,
            payload,
        };
        self.engine.apply(lease, draft).map(|_| ()).map_err(|e| match e {
            ApplyError::Store(s) => s.into(),
            other => ServerError::Invalid(other.to_string()),
        })
    }

    fn plan_errors(e: PlanError) -> ServerError {
        match e {
            PlanError::Stale { key, current, base } => ServerError::StaleVersion { key, current, base },
            PlanError::Missing(k) => ServerError::Invalid(format!("{:?} {} doesn't exist", k.table, k.id)),
            PlanError::Exists(k) => ServerError::Conflict(format!("{:?} {} already exists", k.table, k.id)),
            PlanError::Invalid(m) => ServerError::Invalid(m),
            PlanError::Store(s) => s.into(),
        }
    }

    /// The one write path for knowledge. A dry run does everything up to deciding, and writes nothing.
    pub fn propose(&self, caller: &Caller, vault: &VaultId, p: Proposal, dry_run: bool) -> Result<ProposalOutcome, ServerError> {
        self.allow(caller, Operation::VaultContentPropose(vault))?;
        // Whether a claim is a machine's comes from who holds the token, never from the request.
        self.propose_as(&caller.principal.id, caller.principal.kind == PrincipalKind::Agent, vault, p, dry_run)
    }

    /// `propose` for someone already allowed to: a caller, or a job of the server's own.
    fn propose_as(&self, actor: &str, machine: bool, vault: &VaultId, p: Proposal, dry_run: bool) -> Result<ProposalOutcome, ServerError> {
        self.vault_exists(vault)?;
        crate::page::check_sizes(&p.ops).map_err(ServerError::Invalid)?;
        let lease = self.lease(vault)?;
        let vault_lock = self.vault_lock(vault);
        let _one_at_a_time = vault_lock.lock().expect("lock");
        let snap = self.engine.store().snapshot(vault)?;
        if let Some(prior) = snap.changeset_by_key(&p.idempotency_key)? {
            return Ok(ProposalOutcome {
                validators: prior.validators.clone(),
                risk: prior.risk,
                contradictions: vec![],
                replayed: true,
                dry_run,
                changeset: Some(prior),
            });
        }
        let mut p = p;
        for op in &mut p.ops {
            mark_machine(op, machine);
        }
        let (validators, contradictions) = self.validate(&snap, &p)?;
        let passed = validators.iter().all(|v| v.passed);
        let kinds = governance::kinds_of(&snap, &p.ops)?;
        let decision = governance::risk(&kinds, Author { machine }, self.machine_auto_commit.load(Ordering::SeqCst), passed);
        let actor = actor.to_string();
        let id = ObjectId(self.new_id());
        let now = self.now();
        let mut row = ChangeSetRow {
            header: Header {
                id: id.clone(),
                creator: actor.clone(),
                status: Status::Active,
                version: 1,
                acl_id: None,
            },
            state: match decision {
                RiskDecision::Reject => ChangeSetState::Rejected,
                RiskDecision::Review => ChangeSetState::Pending,
                RiskDecision::AutoCommit => ChangeSetState::Committed,
            },
            proposer: actor.clone(),
            proposer_is_machine: machine,
            rationale: p.rationale,
            idempotency_key: p.idempotency_key,
            ops: p.ops,
            validators: validators.clone(),
            risk: decision,
            proposed_at_ms: now,
            decided_by: None,
            decided_at_ms: if decision == RiskDecision::Review { None } else { Some(now) },
            decision_reason: if decision == RiskDecision::Reject {
                validators.iter().find(|v| !v.passed).and_then(|v| v.detail.clone())
            } else {
                None
            },
            commit_seq: None,
            applied: vec![],
            inverse: vec![],
            reverts: None,
            reverted_by: None,
        };
        // A dry run stops before anything is written, but still catches a stale base version.
        let mut mutations = vec![];
        if decision == RiskDecision::AutoCommit {
            let seed = &self.seed;
            let mut edge_id = || ObjectId(format!("{:012x}{:020x}", now.max(0), u128::from(seed.next_u64())));
            mutations = governance::plan(&snap, &row.ops, &actor, &id, now, &contradictions, &mut edge_id).map_err(Self::plan_errors)?;
            self.anchor_new_evidence(vault, &snap, &mut mutations)?;
            row.inverse = governance::inverse_of(&snap, &mutations)?;
            row.applied = mutations.clone();
            row.commit_seq = snap.head()?.map(|h| h + 1).or(Some(0));
        }
        drop(snap);
        if dry_run {
            return Ok(ProposalOutcome {
                changeset: None,
                validators,
                risk: decision,
                contradictions,
                replayed: false,
                dry_run: true,
            });
        }
        self.commit_changeset(&lease, &actor, row.clone(), mutations)?;
        Ok(ProposalOutcome {
            changeset: Some(row),
            validators,
            risk: decision,
            contradictions,
            replayed: false,
            dry_run: false,
        })
    }

    fn changeset(&self, vault: &VaultId, id: &ObjectId) -> Result<ChangeSetRow, ServerError> {
        match self.engine.store().snapshot(vault)?.get(&RowKey {
            table: Table::ChangeSet,
            id: id.clone(),
        })? {
            Some(Row::ChangeSet(c)) => Ok(c),
            _ => Err(ServerError::NotFound),
        }
    }

    pub fn get_changeset(&self, caller: &Caller, vault: &VaultId, id: &ObjectId) -> Result<ChangeSetRow, ServerError> {
        self.allow(caller, Operation::VaultContentRead(vault))?;
        self.vault_exists(vault)?;
        self.changeset(vault, id)
    }

    /// In id order, which is creation order. Optionally only one state, such as the review queue.
    pub fn list_changesets(
        &self,
        caller: &Caller,
        vault: &VaultId,
        state: Option<ChangeSetState>,
        after: Option<&ObjectId>,
        limit: usize,
    ) -> Result<Vec<ChangeSetRow>, ServerError> {
        self.allow(caller, Operation::VaultContentRead(vault))?;
        self.vault_exists(vault)?;
        let snap = self.engine.store().snapshot(vault)?;
        let (mut out, mut cursor) = (vec![], after.cloned());
        loop {
            let page = snap.rows(Table::ChangeSet, cursor.as_ref())?;
            let Some(last) = page.last() else { return Ok(out) };
            cursor = Some(last.header().id.clone());
            for r in page {
                if let Row::ChangeSet(c) = r
                    && state.is_none_or(|s| s == c.state)
                {
                    out.push(c);
                    if out.len() == limit {
                        return Ok(out);
                    }
                }
            }
        }
    }

    /// Approving needs admin scope. The reviewer is recorded.
    pub fn approve(&self, caller: &Caller, vault: &VaultId, id: &ObjectId, reason: Option<String>) -> Result<ChangeSetRow, ServerError> {
        self.allow(caller, Operation::VaultContentReview(vault))?;
        self.vault_exists(vault)?;
        let lease = self.lease(vault)?;
        let vault_lock = self.vault_lock(vault);
        let _one_at_a_time = vault_lock.lock().expect("lock");
        let snap = self.engine.store().snapshot(vault)?;
        let mut row = self.changeset(vault, id)?;
        if row.state != ChangeSetState::Pending {
            return Err(ServerError::Conflict(format!("this ChangeSet is already {:?}", row.state)));
        }
        self.only_a_person(caller, &snap, &row)?;
        let now = self.now();
        let actor = caller.principal.id.clone();
        // Validators run again, since the vault may have moved on during review.
        let proposal = Proposal {
            ops: row.ops.clone(),
            rationale: row.rationale.clone(),
            idempotency_key: row.idempotency_key.clone(),
        };
        let (validators, contradictions) = self.validate(&snap, &proposal)?;
        if let Some(failed) = validators.iter().find(|v| !v.passed) {
            return Err(ServerError::Invalid(format!("no longer valid: {}", failed.detail.clone().unwrap_or_default())));
        }
        let seed = &self.seed;
        let mut edge_id = || ObjectId(format!("{:012x}{:020x}", now.max(0), u128::from(seed.next_u64())));
        let mut mutations = governance::plan(&snap, &row.ops, &row.proposer, id, now, &contradictions, &mut edge_id).map_err(Self::plan_errors)?;
        self.anchor_new_evidence(vault, &snap, &mut mutations)?;
        row.inverse = governance::inverse_of(&snap, &mutations)?;
        row.applied = mutations.clone();
        row.state = ChangeSetState::Committed;
        row.validators = validators;
        row.decided_by = Some(actor.clone());
        row.decided_at_ms = Some(now);
        row.decision_reason = reason;
        row.commit_seq = snap.head()?.map(|h| h + 1).or(Some(0));
        row.header.version += 1;
        drop(snap);
        self.commit_changeset(&lease, &actor, row.clone(), mutations)?;
        Ok(row)
    }

    pub fn reject(&self, caller: &Caller, vault: &VaultId, id: &ObjectId, reason: String) -> Result<ChangeSetRow, ServerError> {
        self.allow(caller, Operation::VaultContentReview(vault))?;
        self.vault_exists(vault)?;
        if reason.trim().is_empty() {
            return Err(ServerError::Invalid("a rejection needs a reason".into()));
        }
        let lease = self.lease(vault)?;
        let vault_lock = self.vault_lock(vault);
        let _one_at_a_time = vault_lock.lock().expect("lock");
        let mut row = self.changeset(vault, id)?;
        if row.state != ChangeSetState::Pending {
            return Err(ServerError::Conflict(format!("this ChangeSet is already {:?}", row.state)));
        }
        let actor = caller.principal.id.clone();
        row.state = ChangeSetState::Rejected;
        row.decided_by = Some(actor.clone());
        row.decided_at_ms = Some(self.now());
        row.decision_reason = Some(reason);
        row.header.version += 1;
        self.commit_changeset(&lease, &actor, row.clone(), vec![])?;
        Ok(row)
    }

    /// Whether a revert would work now, and what it would do, with nothing changed.
    pub fn inverse_preview(&self, caller: &Caller, vault: &VaultId, id: &ObjectId) -> Result<(Vec<Mutation>, Option<String>), ServerError> {
        self.allow(caller, Operation::VaultContentRead(vault))?;
        self.vault_exists(vault)?;
        let row = self.changeset(vault, id)?;
        let snap = self.engine.store().snapshot(vault)?;
        let mut blocked = governance::check_reversible(&snap, &row)?.err().map(|e| Self::why_not(&e));
        if blocked.is_none() {
            blocked = entities::type_revert_blocked(&snap, &row)?;
        }
        Ok((row.inverse, blocked))
    }

    fn why_not(e: &NotReversible) -> String {
        match e {
            NotReversible::AlreadyReverted => "it was already reverted".into(),
            NotReversible::NotCommitted => "it was never committed".into(),
            NotReversible::ChangedSince { key } => format!("{:?} {} has changed since", key.table, key.id),
        }
    }

    /// A revert is itself a committed ChangeSet, so it shows in history and can't be lost. Refused up front if it would overwrite later work.
    pub fn revert(&self, caller: &Caller, vault: &VaultId, id: &ObjectId, reason: String) -> Result<ChangeSetRow, ServerError> {
        self.allow(caller, Operation::VaultContentReview(vault))?;
        self.vault_exists(vault)?;
        let lease = self.lease(vault)?;
        let vault_lock = self.vault_lock(vault);
        let _one_at_a_time = vault_lock.lock().expect("lock");
        let mut target = self.changeset(vault, id)?;
        let snap = self.engine.store().snapshot(vault)?;
        if let Err(e) = governance::check_reversible(&snap, &target)? {
            return Err(ServerError::NotReversible(Self::why_not(&e)));
        }
        self.only_a_person(caller, &snap, &target)?;
        if let Some(why) = entities::type_revert_blocked(&snap, &target)? {
            return Err(ServerError::NotReversible(why));
        }
        let now = self.now();
        let actor = caller.principal.id.clone();
        let revert_id = ObjectId(self.new_id());
        let mut mutations = target.inverse.clone();
        let inverse = governance::inverse_of(&snap, &mutations)?;
        let commit_seq = snap.head()?.map(|h| h + 1).or(Some(0));
        drop(snap);
        target.reverted_by = Some(revert_id.clone());
        target.header.version += 1;
        // The target's row changes in the same event, so a crash can't leave a half-recorded revert.
        mutations.push(Mutation::Put(Row::ChangeSet(target.clone())));
        let row = ChangeSetRow {
            header: Header {
                id: revert_id.clone(),
                creator: actor.clone(),
                status: Status::Active,
                version: 1,
                acl_id: None,
            },
            state: ChangeSetState::Committed,
            proposer: actor.clone(),
            proposer_is_machine: false,
            rationale: reason,
            idempotency_key: format!("revert:{id}"),
            ops: vec![],
            validators: vec![],
            risk: RiskDecision::AutoCommit,
            proposed_at_ms: now,
            decided_by: Some(actor.clone()),
            decided_at_ms: Some(now),
            decision_reason: None,
            commit_seq,
            applied: target.inverse.clone(),
            inverse,
            reverts: Some(id.clone()),
            reverted_by: None,
        };
        self.commit_changeset(&lease, &actor, row.clone(), mutations)?;
        Ok(row)
    }

    /// Events after a sequence number, for consumers following changes. Metadata and results, as the log recorded them.
    pub fn changes_since(&self, caller: &Caller, vault: &VaultId, after: Option<u64>) -> Result<Vec<Event>, ServerError> {
        self.read_events(caller, vault, after)
    }

    /// What the vault believed about a subject at a past sequence.
    pub fn believed_at(&self, caller: &Caller, vault: &VaultId, subject: &str, seq: u64) -> Result<Vec<memex_types::AssertionRow>, ServerError> {
        self.allow(caller, Operation::VaultContentRead(vault))?;
        self.vault_exists(vault)?;
        let snap = self.engine.store().snapshot(vault)?;
        let (mut all, mut after) = (vec![], None);
        loop {
            let page = snap.events(after)?;
            let Some(last) = page.last() else { break };
            after = Some(last.seq);
            let done = last.seq >= seq;
            all.extend(page);
            if done {
                break;
            }
        }
        Ok(governance::believed_at(&all, seq, subject))
    }

    /// Claims that disagree and are both still active, as id pairs.
    pub fn open_contradictions(&self, caller: &Caller, vault: &VaultId) -> Result<Vec<(ObjectId, ObjectId)>, ServerError> {
        self.allow(caller, Operation::VaultContentRead(vault))?;
        self.vault_exists(vault)?;
        let snap = self.engine.store().snapshot(vault)?;
        let (mut edges, mut after) = (vec![], None);
        loop {
            let page = snap.rows(Table::Edge, after.as_ref())?;
            let Some(last) = page.last() else { break };
            after = Some(last.header().id.clone());
            for r in page {
                if let Row::Edge(e) = r
                    && e.kind == governance::CONTRADICTS
                {
                    edges.push(e);
                }
            }
        }
        let keys: Vec<RowKey> = edges
            .iter()
            .flat_map(|e| {
                [
                    RowKey {
                        table: Table::Assertion,
                        id: e.from.clone(),
                    },
                    RowKey {
                        table: Table::Assertion,
                        id: e.to.clone(),
                    },
                ]
            })
            .collect();
        let rows = snap.get_many(&keys)?;
        let active = |r: &Option<Row>| matches!(r, Some(Row::Assertion(a)) if a.header.status == Status::Active);
        Ok(edges
            .iter()
            .zip(rows.chunks(2))
            .filter(|(_, pair)| active(&pair[0]) && active(&pair[1]))
            .map(|(e, _)| (e.from.clone(), e.to.clone()))
            .collect())
    }

    /// Stores a segment directly. Spec 015's rendition job replaces this with its pipeline.
    pub fn record_segment(
        &self,
        caller: &Caller,
        vault: &VaultId,
        id: &ObjectId,
        source_id: &ObjectId,
        language: Option<&str>,
        text: &str,
    ) -> Result<Receipt, ServerError> {
        self.apply_as(
            caller,
            vault,
            "segment.recorded",
            1,
            json!({"id": id, "source_id": source_id, "language": language, "text": text}),
        )
    }

    fn state_event(&self, lease: &WriterLease, actor: &str, mutations: Vec<Mutation>) -> Result<(), ServerError> {
        let payload = serde_json::to_value(StateRecorded { mutations }).map_err(|e| ServerError::Invalid(e.to_string()))?;
        let draft = EventDraft {
            actor: actor.into(),
            kind: "state.recorded".into(),
            schema_version: 1,
            payload,
        };
        self.engine.apply(lease, draft).map(|_| ()).map_err(|e| match e {
            ApplyError::Store(s) => s.into(),
            other => ServerError::Invalid(other.to_string()),
        })
    }

    /// Streams an upload to the blob store, then commits the Source and its rendition job in one transaction. A failure before that commit leaves at most an orphan blob for the sweep.
    pub fn upload(
        &self,
        caller: &Caller,
        vault: &VaultId,
        req: UploadRequest,
        limits: UploadLimits,
        bytes: &mut dyn Read,
    ) -> Result<UploadOutcome, ServerError> {
        self.allow(caller, Operation::VaultContentPropose(vault))?;
        self.vault_exists(vault)?;
        check_name(&req.name)?;
        if !sources::TRUST_TIERS.contains(&req.trust_tier.as_str()) {
            return Err(ServerError::Invalid(format!("unknown trust tier '{}'", req.trust_tier)));
        }
        let lease = self.lease(vault)?;
        if self.engine.store().snapshot(vault)?.open_job_count(&[crate::extraction::EXTRACT_JOB])? >= limits.max_open_jobs {
            return Err(ServerError::Busy);
        }
        let mut reader = LimitedSniffer {
            inner: bytes,
            limit: limits.max_bytes,
            seen: 0,
            prefix: Vec::with_capacity(4096),
            over: false,
        };
        let stored = match self.blobs.put(vault, &mut reader) {
            Ok(s) => s,
            Err(_) if reader.over => return Err(ServerError::TooLarge { limit: limits.max_bytes }),
            Err(BlobStoreError::Input(m)) => return Err(ServerError::Invalid(format!("the upload was interrupted: {m}"))),
            Err(e) => return Err(e.into()),
        };
        let refuse = |error: ServerError| {
            if stored.newly_stored {
                // Nothing references it yet, and the sweep would get it anyway.
                let _ = self.blobs.delete(vault, &stored.hash);
            }
            error
        };
        if stored.size == 0 {
            return Err(refuse(ServerError::Invalid("the upload is empty".into())));
        }
        let Some(media_type) = sources::sniff_media_type(&reader.prefix, &req.name) else {
            return Err(refuse(ServerError::UnsupportedMedia("not a PDF or text file".into())));
        };
        let vault_lock = self.vault_lock(vault);
        let _one_at_a_time = vault_lock.lock().expect("lock");
        let snap = self.engine.store().snapshot(vault)?;
        let sha = stored.hash.to_string();
        if let Some(existing) = snap.source_by_hash(&sha)? {
            let job = snap.job_by_dedupe(&rendition_key(&sha))?;
            return Ok(UploadOutcome {
                source: existing,
                job,
                existing: true,
            });
        }
        let (logical_id, version_no) = match &req.replaces {
            None => (None, 1),
            Some(old) => match snap.get(&RowKey {
                table: Table::Source,
                id: old.clone(),
            })? {
                Some(Row::Source(s)) => {
                    let versions = snap.source_versions(&s.logical_id)?;
                    (Some(s.logical_id), versions.last().map_or(1, |v| v.version_no + 1))
                }
                _ => return Err(ServerError::NotFound),
            },
        };
        drop(snap);
        let now = self.now();
        let actor = caller.principal.id.clone();
        let id = ObjectId(self.new_id());
        let source = SourceRow {
            header: Header {
                id: id.clone(),
                creator: actor.clone(),
                status: Status::Active,
                version: 1,
                acl_id: None,
            },
            sha256: sha.clone(),
            media_type: media_type.into(),
            trust_tier: req.trust_tier,
            recorded_at_ms: now,
            name: req.name,
            size: stored.size,
            logical_id: logical_id.unwrap_or_else(|| id.clone()),
            version_no,
            metadata: req.metadata,
        };
        let job = jobs::new_job(
            ObjectId(self.new_id()),
            &actor,
            RENDITION_JOB,
            json!({"source_id": id, "sha256": sha, "media_type": media_type}),
            rendition_key(&sha),
            5,
            now,
        );
        self.state_event(
            &lease,
            &actor,
            vec![Mutation::Put(Row::Source(source.clone())), Mutation::Put(Row::Job(job.clone()))],
        )?;
        Ok(UploadOutcome {
            source,
            job: Some(job),
            existing: false,
        })
    }

    pub fn list_sources(&self, caller: &Caller, vault: &VaultId, after: Option<&ObjectId>, limit: usize) -> Result<Vec<SourceRow>, ServerError> {
        self.allow(caller, Operation::VaultContentRead(vault))?;
        self.vault_exists(vault)?;
        let snap = self.engine.store().snapshot(vault)?;
        let (mut out, mut cursor) = (vec![], after.cloned());
        loop {
            let page = snap.rows(Table::Source, cursor.as_ref())?;
            let Some(last) = page.last() else { return Ok(out) };
            cursor = Some(last.header().id.clone());
            for r in page {
                if let Row::Source(s) = r {
                    out.push(s);
                    if out.len() == limit {
                        return Ok(out);
                    }
                }
            }
        }
    }

    pub fn get_source(&self, caller: &Caller, vault: &VaultId, id: &ObjectId) -> Result<SourceRow, ServerError> {
        self.allow(caller, Operation::VaultContentRead(vault))?;
        self.vault_exists(vault)?;
        match self.engine.store().snapshot(vault)?.get(&RowKey {
            table: Table::Source,
            id: id.clone(),
        })? {
            Some(Row::Source(s)) => Ok(s),
            _ => Err(ServerError::NotFound),
        }
    }

    pub fn source_versions(&self, caller: &Caller, vault: &VaultId, id: &ObjectId) -> Result<Vec<SourceRow>, ServerError> {
        let s = self.get_source(caller, vault, id)?;
        Ok(self.engine.store().snapshot(vault)?.source_versions(&s.logical_id)?)
    }

    pub fn list_jobs(
        &self,
        caller: &Caller,
        vault: &VaultId,
        kind: Option<&str>,
        state: Option<JobState>,
        after: Option<&ObjectId>,
        limit: usize,
    ) -> Result<Vec<JobRow>, ServerError> {
        self.allow(caller, Operation::VaultContentRead(vault))?;
        self.vault_exists(vault)?;
        Ok(self.engine.store().snapshot(vault)?.jobs(kind, state, after, limit)?)
    }

    fn job(&self, vault: &VaultId, id: &ObjectId) -> Result<JobRow, ServerError> {
        match self.engine.store().snapshot(vault)?.get(&RowKey {
            table: Table::Job,
            id: id.clone(),
        })? {
            Some(Row::Job(j)) => Ok(j),
            _ => Err(ServerError::NotFound),
        }
    }

    pub fn get_job(&self, caller: &Caller, vault: &VaultId, id: &ObjectId) -> Result<JobRow, ServerError> {
        self.allow(caller, Operation::VaultContentRead(vault))?;
        self.vault_exists(vault)?;
        self.job(vault, id)
    }

    pub fn retry_job(&self, caller: &Caller, vault: &VaultId, id: &ObjectId) -> Result<JobRow, ServerError> {
        self.change_job(caller, vault, id, jobs::retried, "only a dead or cancelled job can be retried")
    }

    pub fn cancel_job(&self, caller: &Caller, vault: &VaultId, id: &ObjectId) -> Result<JobRow, ServerError> {
        self.change_job(caller, vault, id, jobs::cancelled, "only unfinished work can be cancelled")
    }

    fn change_job(
        &self,
        caller: &Caller,
        vault: &VaultId,
        id: &ObjectId,
        change: impl FnOnce(JobRow, i64) -> Option<JobRow>,
        refused: &str,
    ) -> Result<JobRow, ServerError> {
        self.allow(caller, Operation::VaultContentPropose(vault))?;
        self.vault_exists(vault)?;
        let lease = self.lease(vault)?;
        let vault_lock = self.vault_lock(vault);
        let _one_at_a_time = vault_lock.lock().expect("lock");
        let job = self.job(vault, id)?;
        let changed = change(job, self.now()).ok_or_else(|| ServerError::Conflict(refused.into()))?;
        self.state_event(&lease, &caller.principal.id, vec![Mutation::Put(Row::Job(changed.clone()))])?;
        Ok(changed)
    }

    /// Takes the oldest runnable job of a kind for a worker. Its attempt count is the fence: a worker whose lease expired can't commit afterwards.
    pub fn claim_job(&self, vault: &VaultId, kind: &str, owner: &str) -> Result<Option<JobRow>, ServerError> {
        let lease = self.lease(vault)?;
        let vault_lock = self.vault_lock(vault);
        let _one_at_a_time = vault_lock.lock().expect("lock");
        loop {
            let now = self.now();
            let found = self.engine.store().snapshot(vault)?.claimable_jobs(kind, now, 1)?;
            let Some(job) = found.into_iter().next() else { return Ok(None) };
            let next = jobs::claimed(job, owner, now);
            self.state_event(&lease, owner, vec![Mutation::Put(Row::Job(next.clone()))])?;
            if next.state == JobState::Running {
                return Ok(Some(next));
            }
            // It went dead from repeated crashes, so look for the next one.
        }
    }

    /// Commits a job's results and its completion in one transaction, which is what makes a result appear exactly once. False means this worker no longer holds the job, and nothing was written.
    pub fn complete_job(&self, vault: &VaultId, held: &JobRow, results: Vec<Mutation>) -> Result<bool, ServerError> {
        let lease = self.lease(vault)?;
        let vault_lock = self.vault_lock(vault);
        let _one_at_a_time = vault_lock.lock().expect("lock");
        let current = self.job(vault, &held.header.id)?;
        if !still_held(&current, held) {
            return Ok(false);
        }
        let owner = held.lease_owner.clone().unwrap_or_default();
        let mut mutations = results;
        mutations.push(Mutation::Put(Row::Job(jobs::succeeded(current, self.now()))));
        self.state_event(&lease, &owner, mutations)?;
        Ok(true)
    }

    /// Records a failed attempt. False means this worker no longer holds the job.
    pub fn fail_job(&self, vault: &VaultId, held: &JobRow, error: &str, retryable: bool) -> Result<bool, ServerError> {
        let lease = self.lease(vault)?;
        let vault_lock = self.vault_lock(vault);
        let _one_at_a_time = vault_lock.lock().expect("lock");
        let current = self.job(vault, &held.header.id)?;
        if !still_held(&current, held) {
            return Ok(false);
        }
        let owner = held.lease_owner.clone().unwrap_or_default();
        let next = jobs::failed(current, error, retryable, self.now(), self.seed.next_u64());
        self.state_event(&lease, &owner, vec![Mutation::Put(Row::Job(next))])?;
        Ok(true)
    }

    /// What a parser found, turned into rows: the rendition, its segments, and re-anchored evidence from the rendition before it. Blobs are written first, so a crash leaves only orphans. A job commits these with its completion.
    pub fn build_rendition(&self, vault: &VaultId, source_id: &ObjectId, parsed: RenditionInput) -> Result<Vec<Mutation>, ServerError> {
        let snap = self.engine.store().snapshot(vault)?;
        let Some(Row::Source(source)) = snap.get(&RowKey {
            table: Table::Source,
            id: source_id.clone(),
        })?
        else {
            return Err(ServerError::NotFound);
        };
        let (canon, words, parser, doc, pages) = match parsed {
            RenditionInput::Pdf(doc, pages) => (rendition::canonical(&doc.words), doc.words.clone(), doc.parser.clone(), Some(doc), pages),
            RenditionInput::Plain { text, parser } => (rendition::canonical_plain(&text), vec![], parser, None, vec![]),
        };
        let page_count = doc.as_ref().map_or(0, |d| d.pages.len() as u32);
        let table = doc.as_ref().map(rendition::word_table_bytes).unwrap_or_else(|| b"{}".to_vec());
        let text_blob = self.blobs.put(vault, &mut canon.text.as_bytes())?.hash.to_string();
        let words_blob = self.blobs.put(vault, &mut table.as_slice())?.hash.to_string();
        let now = self.now();
        let header = |id: ObjectId, version| Header {
            id,
            creator: "pipeline".into(),
            status: Status::Active,
            version,
            acl_id: None,
        };
        let id = ObjectId(self.new_id());
        let spans = rendition::segments(&canon, &words);
        let mut out = vec![];
        let mut rows: Vec<SegmentRow> = vec![];
        for span in &spans {
            let text = canon.text[span.start..span.end].to_string();
            rows.push(SegmentRow {
                header: header(ObjectId(self.new_id()), 1),
                source_id: source.header.id.clone(),
                language: rendition::detect_language(&text),
                text,
                rendition_id: Some(id.clone()),
                start: span.start as u64,
                end: span.end as u64,
                heading: span.heading.clone(),
                page_from: span.page_from,
                page_to: span.page_to,
            });
        }
        // Supersede the rendition before this one, and move its evidence over by quote. What can't be found is flagged, never dropped.
        let earlier = snap.renditions_of(source_id)?;
        if let Some(previous) = earlier.iter().find(|r| r.superseded_by.is_none()) {
            let mut old = previous.clone();
            old.superseded_by = Some(id.clone());
            old.header.version += 1;
            let (mut old_segments, mut after) = (vec![], None);
            loop {
                let page = snap.segments_of(&previous.header.id, after.as_ref(), crate::PAGE_LIMIT)?;
                let Some(last) = page.last() else { break };
                after = Some(last.header.id.clone());
                old_segments.extend(page);
            }
            let ids: Vec<ObjectId> = old_segments.iter().map(|x| x.header.id.clone()).collect();
            for mut evidence in snap.evidence_citing(&ids)? {
                let (prefix, suffix) = evidence
                    .anchor
                    .as_ref()
                    .map_or((String::new(), String::new()), |a| (a.prefix.clone(), a.suffix.clone()));
                match rendition::locate(&canon, &words, &source.sha256, &id, &evidence.quote, &prefix, &suffix) {
                    Some(anchor) => {
                        if let Some(seg) = rows.iter().find(|r| r.start <= anchor.start && anchor.start < r.end) {
                            evidence.segment_id = seg.header.id.clone();
                        }
                        evidence.anchor = Some(anchor);
                        evidence.anchor_broken = false;
                    }
                    None => evidence.anchor_broken = true,
                }
                evidence.header.version += 1;
                out.push(Mutation::Put(Row::Evidence(evidence)));
            }
            out.push(Mutation::Put(Row::Rendition(old)));
        }
        let segment_count = rows.len() as u32;
        out.extend(rows.into_iter().map(|r| Mutation::Put(Row::Segment(r))));
        if segment_count > 0 {
            out.extend(self.extraction_rows(source_id, &id));
        }
        out.push(Mutation::Put(Row::Rendition(RenditionRow {
            header: header(id, 1),
            source_id: source_id.clone(),
            parser,
            pipeline_version: PIPELINE_VERSION,
            text_blob,
            words_blob,
            page_count,
            segment_count,
            created_at_ms: now,
            superseded_by: None,
            pages,
        })));
        Ok(out)
    }

    /// The source a rendition job works on, and its bytes up to a limit.
    pub fn read_source_bytes(&self, vault: &VaultId, source_id: &ObjectId, limit: u64) -> Result<(SourceRow, Vec<u8>), ServerError> {
        let snap = self.engine.store().snapshot(vault)?;
        let Some(Row::Source(source)) = snap.get(&RowKey {
            table: Table::Source,
            id: source_id.clone(),
        })?
        else {
            return Err(ServerError::NotFound);
        };
        drop(snap);
        let hash = memex_types::BlobHash::parse(&source.sha256).map_err(|e| ServerError::Invalid(e.to_string()))?;
        let mut reader = self.blobs.read(vault, &hash, 0, Some(limit))?;
        let mut bytes = vec![];
        reader.read_to_end(&mut bytes).map_err(|e| ServerError::Unavailable(e.to_string()))?;
        Ok((source, bytes))
    }

    pub fn list_renditions(&self, caller: &Caller, vault: &VaultId, source_id: &ObjectId) -> Result<Vec<RenditionRow>, ServerError> {
        self.get_source(caller, vault, source_id)?;
        Ok(self.engine.store().snapshot(vault)?.renditions_of(source_id)?)
    }

    fn rendition(&self, vault: &VaultId, id: &ObjectId) -> Result<RenditionRow, ServerError> {
        match self.engine.store().snapshot(vault)?.get(&RowKey {
            table: Table::Rendition,
            id: id.clone(),
        })? {
            Some(Row::Rendition(r)) => Ok(r),
            _ => Err(ServerError::NotFound),
        }
    }

    pub fn get_rendition(&self, caller: &Caller, vault: &VaultId, id: &ObjectId) -> Result<RenditionRow, ServerError> {
        self.allow(caller, Operation::VaultContentRead(vault))?;
        self.vault_exists(vault)?;
        self.rendition(vault, id)
    }

    /// A rendition's stored blob, either its canonical text or its word table.
    pub fn rendition_blob(&self, caller: &Caller, vault: &VaultId, id: &ObjectId, words: bool) -> Result<Vec<u8>, ServerError> {
        let r = self.get_rendition(caller, vault, id)?;
        let hash = memex_types::BlobHash::parse(if words { &r.words_blob } else { &r.text_blob }).map_err(|e| ServerError::Invalid(e.to_string()))?;
        let mut out = vec![];
        self.blobs
            .read(vault, &hash, 0, None)?
            .read_to_end(&mut out)
            .map_err(|e| ServerError::Unavailable(e.to_string()))?;
        Ok(out)
    }

    pub fn list_segments(
        &self,
        caller: &Caller,
        vault: &VaultId,
        rendition_id: &ObjectId,
        after: Option<&ObjectId>,
        limit: usize,
    ) -> Result<Vec<SegmentRow>, ServerError> {
        self.get_rendition(caller, vault, rendition_id)?;
        Ok(self.engine.store().snapshot(vault)?.segments_of(rendition_id, after, limit)?)
    }

    pub fn get_segment(&self, caller: &Caller, vault: &VaultId, id: &ObjectId) -> Result<SegmentRow, ServerError> {
        self.allow(caller, Operation::VaultContentRead(vault))?;
        self.vault_exists(vault)?;
        match self.engine.store().snapshot(vault)?.get(&RowKey {
            table: Table::Segment,
            id: id.clone(),
        })? {
            Some(Row::Segment(s)) => Ok(s),
            _ => Err(ServerError::NotFound),
        }
    }

    pub fn list_broken_anchors(
        &self,
        caller: &Caller,
        vault: &VaultId,
        after: Option<&ObjectId>,
        limit: usize,
    ) -> Result<Vec<memex_types::EvidenceRow>, ServerError> {
        self.allow(caller, Operation::VaultContentRead(vault))?;
        self.vault_exists(vault)?;
        Ok(self.engine.store().snapshot(vault)?.broken_anchors(after, limit)?)
    }

    /// Pages flagged in current renditions: low confidence, hidden text and the like. Oldest rendition first.
    pub fn flagged_pages(&self, caller: &Caller, vault: &VaultId) -> Result<Vec<(RenditionRow, memex_types::PageOutcome)>, ServerError> {
        self.allow(caller, Operation::VaultContentRead(vault))?;
        self.vault_exists(vault)?;
        let snap = self.engine.store().snapshot(vault)?;
        let (mut out, mut after) = (vec![], None);
        loop {
            let page = snap.rows(Table::Rendition, after.as_ref())?;
            let Some(last) = page.last() else { return Ok(out) };
            after = Some(last.header().id.clone());
            for row in page {
                if let Row::Rendition(r) = row
                    && r.superseded_by.is_none()
                {
                    for outcome in r.pages.iter().filter(|o| !o.flags.is_empty()) {
                        out.push((r.clone(), outcome.clone()));
                    }
                }
            }
        }
    }

    /// Queues the source to be parsed again. Each request is its own job, as the parser or its settings may have changed.
    pub fn reprocess_source(&self, caller: &Caller, vault: &VaultId, source_id: &ObjectId) -> Result<JobRow, ServerError> {
        self.allow(caller, Operation::VaultContentPropose(vault))?;
        self.vault_exists(vault)?;
        let source = self.get_source(caller, vault, source_id)?;
        let lease = self.lease(vault)?;
        let vault_lock = self.vault_lock(vault);
        let _one_at_a_time = vault_lock.lock().expect("lock");
        let id = ObjectId(self.new_id());
        let dedupe = format!("{}:reprocess:{id}", rendition_key(&source.sha256));
        let job = jobs::new_job(
            id,
            &caller.principal.id,
            RENDITION_JOB,
            json!({"source_id": source_id, "sha256": source.sha256, "media_type": source.media_type}),
            dedupe,
            5,
            self.now(),
        );
        self.state_event(&lease, &caller.principal.id, vec![Mutation::Put(Row::Job(job.clone()))])?;
        Ok(job)
    }

    pub(super) fn canonical_of(&self, vault: &VaultId, r: &RenditionRow) -> Result<(rendition::Canonical, Vec<memex_types::Word>), ServerError> {
        let read = |blob: &str| -> Result<Vec<u8>, ServerError> {
            let hash = memex_types::BlobHash::parse(blob).map_err(|e| ServerError::Invalid(e.to_string()))?;
            let mut out = vec![];
            self.blobs
                .read(vault, &hash, 0, None)?
                .read_to_end(&mut out)
                .map_err(|e| ServerError::Unavailable(e.to_string()))?;
            Ok(out)
        };
        let table = read(&r.words_blob)?;
        let words = serde_json::from_slice::<memex_types::ParsedDocument>(&table)
            .map(|d| d.words)
            .unwrap_or_default();
        if words.is_empty() {
            let text = String::from_utf8_lossy(&read(&r.text_blob)?).into_owned();
            return Ok((rendition::Canonical { text, spans: vec![] }, words));
        }
        Ok((rendition::canonical(&words), words))
    }

    /// Follows a claim to its sources and regions. Each quote is checked against the text now, so a broken anchor is reported, not assumed.
    pub fn lineage(&self, caller: &Caller, vault: &VaultId, assertion_id: &ObjectId) -> Result<Lineage, ServerError> {
        self.allow(caller, Operation::VaultContentRead(vault))?;
        self.vault_exists(vault)?;
        let snap = self.engine.store().snapshot(vault)?;
        let Some(Row::Assertion(assertion)) = snap.get(&RowKey {
            table: Table::Assertion,
            id: assertion_id.clone(),
        })?
        else {
            return Err(ServerError::NotFound);
        };
        let changeset = match snap.get(&RowKey {
            table: Table::ChangeSet,
            id: assertion.changeset_id.clone(),
        })? {
            Some(Row::ChangeSet(c)) => Some(c),
            _ => None,
        };
        let mut evidence = vec![];
        for e in snap.evidence_of(std::slice::from_ref(assertion_id))? {
            let Some(Row::Segment(segment)) = snap.get(&RowKey {
                table: Table::Segment,
                id: e.segment_id.clone(),
            })?
            else {
                continue;
            };
            let Some(Row::Source(source)) = snap.get(&RowKey {
                table: Table::Source,
                id: segment.source_id.clone(),
            })?
            else {
                continue;
            };
            let Some(rendition_id) = segment.rendition_id.clone() else { continue };
            let Some(Row::Rendition(r)) = snap.get(&RowKey {
                table: Table::Rendition,
                id: rendition_id,
            })?
            else {
                continue;
            };
            let (canon, words) = self.canonical_of(vault, &r)?;
            let (prefix, suffix) = e
                .anchor
                .as_ref()
                .map_or((String::new(), String::new()), |a| (a.prefix.clone(), a.suffix.clone()));
            let anchor = rendition::locate(&canon, &words, &source.sha256, &r.header.id, &e.quote, &prefix, &suffix);
            let broken = anchor.is_none() || e.anchor_broken;
            evidence.push(EvidenceLineage {
                superseded: r.superseded_by.is_some(),
                evidence: e,
                segment,
                rendition: r,
                source,
                anchor,
                broken,
            });
        }
        Ok(Lineage {
            assertion,
            changeset,
            evidence,
        })
    }

    fn claims_from(&self, vault: &VaultId, evidence: Vec<memex_types::EvidenceRow>) -> Result<Vec<ClaimRef>, ServerError> {
        let keys: Vec<RowKey> = evidence
            .iter()
            .map(|e| RowKey {
                table: Table::Assertion,
                id: e.assertion_id.clone(),
            })
            .collect();
        let rows = self.engine.store().snapshot(vault)?.get_many(&keys)?;
        Ok(evidence
            .into_iter()
            .zip(rows)
            .filter_map(|(e, r)| match r {
                Some(Row::Assertion(assertion)) => Some(ClaimRef {
                    assertion,
                    evidence_id: e.header.id,
                }),
                _ => None,
            })
            .collect())
    }

    /// The claims citing a segment.
    pub fn claims_citing_segment(&self, caller: &Caller, vault: &VaultId, segment_id: &ObjectId) -> Result<Vec<ClaimRef>, ServerError> {
        self.get_segment(caller, vault, segment_id)?;
        let evidence = self.engine.store().snapshot(vault)?.evidence_citing(std::slice::from_ref(segment_id))?;
        self.claims_from(vault, evidence)
    }

    /// The claims citing any segment of any rendition of a source.
    pub fn claims_citing_source(&self, caller: &Caller, vault: &VaultId, source_id: &ObjectId) -> Result<Vec<ClaimRef>, ServerError> {
        self.get_source(caller, vault, source_id)?;
        let snap = self.engine.store().snapshot(vault)?;
        let mut ids = vec![];
        for r in snap.renditions_of(source_id)? {
            let mut after = None;
            loop {
                let page = snap.segments_of(&r.header.id, after.as_ref(), crate::PAGE_LIMIT)?;
                let Some(last) = page.last() else { break };
                after = Some(last.header.id.clone());
                ids.extend(page.into_iter().map(|s| s.header.id));
            }
        }
        let evidence = snap.evidence_citing(&ids)?;
        drop(snap);
        self.claims_from(vault, evidence)
    }

    fn current_rendition(&self, vault: &VaultId, source_id: &ObjectId) -> Result<RenditionRow, ServerError> {
        let all = self.engine.store().snapshot(vault)?.renditions_of(source_id)?;
        all.into_iter().rfind(|r| r.superseded_by.is_none()).ok_or(ServerError::NotFound)
    }

    /// A source's pages, from its current rendition.
    pub fn list_pages(&self, caller: &Caller, vault: &VaultId, source_id: &ObjectId) -> Result<PageList, ServerError> {
        self.get_source(caller, vault, source_id)?;
        let r = self.current_rendition(vault, source_id)?;
        let table = self.rendition_blob(caller, vault, &r.header.id, true)?;
        let doc: memex_types::ParsedDocument =
            serde_json::from_slice(&table).map_err(|_| ServerError::Invalid("this source has no pages, as it isn't a PDF".into()))?;
        Ok(PageList {
            rendition_id: r.header.id,
            pages: doc.pages,
        })
    }

    /// A page as a PNG, with these rects highlighted. Refuses a resolution or page that isn't there.
    pub fn page_image(
        &self,
        caller: &Caller,
        vault: &VaultId,
        source_id: &ObjectId,
        page: u32,
        dpi: u32,
        highlights: &[Rect],
    ) -> Result<PageImage, ServerError> {
        if !ALLOWED_DPI.contains(&dpi) {
            return Err(ServerError::Invalid(format!("dpi must be one of {ALLOWED_DPI:?}")));
        }
        let pages = self.list_pages(caller, vault, source_id)?;
        if page as usize >= pages.pages.len() {
            return Err(ServerError::Invalid(format!(
                "page {page} doesn't exist: this source has {} pages",
                pages.pages.len()
            )));
        }
        let (_, bytes) = self.read_source_bytes(vault, source_id, u64::MAX)?;
        self.rasterizer
            .render(&bytes, page, dpi, highlights)
            .map_err(|e| ServerError::Invalid(e.to_string()))
    }

    pub fn thumbnail(&self, caller: &Caller, vault: &VaultId, source_id: &ObjectId, page: u32) -> Result<PageImage, ServerError> {
        self.page_image(caller, vault, source_id, page, THUMBNAIL_DPI, &[])
    }

    /// One evidence row's highlight: which source and page, and its rects on that page. Several pages give the first.
    pub fn evidence_region(&self, caller: &Caller, vault: &VaultId, assertion_id: &ObjectId, evidence_id: &ObjectId) -> Result<EvidenceLineage, ServerError> {
        let l = self.lineage(caller, vault, assertion_id)?;
        l.evidence
            .into_iter()
            .find(|e| &e.evidence.header.id == evidence_id)
            .ok_or(ServerError::NotFound)
    }

    /// An original file's bytes, from `offset`, at most `len`, with its total size.
    pub fn read_original(
        &self,
        caller: &Caller,
        vault: &VaultId,
        source_id: &ObjectId,
        offset: u64,
        len: Option<u64>,
    ) -> Result<(SourceRow, Vec<u8>), ServerError> {
        let source = self.get_source(caller, vault, source_id)?;
        let hash = memex_types::BlobHash::parse(&source.sha256).map_err(|e| ServerError::Invalid(e.to_string()))?;
        let mut out = vec![];
        self.blobs
            .read(vault, &hash, offset, len)?
            .read_to_end(&mut out)
            .map_err(|e| ServerError::Unavailable(e.to_string()))?;
        Ok((source, out))
    }

    /// Every vault's id, for the worker pool.
    pub fn vault_ids(&self) -> Result<Vec<VaultId>, ServerError> {
        let (mut out, mut after) = (vec![], None);
        loop {
            let page = self.registry.vaults(None, after.as_ref(), RESULT_LIMIT)?;
            let Some(last) = page.last() else { return Ok(out) };
            after = Some(last.id.clone());
            out.extend(page.into_iter().map(|v| v.id));
        }
    }
}

/// The job kind that turns a Source into a rendition. Spec 015 implements it.
pub const RENDITION_JOB: &str = "rendition";
/// Part of the dedupe key, so a new pipeline can process the same bytes again.
pub const PIPELINE_VERSION: u32 = 1;

fn rendition_key(sha256: &str) -> String {
    format!("{RENDITION_JOB}:{sha256}:{PIPELINE_VERSION}")
}

fn still_held(current: &JobRow, held: &JobRow) -> bool {
    current.state == JobState::Running && current.lease_owner == held.lease_owner && current.attempts.len() == held.attempts.len()
}

/// Counts bytes against the limit and keeps the first few for sniffing, while the blob store reads through it.
struct LimitedSniffer<'a> {
    inner: &'a mut dyn Read,
    limit: u64,
    seen: u64,
    prefix: Vec<u8>,
    over: bool,
}

impl Read for LimitedSniffer<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.seen += n as u64;
        if self.seen > self.limit {
            self.over = true;
            return Err(std::io::Error::other("over the size limit"));
        }
        let room = 4096usize.saturating_sub(self.prefix.len()).min(n);
        self.prefix.extend_from_slice(&buf[..room]);
        Ok(n)
    }
}
