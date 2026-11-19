use crate::migrate::{LATEST_SERVER_VERSION, LATEST_VAULT_VERSION, SERVER_MIGRATIONS, VAULT_MIGRATIONS};
use crate::net::net_mutations;
use crate::tls::Target;
use memex_types::{
    AssertionRow, BlockKind, BlockRow, ChangeSetRow, ChatMessage, ChatReservation, ChatStored, ChatThread, ChatThreadKey, DocKind, EdgeDirection, EdgeRow,
    EntityRow, EntityTypeRow, Event, EvidenceRow, FieldFilter, FieldSort, FilterValue, Header, IdDigest, JobRow, JobState, Mutation, ObjectId, PageRow,
    RenditionRow, RevisionRow, Row, RowKey, SegmentRow, SharedValue, SortAfter, SortColumn, SourceRow, Status, Table, ValueMatch, VaultId,
};
use postgres::{Client, Statement, types::ToSql};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

pub const PAGE_LIMIT: i64 = 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PgError {
    WriterActive,
    LeaseLost,
    UnknownVault,
    EmptyBatch,
    SequenceGap {
        head: Option<u64>,
        got: u64,
    },
    MissingRow {
        key: RowKey,
    },
    /// The server's settings or schema version, or the connection URL's TLS options, mean this binary must not run on it.
    Refused(String),
    Backend(String),
}

impl fmt::Display for PgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for PgError {}

/// The server's own message when there is one, otherwise the error and its cause, which says why a TLS handshake failed. Neither form includes the password from the connection URL.
pub(crate) fn describe(e: &postgres::Error) -> String {
    match (e.as_db_error(), std::error::Error::source(e)) {
        (Some(db), _) => format!("{}: {}", db.code().code(), db.message()),
        (None, Some(cause)) => format!("{e}: {cause}"),
        (None, None) => e.to_string(),
    }
}

fn backend(e: postgres::Error) -> PgError {
    PgError::Backend(describe(&e))
}

/// A connection and its prepared statements. Preparing costs a round trip, so each statement is prepared once per connection.
pub struct Conn {
    pub client: Client,
    statements: HashMap<String, Statement>,
    /// The pool's count this connection is part of, given back when it's dropped. A writer's connection has none.
    slot: Option<Weak<Slots>>,
}

impl Drop for Conn {
    fn drop(&mut self) {
        if let Some(slots) = self.slot.take().and_then(|w| w.upgrade()) {
            slots.release();
        }
    }
}

impl Conn {
    fn new(client: Client) -> Self {
        Self {
            client,
            statements: HashMap::new(),
            slot: None,
        }
    }

    pub fn query(&mut self, sql: &str, params: &[&(dyn ToSql + Sync)]) -> Result<Vec<postgres::Row>, postgres::Error> {
        let st = self.statement(sql)?;
        self.client.query(&st, params)
    }

    pub fn query_one(&mut self, sql: &str, params: &[&(dyn ToSql + Sync)]) -> Result<postgres::Row, postgres::Error> {
        let st = self.statement(sql)?;
        self.client.query_one(&st, params)
    }

    pub fn query_opt(&mut self, sql: &str, params: &[&(dyn ToSql + Sync)]) -> Result<Option<postgres::Row>, postgres::Error> {
        let st = self.statement(sql)?;
        self.client.query_opt(&st, params)
    }

    fn statement(&mut self, sql: &str) -> Result<Statement, postgres::Error> {
        if let Some(st) = self.statements.get(sql) {
            return Ok(st.clone());
        }
        let st = self.client.prepare(sql)?;
        self.statements.insert(sql.to_string(), st.clone());
        Ok(st)
    }
}

/// A vault's writer: its lease's token, its schema, and the shared connection holding its advisory lock.
#[derive(Clone)]
struct Writer {
    token: u64,
    schema: String,
    shared: Arc<SharedWriter>,
}

/// Proof of holding a vault's advisory lock, on the writer's own connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PgLease {
    pub vault: VaultId,
    pub token: u64,
}

/// How many connections a store's pool may open, how many it keeps idle, how many writer connections it may open, and how long a caller waits for either past its limit.
///
/// Writers' lock connections are outside the pool's count, so a burst of readers can't stop a writer opening its vault or committing. They have a limit of their own, `writers`, shared by every vault this store writes to (spec 093), so a store holds at most `max_open + writers` connections however many vaults it has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolLimits {
    pub max_open: usize,
    pub idle: usize,
    pub wait: Duration,
    pub writers: usize,
}

/// Chosen from spec 085's measurements: see its Result.
pub const DEFAULT_MAX_OPEN: usize = 32;
pub const DEFAULT_WAIT: Duration = Duration::from_secs(5);
/// Chosen from spec 093's measurements: see its Result.
pub const DEFAULT_WRITERS: usize = 8;

impl PoolLimits {
    /// The defaults, keeping at most `idle` idle connections.
    pub fn with_idle(idle: usize) -> Self {
        Self {
            max_open: DEFAULT_MAX_OPEN.max(idle).max(1),
            idle,
            wait: DEFAULT_WAIT,
            writers: DEFAULT_WRITERS,
        }
    }
}

/// A writer connection, shared by the vaults whose advisory locks it holds. One vault's work uses it at a time, and the others wait their turn, in the order they came, up to the store's wait.
struct SharedWriter {
    state: Mutex<SharedState>,
    freed: Condvar,
    /// Set once a call on the connection has found it closed, so a lease can be checked without waiting for the connection.
    closed: AtomicBool,
    /// How many vaults hold their lock on this connection.
    vaults: AtomicUsize,
}

struct SharedState {
    /// None while a caller has it.
    conn: Option<Conn>,
    queue: VecDeque<u64>,
    next_ticket: u64,
}

/// A shared writer connection taken by one caller, given back when dropped.
struct Borrowed<'a> {
    shared: &'a SharedWriter,
    conn: Option<Conn>,
}

impl std::ops::Deref for Borrowed<'_> {
    type Target = Conn;
    fn deref(&self) -> &Conn {
        self.conn.as_ref().expect("borrowed")
    }
}

impl std::ops::DerefMut for Borrowed<'_> {
    fn deref_mut(&mut self) -> &mut Conn {
        self.conn.as_mut().expect("borrowed")
    }
}

impl Drop for Borrowed<'_> {
    fn drop(&mut self) {
        let Some(conn) = self.conn.take() else { return };
        if conn.client.is_closed() {
            self.shared.closed.store(true, Ordering::Relaxed);
        }
        self.shared.state.lock().expect("lock").conn = Some(conn);
        self.shared.freed.notify_all();
    }
}

impl SharedWriter {
    fn new(client: Client) -> Self {
        Self {
            state: Mutex::new(SharedState {
                conn: Some(Conn::new(client)),
                queue: VecDeque::new(),
                next_ticket: 0,
            }),
            freed: Condvar::new(),
            closed: AtomicBool::new(false),
            vaults: AtomicUsize::new(0),
        }
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    /// The connection once it's free and every earlier waiter has had it, or an error saying it's busy after `wait`.
    // Edge: the wait is measured on the real clock, as it's a time a person waits.
    #[allow(clippy::disallowed_methods)]
    fn borrow(&self, wait: Duration) -> Result<Borrowed<'_>, PgError> {
        let deadline = Instant::now() + wait;
        let mut st = self.state.lock().expect("lock");
        let mut ticket = None;
        loop {
            let my_turn = match st.queue.front() {
                None => true,
                Some(front) => ticket == Some(*front),
            };
            if my_turn && let Some(conn) = st.conn.take() {
                if let Some(t) = ticket {
                    st.queue.retain(|q| *q != t);
                }
                let others = !st.queue.is_empty();
                drop(st);
                if others {
                    self.freed.notify_all();
                }
                return Ok(Borrowed {
                    shared: self,
                    conn: Some(conn),
                });
            }
            if ticket.is_none() {
                let t = st.next_ticket;
                st.next_ticket += 1;
                st.queue.push_back(t);
                ticket = Some(t);
            }
            let now = Instant::now();
            if now >= deadline {
                if let Some(t) = ticket {
                    st.queue.retain(|q| *q != t);
                }
                drop(st);
                self.freed.notify_all();
                return Err(PgError::Backend(format!(
                    "the vault's writer connection is busy with other vaults' writes, and didn't come free within {} ms",
                    wait.as_millis()
                )));
            }
            st = self.freed.wait_timeout(st, deadline - now).expect("lock").0;
        }
    }
}

/// Pooled connections: the idle ones, and the count of every one open, idle or taken.
struct Slots {
    state: Mutex<SlotState>,
    freed: Condvar,
}

struct SlotState {
    idle: Vec<Conn>,
    open: usize,
    /// Waiters' tickets, oldest first. Only the oldest may take a connection that comes free, so a busy caller giving one back and taking it again can't starve a waiter.
    queue: VecDeque<u64>,
    next_ticket: u64,
}

impl Slots {
    /// A pooled connection was dropped, so one more may open.
    fn release(&self) {
        let mut st = self.state.lock().expect("lock");
        st.open = st.open.saturating_sub(1);
        drop(st);
        self.freed.notify_all();
    }
}

/// Takes a connection out of the count without calling back into it, for one dropped while the count's lock is held.
fn detach(mut conn: Conn) -> Conn {
    conn.slot = None;
    conn
}

/// How many pooled connections are open, and how many of those are idle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolStats {
    pub open: usize,
    pub idle: usize,
}

pub struct PgStore {
    target: Target,
    slots: Arc<Slots>,
    limits: PoolLimits,
    writers: Mutex<HashMap<VaultId, Writer>>,
    /// The open writer connections, at most `limits.writers`.
    shared: Mutex<Vec<Arc<SharedWriter>>>,
    schemas: Mutex<HashMap<VaultId, String>>,
    next_token: AtomicU64,
}

impl fmt::Debug for PgStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PgStore { url: <redacted> }")
    }
}

pub fn schema_of_vault(vault: &VaultId) -> String {
    let clean: String = vault
        .0
        .chars()
        .take(40)
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect();
    let digest = Sha256::digest(vault.0.as_bytes());
    format!("vault_{clean}_{:02x}{:02x}{:02x}{:02x}", digest[0], digest[1], digest[2], digest[3])
}

fn lock_key(vault: &VaultId) -> i64 {
    let d = Sha256::digest(format!("memexvault-writer:{}", vault.0).as_bytes());
    i64::from_be_bytes([d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]])
}

impl PgStore {
    /// The default limits, keeping at most `pool_max` idle connections.
    pub fn connect(url: &str, pool_max: usize) -> Result<Self, PgError> {
        Self::connect_with(url, PoolLimits::with_idle(pool_max))
    }

    pub fn connect_with(url: &str, limits: PoolLimits) -> Result<Self, PgError> {
        if limits.max_open == 0 {
            return Err(PgError::Refused("the connection limit must be at least 1".into()));
        }
        if limits.writers == 0 {
            return Err(PgError::Refused("the writer connection limit must be at least 1".into()));
        }
        let store = Self {
            target: Target::parse(url)?,
            slots: Arc::new(Slots {
                state: Mutex::new(SlotState {
                    idle: vec![],
                    open: 0,
                    queue: VecDeque::new(),
                    next_ticket: 0,
                }),
                freed: Condvar::new(),
            }),
            limits,
            writers: Mutex::new(HashMap::new()),
            shared: Mutex::new(vec![]),
            schemas: Mutex::new(HashMap::new()),
            next_token: AtomicU64::new(1),
        };
        // Proves the server is reachable, and keeps the connection for the first caller.
        let first = store.take_conn()?;
        store.give_conn(first);
        Ok(store)
    }

    pub fn limits(&self) -> PoolLimits {
        self.limits
    }

    pub fn pool_stats(&self) -> PoolStats {
        let st = self.slots.state.lock().expect("lock");
        PoolStats {
            open: st.open,
            idle: st.idle.len(),
        }
    }

    fn new_client(&self) -> Result<Client, PgError> {
        self.target.connect().map_err(backend)
    }

    /// An idle connection, or a new one while fewer than the limit are open. Past the limit it waits for one to be given back or dropped, up to the set time, then fails with a backend error that says the pool is full. A closed connection stops counting once it's dropped.
    // Edge: the wait is measured on the real clock, as it's a time a person waits.
    #[allow(clippy::disallowed_methods)]
    pub fn take_conn(&self) -> Result<Conn, PgError> {
        let deadline = Instant::now() + self.limits.wait;
        let mut closed = vec![];
        let mut ticket = None;
        let mut st = self.slots.state.lock().expect("lock");
        loop {
            let my_turn = match st.queue.front() {
                None => true,
                Some(front) => ticket == Some(*front),
            };
            if my_turn {
                while let Some(c) = st.idle.pop() {
                    if c.client.is_closed() {
                        st.open = st.open.saturating_sub(1);
                        closed.push(detach(c));
                    } else {
                        self.leave_queue(st, ticket);
                        return Ok(c);
                    }
                }
                if st.open < self.limits.max_open {
                    st.open += 1;
                    self.leave_queue(st, ticket);
                    drop(closed);
                    return match self.new_client() {
                        Ok(client) => {
                            let mut conn = Conn::new(client);
                            conn.slot = Some(Arc::downgrade(&self.slots));
                            Ok(conn)
                        }
                        Err(e) => {
                            self.slots.release();
                            Err(e)
                        }
                    };
                }
            }
            if ticket.is_none() {
                let t = st.next_ticket;
                st.next_ticket += 1;
                st.queue.push_back(t);
                ticket = Some(t);
            }
            let now = Instant::now();
            if now >= deadline {
                self.leave_queue(st, ticket);
                return Err(PgError::Backend(format!(
                    "the connection pool is full: all {} connections are in use, and none came free within {} ms",
                    self.limits.max_open,
                    self.limits.wait.as_millis()
                )));
            }
            st = self.slots.freed.wait_timeout(st, deadline - now).expect("lock").0;
        }
    }

    /// Takes a waiter's ticket out of the queue, and wakes the others, as the next one may now take what's free.
    fn leave_queue(&self, mut st: std::sync::MutexGuard<'_, SlotState>, ticket: Option<u64>) {
        let Some(t) = ticket else { return };
        st.queue.retain(|q| *q != t);
        let others = !st.queue.is_empty();
        drop(st);
        if others {
            self.slots.freed.notify_all();
        }
    }

    /// Returns a connection to the pool. It must have no open transaction.
    pub fn give_conn(&self, conn: Conn) {
        self.give_client(conn);
    }

    pub(crate) fn take_client(&self) -> Result<Conn, PgError> {
        self.take_conn()
    }

    /// Keeps the connection idle and wakes a waiter, or drops it, which also wakes one.
    pub(crate) fn give_client(&self, conn: Conn) {
        let mut st = self.slots.state.lock().expect("lock");
        if st.idle.len() < self.limits.idle && !conn.client.is_closed() && conn.slot.is_some() {
            st.idle.push(conn);
            drop(st);
            self.slots.freed.notify_all();
        } else {
            drop(st);
            drop(conn);
        }
    }

    /// The explicit `migrate` command: brings the server schema and every known vault up to date.
    pub fn migrate(&self) -> Result<(), PgError> {
        let mut conn = self.take_client()?;
        let c = &mut conn.client;
        c.batch_execute("SET client_min_messages = warning; CREATE SCHEMA IF NOT EXISTS memex_server; CREATE TABLE IF NOT EXISTS memex_server.schema_version (version integer NOT NULL);")
            .map_err(backend)?;
        let current: i32 = c
            .query_one("SELECT coalesce(max(version), 0) FROM memex_server.schema_version", &[])
            .map_err(backend)?
            .get(0);
        if current > LATEST_SERVER_VERSION {
            return Err(PgError::Refused(format!(
                "server schema is at version {current}, newer than this binary's {LATEST_SERVER_VERSION}"
            )));
        }
        for (i, sql) in SERVER_MIGRATIONS.iter().enumerate().skip(current as usize) {
            let mut t = c.transaction().map_err(backend)?;
            t.batch_execute(sql).map_err(backend)?;
            t.execute("INSERT INTO memex_server.schema_version VALUES ($1)", &[&(i as i32 + 1)])
                .map_err(backend)?;
            t.commit().map_err(backend)?;
        }
        let vaults = c.query("SELECT schema_name FROM memex_server.vaults", &[]).map_err(backend)?;
        for row in vaults {
            migrate_vault(c, row.get(0))?;
        }
        self.give_client(conn);
        Ok(())
    }

    /// Startup checks: durability on, and a schema this binary knows. Refuses to run otherwise.
    pub fn check_server(&self) -> Result<(), PgError> {
        let mut conn = self.take_client()?;
        let c = &mut conn.client;
        for setting in ["fsync", "synchronous_commit"] {
            let v: String = c.query_one(&format!("SHOW {setting}"), &[]).map_err(backend)?.get(0);
            if v != "on" {
                return Err(PgError::Refused(format!(
                    "{setting} is {v}, and must be on: acknowledged commits could be lost"
                )));
            }
        }
        let exists: bool = c
            .query_one("SELECT to_regclass('memex_server.schema_version') IS NOT NULL", &[])
            .map_err(backend)?
            .get(0);
        if !exists {
            return Err(PgError::Refused("no memexvault schema here, so run the migrate command first".into()));
        }
        let version: i32 = c
            .query_one("SELECT coalesce(max(version), 0) FROM memex_server.schema_version", &[])
            .map_err(backend)?
            .get(0);
        if version != LATEST_SERVER_VERSION {
            return Err(PgError::Refused(format!(
                "server schema is at version {version}, and this binary needs {LATEST_SERVER_VERSION}, so run migrate or upgrade"
            )));
        }
        self.give_client(conn);
        Ok(())
    }

    fn schema_of(&self, vault: &VaultId, c: &mut Client) -> Result<String, PgError> {
        if let Some(s) = self.schemas.lock().expect("lock").get(vault) {
            return Ok(s.clone());
        }
        let row = c
            .query_opt("SELECT schema_name FROM memex_server.vaults WHERE vault_id = $1", &[&vault.0])
            .map_err(backend)?;
        let schema: String = row.ok_or(PgError::UnknownVault)?.get(0);
        self.schemas.lock().expect("lock").insert(vault.clone(), schema.clone());
        Ok(schema)
    }

    /// Takes the session advisory lock on one of the store's writer connections, creating the vault if it's new. A connection holds the locks of several vaults when there are more vaults than `limits.writers`, and losing it loses all of their leases. Refused while another writer holds the vault, this store included.
    pub fn open_vault(&self, vault: &VaultId) -> Result<PgLease, PgError> {
        {
            let mut writers = self.writers.lock().expect("lock");
            match writers.get(vault) {
                Some(w) if !w.shared.is_closed() => return Err(PgError::WriterActive),
                Some(_) => {
                    writers.remove(vault);
                }
                None => {}
            }
        }
        let shared = self.writer_connection()?;
        match self.lock_on(&shared, vault) {
            Ok(schema) => {
                let token = self.next_token.fetch_add(1, Ordering::Relaxed);
                self.writers.lock().expect("lock").insert(vault.clone(), Writer { token, schema, shared });
                Ok(PgLease { vault: vault.clone(), token })
            }
            Err(e) => {
                self.unreserve(&shared);
                Err(e)
            }
        }
    }

    /// Takes the vault's lock on the shared connection, and creates or migrates its schema. Returns the schema.
    fn lock_on(&self, shared: &SharedWriter, vault: &VaultId) -> Result<String, PgError> {
        let mut conn = shared.borrow(self.limits.wait)?;
        let key = lock_key(vault);
        let got: bool = conn.client.query_one("SELECT pg_try_advisory_lock($1)", &[&key]).map_err(backend)?.get(0);
        if !got {
            return Err(PgError::WriterActive);
        }
        let schema = schema_of_vault(vault);
        let created = conn
            .client
            .execute(
                "INSERT INTO memex_server.vaults (vault_id, schema_name) VALUES ($1, $2) ON CONFLICT (vault_id) DO NOTHING",
                &[&vault.0, &schema],
            )
            .map_err(backend)
            .and_then(|_| migrate_vault(&mut conn.client, schema.clone()));
        if let Err(e) = created {
            // The connection lives on for other vaults, so the lock has to be let go by hand.
            let _ = conn.client.execute("SELECT pg_advisory_unlock($1)", &[&key]);
            return Err(e);
        }
        Ok(schema)
    }

    /// A writer connection for a vault to take its lock on, with the vault counted on it: a new one while fewer than the limit are open, else the one holding the fewest vaults. Closed ones are let go first. Counting under the list's lock means a connection can't be let go between being chosen and being counted.
    fn writer_connection(&self) -> Result<Arc<SharedWriter>, PgError> {
        let mut shared = self.shared.lock().expect("lock");
        shared.retain(|w| !w.is_closed());
        let w = if shared.len() < self.limits.writers {
            let w = Arc::new(SharedWriter::new(self.new_client()?));
            shared.push(w.clone());
            w
        } else {
            shared
                .iter()
                .min_by_key(|w| w.vaults.load(Ordering::Relaxed))
                .cloned()
                .expect("at least one writer")
        };
        w.vaults.fetch_add(1, Ordering::Relaxed);
        Ok(w)
    }

    /// One vault fewer on the connection, which is let go when none is left.
    fn unreserve(&self, w: &Arc<SharedWriter>) {
        let mut shared = self.shared.lock().expect("lock");
        if w.vaults.fetch_sub(1, Ordering::Relaxed) == 1 {
            shared.retain(|s| !Arc::ptr_eq(s, w));
        }
    }

    /// How many writer connections are open, and how many vaults hold a lease on them.
    pub fn writer_stats(&self) -> (usize, usize) {
        let mut shared = self.shared.lock().expect("lock");
        shared.retain(|w| !w.is_closed());
        (
            shared.len(),
            self.writers.lock().expect("lock").values().filter(|w| !w.shared.is_closed()).count(),
        )
    }

    fn writer(&self, lease: &PgLease) -> Result<Writer, PgError> {
        let writer = self.writers.lock().expect("lock").get(&lease.vault).cloned().ok_or(PgError::LeaseLost)?;
        if writer.token != lease.token || writer.shared.is_closed() {
            return Err(PgError::LeaseLost);
        }
        Ok(writer)
    }

    /// Whether the lease is still this vault's writer and its connection is open, with no round trip. A connection that died shows as closed once a call on it has failed.
    pub fn lease_held(&self, lease: &PgLease) -> bool {
        self.writer(lease).is_ok()
    }

    /// Lets go of the vault's advisory lock, and of its writer connection when no other vault holds a lock on it.
    pub fn close_vault(&self, lease: &PgLease) -> Result<(), PgError> {
        let writer = self.writer(lease)?;
        self.release(lease, &writer)
    }

    /// Unlocks the vault, waiting for the shared connection as a write would, and then forgets its writer. Fails with the busy error and keeps the lease if the connection doesn't come free. A connection that has died took the lock with it, so a failed unlock on it still counts as let go.
    fn release(&self, lease: &PgLease, writer: &Writer) -> Result<(), PgError> {
        {
            let mut conn = writer.shared.borrow(self.limits.wait)?;
            let unlocked = conn.client.execute("SELECT pg_advisory_unlock($1)", &[&lock_key(&lease.vault)]);
            if let Err(e) = unlocked
                && !conn.client.is_closed()
            {
                return Err(backend(e));
            }
        }
        self.writers.lock().expect("lock").remove(&lease.vault);
        self.unreserve(&writer.shared);
        Ok(())
    }

    pub fn snapshot(&self, vault: &VaultId) -> Result<PgSnapshot<'_>, PgError> {
        let mut conn = self.take_conn()?;
        let schema = self.schema_of(vault, &mut conn.client)?;
        // The first statement fixes a repeatable-read snapshot.
        conn.client
            .batch_execute("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY; SELECT 1;")
            .map_err(backend)?;
        Ok(PgSnapshot {
            store: self,
            vault: vault.clone(),
            schema,
            conn: RefCell::new(Some(conn)),
        })
    }

    /// The log's head and the cache generation in one statement, with no transaction to open or close. A cached search makes this call first, so it has to be cheap.
    pub fn cache_marks(&self, vault: &VaultId) -> Result<(Option<u64>, u64), PgError> {
        let mut conn = self.take_conn()?;
        let schema = self.schema_of(vault, &mut conn.client)?;
        let sql = format!("SELECT (SELECT max(seq) FROM {schema}.events), (SELECT generation FROM {schema}.cache_generation)");
        let st = conn.statement(&sql).map_err(backend)?;
        let row = conn.client.query_one(&st, &[]).map_err(backend)?;
        let head: Option<i64> = row.get(0);
        let generation: i64 = row.get(1);
        self.give_conn(conn);
        Ok((head.map(|h| h as u64), generation as u64))
    }

    /// Takes the vault's writer lease as its proof, like a commit.
    pub fn bump_cache_generation(&self, lease: &PgLease) -> Result<u64, PgError> {
        let writer = self.writer(lease)?;
        let mut conn = writer.shared.borrow(self.limits.wait)?;
        let schema = &writer.schema;
        let sql = format!("UPDATE {schema}.cache_generation SET generation = generation + 1 RETURNING generation");
        let generation = conn.client.query_one(&sql, &[]).map_err(backend);
        if conn.client.is_closed() {
            return Err(PgError::LeaseLost);
        }
        Ok(generation?.get::<_, i64>(0) as u64)
    }

    /// One SQL transaction for the whole batch. `batch` holds each event with its mutations. Durable when it returns.
    pub fn commit_batch(&self, lease: &PgLease, batch: &[(&Event, &[Mutation])]) -> Result<u64, PgError> {
        self.commit_rewriting(lease, &[], batch)
    }

    /// `commit_batch` that also replaces these events' payloads, in the same transaction. For erasure.
    pub fn commit_rewriting(&self, lease: &PgLease, rewrites: &[(u64, serde_json::Value)], batch: &[(&Event, &[Mutation])]) -> Result<u64, PgError> {
        let last = batch.last().ok_or(PgError::EmptyBatch)?.0.seq;
        let writer = self.writer(lease)?;
        let mut conn = writer.shared.borrow(self.limits.wait)?;
        let result = commit_in_transaction(&mut conn, &writer.schema, rewrites, batch);
        if conn.client.is_closed() {
            return Err(PgError::LeaseLost);
        }
        result.map(|()| last)
    }

    /// Drops the vault's schema and ends its lease.
    pub fn delete_vault(&self, lease: &PgLease) -> Result<(), PgError> {
        let writer = self.writer(lease)?;
        self.drop_vault(&lease.vault)?;
        self.release(lease, &writer)
    }

    /// Physically reclaims deleted rows, for erasure. Plain `VACUUM` frees a dead row's space but leaves its bytes in the page until something overwrites them, and a rewritten event leaves its old payload the same way, so every table of the vault is rewritten: its rows, its TOAST data and its indexes, the vector generations and stored embeddings included. Each table takes an exclusive lock while it's rewritten, which `compact_report` measures. Takes the lease as proof, and runs on a pooled connection, as it changes no row and can take a while, which would hold up every vault sharing the writer connection. Postgres's WAL and backups keep what they kept.
    pub fn compact(&self, lease: &PgLease) -> Result<(), PgError> {
        self.compact_report(lease).map(|_| ())
    }

    /// `compact`, with how long each table's rewrite took, which is how long its exclusive lock was held. An edge, timing its own SQL.
    #[allow(clippy::disallowed_methods)]
    pub fn compact_report(&self, lease: &PgLease) -> Result<Vec<(String, std::time::Duration)>, PgError> {
        let writer = self.writer(lease)?;
        let schema = &writer.schema;
        let mut conn = self.take_conn()?;
        let client = &mut conn.client;
        let tables: Vec<String> = client
            .query("SELECT tablename FROM pg_tables WHERE schemaname = $1 ORDER BY tablename", &[&schema.as_str()])
            .map_err(backend)?
            .iter()
            .map(|r| r.get(0))
            .collect();
        let mut out = vec![];
        for t in tables {
            let started = std::time::Instant::now();
            client.batch_execute(&format!("VACUUM FULL {schema}.\"{t}\"")).map_err(backend)?;
            // Fresh statistics, so no sample of a removed value stays in the catalog.
            client.batch_execute(&format!("ANALYZE {schema}.\"{t}\"")).map_err(backend)?;
            out.push((t, started.elapsed()));
        }
        self.give_conn(conn);
        Ok(out)
    }

    /// Runs one short piece of chat work on a pooled connection, and gives it straight back.
    fn with_chat<T>(&self, vault: &VaultId, f: impl FnOnce(&mut Conn, &str) -> Result<T, postgres::Error>) -> Result<T, PgError> {
        let mut conn = self.take_conn()?;
        let schema = self.schema_of(vault, &mut conn.client)?;
        let out = f(&mut conn, &schema).map_err(backend)?;
        self.give_conn(conn);
        Ok(out)
    }

    /// Whether it was new.
    pub fn chat_create_thread(&self, vault: &VaultId, t: &ChatThread) -> Result<bool, PgError> {
        self.with_chat(vault, |c, s| {
            let sql = format!(
                "INSERT INTO {s}.chat_threads (owner_id, id, title, created_at_ms, updated_at_ms, tokens) VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING"
            );
            let st = c.statement(&sql)?;
            Ok(c.client.execute(&st, &[&t.owner, &t.id, &t.title, &t.created_at_ms, &t.updated_at_ms, &(t.tokens as i64)])? == 1)
        })
    }

    pub fn chat_thread(&self, vault: &VaultId, key: &ChatThreadKey) -> Result<Option<ChatThread>, PgError> {
        self.with_chat(vault, |c, s| {
            let sql = format!("SELECT owner_id, id, title, created_at_ms, updated_at_ms, tokens FROM {s}.chat_threads WHERE owner_id = $1 AND id = $2");
            Ok(c.query_opt(&sql, &[&key.owner, &key.id])?.as_ref().map(chat_thread_of))
        })
    }

    pub fn chat_threads(&self, vault: &VaultId, owner: &str, after: Option<(i64, &str)>, limit: usize) -> Result<Vec<ChatThread>, PgError> {
        self.with_chat(vault, |c, s| {
            let sql = format!(
                "SELECT owner_id, id, title, created_at_ms, updated_at_ms, tokens FROM {s}.chat_threads WHERE owner_id = $1 AND ($2::bigint IS NULL OR (updated_at_ms, id) < ($2, $3)) ORDER BY updated_at_ms DESC, id DESC LIMIT $4"
            );
            let (ms, id) = (after.map(|a| a.0), after.map(|a| a.1));
            Ok(c.query(&sql, &[&owner, &ms, &id, &(limit as i64)])?.iter().map(chat_thread_of).collect())
        })
    }

    pub fn chat_rename_thread(&self, vault: &VaultId, key: &ChatThreadKey, title: &str, now_ms: i64) -> Result<bool, PgError> {
        self.with_chat(vault, |c, s| {
            let sql = format!("UPDATE {s}.chat_threads SET title = $3, updated_at_ms = $4 WHERE owner_id = $1 AND id = $2");
            let st = c.statement(&sql)?;
            Ok(c.client.execute(&st, &[&key.owner, &key.id, &title, &now_ms])? == 1)
        })
    }

    pub fn chat_delete_thread(&self, vault: &VaultId, key: &ChatThreadKey) -> Result<bool, PgError> {
        self.with_chat(vault, |c, s| {
            let sql = format!("DELETE FROM {s}.chat_threads WHERE owner_id = $1 AND id = $2");
            let st = c.statement(&sql)?;
            Ok(c.client.execute(&st, &[&key.owner, &key.id])? == 1)
        })
    }

    /// One transaction. Locking the thread's row orders writers to it, and stops a delete passing between the check and the insert. False if it has gone.
    pub fn chat_append_messages(&self, vault: &VaultId, key: &ChatThreadKey, messages: &[ChatMessage], now_ms: i64) -> Result<bool, PgError> {
        let values: Vec<Value> = messages
            .iter()
            .map(|m| serde_json::to_value(m).map_err(|e| PgError::Backend(e.to_string())))
            .collect::<Result<_, _>>()?;
        self.with_chat(vault, |c, s| {
            let mut t = c.client.transaction()?;
            let touched = t.execute(
                &format!("UPDATE {s}.chat_threads SET updated_at_ms = $3 WHERE owner_id = $1 AND id = $2"),
                &[&key.owner, &key.id, &now_ms],
            )?;
            if touched != 1 {
                t.rollback()?;
                return Ok(false);
            }
            let next: i64 = t
                .query_one(
                    &format!("SELECT coalesce(max(seq) + 1, 0) FROM {s}.chat_messages WHERE owner_id = $1 AND thread_id = $2"),
                    &[&key.owner, &key.id],
                )?
                .get(0);
            t.execute(
                &format!(
                    "INSERT INTO {s}.chat_messages (owner_id, thread_id, seq, message) SELECT $1, $2, $3 + (o - 1), m FROM unnest($4::jsonb[]) WITH ORDINALITY AS w(m, o)"
                ),
                &[&key.owner, &key.id, &next, &values],
            )?;
            t.commit()?;
            Ok(true)
        })
    }

    pub fn chat_messages(&self, vault: &VaultId, key: &ChatThreadKey, after: Option<u64>, limit: usize) -> Result<Vec<ChatStored>, PgError> {
        self.with_chat(vault, |c, s| {
            let sql = format!(
                "SELECT seq, message FROM {s}.chat_messages WHERE owner_id = $1 AND thread_id = $2 AND ($3::bigint IS NULL OR seq > $3) ORDER BY seq LIMIT $4"
            );
            let after = after.map(|a| a as i64);
            let rows = c.query(&sql, &[&key.owner, &key.id, &after, &(limit as i64)])?;
            Ok(rows
                .iter()
                .map(|r| ChatStored {
                    seq: r.get::<_, i64>(0) as u64,
                    message: serde_json::from_value(r.get(1)).expect("a message this binary wrote"),
                })
                .collect())
        })
    }

    /// False if the thread has gone. The foreign key is what says so, so it can't pass a delete.
    pub fn chat_record_sources(&self, vault: &VaultId, key: &ChatThreadKey, sources: &[ObjectId]) -> Result<bool, PgError> {
        let ids: Vec<&str> = sources.iter().map(|i| i.0.as_str()).collect();
        let found = self.with_chat(vault, |c, s| {
            let sql = format!(
                "INSERT INTO {s}.chat_thread_sources (owner_id, thread_id, source_id) SELECT $1, $2, x FROM unnest($3::text[]) AS w(x) ON CONFLICT DO NOTHING"
            );
            let st = c.statement(&sql)?;
            c.client.execute(&st, &[&key.owner, &key.id, &ids])
        });
        match found {
            Ok(_) => Ok(true),
            // 23503 is a foreign key violation: there's no such thread any more.
            Err(PgError::Backend(m)) if m.starts_with("23503") => Ok(false),
            Err(e) => Err(e),
        }
    }

    pub fn chat_sources(&self, vault: &VaultId, key: &ChatThreadKey) -> Result<Vec<ObjectId>, PgError> {
        self.with_chat(vault, |c, s| {
            let sql = format!("SELECT source_id FROM {s}.chat_thread_sources WHERE owner_id = $1 AND thread_id = $2 ORDER BY source_id");
            Ok(c.query(&sql, &[&key.owner, &key.id])?.iter().map(|r| ObjectId(r.get(0))).collect())
        })
    }

    pub fn chat_threads_citing(&self, vault: &VaultId, source: &ObjectId, limit: usize) -> Result<Vec<ChatThreadKey>, PgError> {
        self.with_chat(vault, |c, s| {
            let sql = format!("SELECT owner_id, thread_id FROM {s}.chat_thread_sources WHERE source_id = $1 ORDER BY owner_id, thread_id LIMIT $2");
            Ok(c.query(&sql, &[&source.0, &(limit as i64)])?
                .iter()
                .map(|r| ChatThreadKey { owner: r.get(0), id: r.get(1) })
                .collect())
        })
    }

    /// One transaction, so it charges both or neither. Conditional updates can't both take the last of a budget.
    pub fn chat_reserve(
        &self,
        vault: &VaultId,
        key: &ChatThreadKey,
        day: i64,
        tokens: u64,
        thread_limit: u64,
        day_limit: u64,
    ) -> Result<ChatReservation, PgError> {
        let (n, tl, dl) = (tokens as i64, thread_limit as i64, day_limit as i64);
        self.with_chat(vault, |c, s| {
            let mut t = c.client.transaction()?;
            let exists = t
                .query_opt(
                    &format!("SELECT tokens FROM {s}.chat_threads WHERE owner_id = $1 AND id = $2 FOR UPDATE"),
                    &[&key.owner, &key.id],
                )?
                .map(|r| r.get::<_, i64>(0));
            let Some(spent) = exists else {
                t.rollback()?;
                return Ok(ChatReservation::ThreadGone);
            };
            if spent + n > tl {
                t.rollback()?;
                return Ok(ChatReservation::ThreadOver);
            }
            let charged = t.execute(
                &format!(
                    "INSERT INTO {s}.chat_spend (day, tokens) VALUES ($1, $2) ON CONFLICT (day) DO UPDATE SET tokens = {s}.chat_spend.tokens + $2 WHERE {s}.chat_spend.tokens + $2 <= $3"
                ),
                &[&day, &n, &dl],
            )?;
            if charged != 1 || n > dl {
                t.rollback()?;
                return Ok(ChatReservation::DayOver);
            }
            t.execute(
                &format!("UPDATE {s}.chat_threads SET tokens = tokens + $3 WHERE owner_id = $1 AND id = $2"),
                &[&key.owner, &key.id, &n],
            )?;
            t.commit()?;
            Ok(ChatReservation::Reserved)
        })
    }

    pub fn chat_settle(&self, vault: &VaultId, key: &ChatThreadKey, day: i64, delta: i64) -> Result<(), PgError> {
        self.with_chat(vault, |c, s| {
            let mut t = c.client.transaction()?;
            t.execute(
                &format!("UPDATE {s}.chat_threads SET tokens = greatest(tokens + $3, 0) WHERE owner_id = $1 AND id = $2"),
                &[&key.owner, &key.id, &delta],
            )?;
            t.execute(
                &format!("UPDATE {s}.chat_spend SET tokens = greatest(tokens + $2, 0) WHERE day = $1"),
                &[&day, &delta],
            )?;
            t.commit()
        })
    }

    pub fn chat_day_tokens(&self, vault: &VaultId, day: i64) -> Result<u64, PgError> {
        self.with_chat(vault, |c, s| {
            let sql = format!("SELECT tokens FROM {s}.chat_spend WHERE day = $1");
            Ok(c.query_opt(&sql, &[&day])?.map_or(0, |r| r.get::<_, i64>(0) as u64))
        })
    }

    /// Removes a vault's schema entirely. For tests and vault deletion.
    pub fn drop_vault(&self, vault: &VaultId) -> Result<(), PgError> {
        let mut conn = self.take_client()?;
        let c = &mut conn.client;
        let schema = schema_of_vault(vault);
        // A reader holding a lock on one of its tables would otherwise hold the drop, and everything queued behind it, for as long as it likes (spec 018).
        let dropped = c.batch_execute(&format!(
            "SET lock_timeout = '{DROP_LOCK_TIMEOUT_MS}ms'; DROP SCHEMA IF EXISTS {schema} CASCADE; RESET lock_timeout"
        ));
        if let Err(e) = dropped {
            let _ = c.batch_execute("RESET lock_timeout");
            return Err(backend(e));
        }
        c.execute("DELETE FROM memex_server.vaults WHERE vault_id = $1", &[&vault.0]).map_err(backend)?;
        self.schemas.lock().expect("lock").remove(vault);
        self.give_client(conn);
        Ok(())
    }
}

/// How long dropping a vault waits for a lock before it gives up and can be tried again.
pub const DROP_LOCK_TIMEOUT_MS: u64 = 10_000;

fn migrate_vault(c: &mut Client, schema: String) -> Result<(), PgError> {
    c.batch_execute(&format!(
        "SET client_min_messages = warning; CREATE SCHEMA IF NOT EXISTS {schema}; CREATE TABLE IF NOT EXISTS {schema}.schema_version (version integer NOT NULL);"
    ))
    .map_err(backend)?;
    let current: i32 = c
        .query_one(&format!("SELECT coalesce(max(version), 0) FROM {schema}.schema_version"), &[])
        .map_err(backend)?
        .get(0);
    if current > LATEST_VAULT_VERSION {
        return Err(PgError::Refused(format!(
            "vault schema {schema} is at version {current}, newer than this binary's {LATEST_VAULT_VERSION}"
        )));
    }
    for (i, sql) in VAULT_MIGRATIONS.iter().enumerate().skip(current as usize) {
        let mut t = c.transaction().map_err(backend)?;
        t.batch_execute(&sql.replace("{s}", &schema)).map_err(backend)?;
        t.execute(&format!("INSERT INTO {schema}.schema_version VALUES ($1)"), &[&(i as i32 + 1)])
            .map_err(backend)?;
        t.commit().map_err(backend)?;
    }
    Ok(())
}

fn chat_thread_of(r: &postgres::Row) -> ChatThread {
    ChatThread {
        owner: r.get(0),
        id: r.get(1),
        title: r.get(2),
        created_at_ms: r.get(3),
        updated_at_ms: r.get(4),
        tokens: r.get::<_, i64>(5) as u64,
    }
}

fn status_text(s: Status) -> &'static str {
    match s {
        Status::Active => "active",
        Status::Superseded => "superseded",
    }
}

fn status_from(s: &str) -> Status {
    if s == "superseded" { Status::Superseded } else { Status::Active }
}

type Statements = HashMap<String, Statement>;

/// Runs one statement of a transaction, prepared once per connection.
fn exec(t: &mut postgres::Transaction<'_>, cache: &mut Statements, sql: &str, params: &[&(dyn ToSql + Sync)]) -> Result<u64, postgres::Error> {
    let st = match cache.get(sql) {
        Some(st) => st.clone(),
        None => {
            let st = t.prepare(sql)?;
            cache.insert(sql.to_string(), st.clone());
            st
        }
    };
    t.execute(&st, params)
}

fn commit_in_transaction(conn: &mut Conn, schema: &str, rewrites: &[(u64, serde_json::Value)], batch: &[(&Event, &[Mutation])]) -> Result<(), PgError> {
    let Conn { client, statements: cache, .. } = conn;
    let mut t = client.transaction().map_err(backend)?;
    for (seq, payload) in rewrites {
        let n = t
            .execute(&format!("UPDATE {schema}.events SET payload = $2 WHERE seq = $1"), &[&(*seq as i64), payload])
            .map_err(backend)?;
        if n != 1 {
            return Err(PgError::Backend(format!("there's no event {seq} to rewrite")));
        }
    }
    let head_sql = format!("SELECT max(seq) FROM {schema}.events");
    let head_st = match cache.get(&head_sql) {
        Some(st) => st.clone(),
        None => {
            let st = t.prepare(&head_sql).map_err(backend)?;
            cache.insert(head_sql.clone(), st.clone());
            st
        }
    };
    let head: Option<i64> = t.query_one(&head_st, &[]).map_err(backend)?.get(0);
    let mut known_head = head.map(|h| h as u64);
    for (e, _) in batch {
        if e.seq != known_head.map_or(0, |h| h + 1) {
            return Err(PgError::SequenceGap { head: known_head, got: e.seq });
        }
        known_head = Some(e.seq);
    }
    let net = net_mutations(batch.iter().map(|(_, m)| *m)).map_err(|key| PgError::MissingRow { key })?;
    for table in Table::ALL {
        let ids: Vec<&str> = net.must_exist.iter().filter(|k| k.table == table).map(|k| k.id.0.as_str()).collect();
        if ids.is_empty() {
            continue;
        }
        let found = t
            .query(&format!("SELECT id FROM {schema}.{} WHERE id = ANY($1)", table_name(table)), &[&ids])
            .map_err(backend)?;
        if found.len() != ids.len() {
            let have: Vec<String> = found.iter().map(|r| r.get(0)).collect();
            let missing = ids.iter().find(|i| !have.iter().any(|h| h == *i)).copied().unwrap_or_default();
            return Err(PgError::MissingRow {
                key: RowKey {
                    table,
                    id: ObjectId(missing.into()),
                },
            });
        }
    }
    insert_events(&mut t, cache, schema, batch)?;
    for table in Table::ALL {
        let ids: Vec<&str> = net.deletes.iter().filter(|k| k.table == table).map(|k| k.id.0.as_str()).collect();
        if !ids.is_empty() {
            t.execute(&format!("DELETE FROM {schema}.{} WHERE id = ANY($1)", table_name(table)), &[&ids])
                .map_err(backend)?;
        }
    }
    upsert_rows(&mut t, cache, schema, &net.puts)?;
    t.commit().map_err(backend)
}

fn table_name(t: Table) -> &'static str {
    match t {
        Table::Source => "sources",
        Table::Page => "pages",
        Table::Edge => "edges",
        Table::Segment => "segments",
        Table::Assertion => "assertions",
        Table::Evidence => "evidence",
        Table::ChangeSet => "changesets",
        Table::Job => "jobs",
        Table::Rendition => "renditions",
        Table::EntityType => "entity_types",
        Table::Entity => "entities",
        Table::Derivation => "derivations",
        Table::Extraction => "extractions",
        Table::ReviewVerdict => "review_verdicts",
        Table::AutoCommit => "auto_commit",
        Table::AuditRun => "audit_runs",
        Table::AuditCheck => "audit_checks",
        Table::AuditState => "audit_state",
        Table::AuditPlanted => "audit_planted",
        Table::ChatSettings => "chat_settings",
        Table::Block => "blocks",
        Table::Slug => "page_slugs",
        Table::Revision => "page_revisions",
    }
}

fn insert_events(t: &mut postgres::Transaction<'_>, cache: &mut Statements, schema: &str, batch: &[(&Event, &[Mutation])]) -> Result<(), PgError> {
    let seq: Vec<i64> = batch.iter().map(|(e, _)| e.seq as i64).collect();
    let actor: Vec<&str> = batch.iter().map(|(e, _)| e.actor.as_str()).collect();
    let time: Vec<i64> = batch.iter().map(|(e, _)| e.time_ms).collect();
    let kind: Vec<&str> = batch.iter().map(|(e, _)| e.kind.as_str()).collect();
    let version: Vec<i32> = batch.iter().map(|(e, _)| e.schema_version as i32).collect();
    let payload: Vec<Value> = batch.iter().map(|(e, _)| e.payload.clone()).collect();
    exec(
            t,
            cache,
            &format!("INSERT INTO {schema}.events (seq, actor, time_ms, kind, schema_version, payload) SELECT * FROM unnest($1::bigint[], $2::text[], $3::bigint[], $4::text[], $5::int[], $6::jsonb[])"),
        &[&seq, &actor, &time, &kind, &version, &payload],
    )
    .map_err(backend)?;
    Ok(())
}

fn state_text(s: JobState) -> &'static str {
    match s {
        JobState::Queued => "queued",
        JobState::Running => "running",
        JobState::Done => "done",
        JobState::Dead => "dead",
        JobState::Cancelled => "cancelled",
    }
}

fn upsert_json_row(t: &mut postgres::Transaction<'_>, cache: &mut Statements, schema: &str, row: &Row) -> Result<(), PgError> {
    let json = serde_json::to_value(row).map_err(|e| PgError::Backend(e.to_string()))?;
    let id = row.header().id.0.as_str();
    match row {
        Row::Segment(x) => exec(
            t,
            cache,
            &format!("INSERT INTO {schema}.segments (id, row, source_id, text, rendition_id) VALUES ($1, $2, $3, $4, $5) ON CONFLICT (id) DO UPDATE SET row = excluded.row, source_id = excluded.source_id, text = excluded.text, rendition_id = excluded.rendition_id"),
            &[&id, &json, &x.source_id.0, &x.text, &x.rendition_id.as_ref().map(|r| r.0.as_str())],
        ),
        Row::Assertion(x) => exec(
            t,
            cache,
            &format!("INSERT INTO {schema}.assertions (id, row, subject, predicate, canonical_hash, active) VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (id) DO UPDATE SET row = excluded.row, subject = excluded.subject, predicate = excluded.predicate, canonical_hash = excluded.canonical_hash, active = excluded.active"),
            &[&id, &json, &x.subject, &x.predicate, &x.canonical_hash, &(x.header.status == Status::Active)],
        ),
        Row::Evidence(x) => exec(
            t,
            cache,
            &format!("INSERT INTO {schema}.evidence (id, row, assertion_id, segment_id, anchor_broken) VALUES ($1, $2, $3, $4, $5) ON CONFLICT (id) DO UPDATE SET row = excluded.row, assertion_id = excluded.assertion_id, segment_id = excluded.segment_id, anchor_broken = excluded.anchor_broken"),
            &[&id, &json, &x.assertion_id.0, &x.segment_id.0, &x.anchor_broken],
        ),
        Row::Rendition(x) => exec(
            t,
            cache,
            &format!("INSERT INTO {schema}.renditions (id, row, source_id) VALUES ($1, $2, $3) ON CONFLICT (id) DO UPDATE SET row = excluded.row"),
            &[&id, &json, &x.source_id.0],
        ),
        Row::Job(x) => exec(
            t,
            cache,
            &format!("INSERT INTO {schema}.jobs (id, row, kind, state, dedupe_key, next_run_at_ms, lease_until_ms) VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT (id) DO UPDATE SET row = excluded.row, state = excluded.state, next_run_at_ms = excluded.next_run_at_ms, lease_until_ms = excluded.lease_until_ms"),
            &[&id, &json, &x.kind, &state_text(x.state), &x.dedupe_key, &x.next_run_at_ms, &x.lease_until_ms],
        ),
        Row::ChangeSet(x) => exec(
            t,
            cache,
            &format!("INSERT INTO {schema}.changesets (id, row, state, idempotency_key, proposer, extractor, prompt_version, item_types, source_ids, rendition_ids) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) ON CONFLICT (id) DO UPDATE SET row = excluded.row, state = excluded.state, extractor = excluded.extractor, prompt_version = excluded.prompt_version, item_types = excluded.item_types, source_ids = excluded.source_ids, rendition_ids = excluded.rendition_ids"),
            &[
                &id,
                &json,
                &format!("{:?}", x.state).to_lowercase(),
                &x.idempotency_key,
                &x.proposer,
                &x.origin.as_ref().map(|o| o.extractor.as_str()),
                &x.origin.as_ref().map(|o| o.prompt_version.as_str()),
                &x.origin.as_ref().map(|o| o.item_types.clone()),
                &x.origin.as_ref().map(|o| o.source_ids.iter().map(|i| i.0.clone()).collect::<Vec<String>>()),
                &x.origin.as_ref().map(|o| o.rendition_ids.iter().map(|i| i.0.clone()).collect::<Vec<String>>()),
            ],
        ),
        Row::EntityType(x) => exec(
            t,
            cache,
            &format!("INSERT INTO {schema}.entity_types (id, row, name, version_no) VALUES ($1, $2, $3, $4) ON CONFLICT (id) DO UPDATE SET row = excluded.row"),
            &[&id, &json, &x.name, &(x.version_no as i32)],
        ),
        Row::Entity(x) => return upsert_entity(t, cache, schema, x, &json),
        Row::Derivation(x) => exec(
            t,
            cache,
            &format!("INSERT INTO {schema}.derivations (id, row, rendition_id) VALUES ($1, $2, $3) ON CONFLICT (id) DO UPDATE SET row = excluded.row"),
            &[&id, &json, &x.rendition_id.0],
        ),
        Row::Extraction(_) => exec(
            t,
            cache,
            &format!("INSERT INTO {schema}.extractions (id, row) VALUES ($1, $2) ON CONFLICT (id) DO UPDATE SET row = excluded.row"),
            &[&id, &json],
        ),
        Row::ReviewVerdict(x) => exec(
            t,
            cache,
            &format!("INSERT INTO {schema}.review_verdicts (id, row, verdict) VALUES ($1, $2, $3) ON CONFLICT (id) DO UPDATE SET row = excluded.row, verdict = excluded.verdict"),
            &[&id, &json, &x.verdict.as_str()],
        ),
        Row::AutoCommit(_) => exec(
            t,
            cache,
            &format!("INSERT INTO {schema}.auto_commit (id, row) VALUES ($1, $2) ON CONFLICT (id) DO UPDATE SET row = excluded.row"),
            &[&id, &json],
        ),
        Row::AuditRun(_) => exec(
            t,
            cache,
            &format!("INSERT INTO {schema}.audit_runs (id, row) VALUES ($1, $2) ON CONFLICT (id) DO UPDATE SET row = excluded.row"),
            &[&id, &json],
        ),
        Row::AuditCheck(x) => exec(
            t,
            cache,
            &format!("INSERT INTO {schema}.audit_checks (id, row, run_id, claim_id, outcome) VALUES ($1, $2, $3, $4, $5) ON CONFLICT (id) DO UPDATE SET row = excluded.row, outcome = excluded.outcome"),
            &[&id, &json, &x.run_id.0, &x.claim_id.0, &x.outcome.as_str()],
        ),
        Row::AuditState(_) => exec(
            t,
            cache,
            &format!("INSERT INTO {schema}.audit_state (id, row) VALUES ($1, $2) ON CONFLICT (id) DO UPDATE SET row = excluded.row"),
            &[&id, &json],
        ),
        Row::AuditPlanted(_) => exec(
            t,
            cache,
            &format!("INSERT INTO {schema}.audit_planted (id, row) VALUES ($1, $2) ON CONFLICT (id) DO UPDATE SET row = excluded.row"),
            &[&id, &json],
        ),
        Row::ChatSettings(_) => exec(
            t,
            cache,
            &format!("INSERT INTO {schema}.chat_settings (id, row) VALUES ($1, $2) ON CONFLICT (id) DO UPDATE SET row = excluded.row"),
            &[&id, &json],
        ),
        Row::Block(x) => exec(
            t,
            cache,
            &format!("INSERT INTO {schema}.blocks (id, row, page_id, position, kind) VALUES ($1, $2, $3, $4, $5) ON CONFLICT (id) DO UPDATE SET row = excluded.row, page_id = excluded.page_id, position = excluded.position, kind = excluded.kind"),
            &[&id, &json, &x.page_id.0, &(x.position as i32), &block_kind_text(x.kind)],
        ),
        Row::Slug(x) => exec(
            t,
            cache,
            &format!("INSERT INTO {schema}.page_slugs (id, row, page_id) VALUES ($1, $2, $3) ON CONFLICT (id) DO UPDATE SET row = excluded.row, page_id = excluded.page_id"),
            &[&id, &json, &x.page_id.0],
        ),
        Row::Revision(x) => exec(
            t,
            cache,
            &format!("INSERT INTO {schema}.page_revisions (id, row, page_id, revision) VALUES ($1, $2, $3, $4) ON CONFLICT (id) DO UPDATE SET row = excluded.row"),
            &[&id, &json, &x.page_id.0, &(x.revision as i32)],
        ),
        _ => return Ok(()),
    }
    .map_err(backend)?;
    Ok(())
}

/// The field-value table's column for a value, and the value as a parameter.
fn filter_param(value: &FilterValue) -> (&'static str, Box<dyn ToSql + Sync>) {
    match value {
        FilterValue::Text(t) => ("text_value", Box::new(t.clone())),
        FilterValue::Number(n) => ("number_value", Box::new(*n)),
        FilterValue::Time(t) => ("time_value", Box::new(*t)),
    }
}

fn block_kind_text(kind: BlockKind) -> String {
    format!("{kind:?}").to_lowercase()
}

/// The row and its values in the field table, which are replaced whole, so the table can't drift from the row.
fn upsert_entity(t: &mut postgres::Transaction<'_>, cache: &mut Statements, schema: &str, x: &EntityRow, json: &Value) -> Result<(), PgError> {
    let id = x.header.id.0.as_str();
    exec(
        t,
        cache,
        &format!("INSERT INTO {schema}.entities (id, row, type_name, current) VALUES ($1, $2, $3, $4) ON CONFLICT (id) DO UPDATE SET row = excluded.row, type_name = excluded.type_name, current = excluded.current"),
        &[&id, json, &x.type_name, &x.is_current()],
    )
    .map_err(backend)?;
    exec(t, cache, &format!("DELETE FROM {schema}.entity_fields WHERE entity_id = $1"), &[&id]).map_err(backend)?;
    let values = x.value_rows();
    if values.is_empty() {
        return Ok(());
    }
    let fields: Vec<&str> = values.iter().map(|v| v.field.as_str()).collect();
    let texts: Vec<Option<&str>> = values.iter().map(|v| v.text.as_deref()).collect();
    let numbers: Vec<Option<f64>> = values.iter().map(|v| v.number).collect();
    let times: Vec<Option<i64>> = values.iter().map(|v| v.time_ms).collect();
    exec(
        t,
        cache,
        &format!("INSERT INTO {schema}.entity_fields (entity_id, type_name, field, text_value, number_value, time_value) SELECT $1, $2, * FROM unnest($3::text[], $4::text[], $5::float8[], $6::bigint[])"),
        &[&id, &x.type_name, &fields, &texts, &numbers, &times],
    )
    .map_err(backend)?;
    Ok(())
}

struct HeaderColumns<'a> {
    id: Vec<&'a str>,
    creator: Vec<&'a str>,
    status: Vec<&'static str>,
    version: Vec<i32>,
    acl: Vec<Option<&'a str>>,
}

fn header_columns<'a>(headers: impl Iterator<Item = &'a Header>) -> HeaderColumns<'a> {
    let mut c = HeaderColumns {
        id: vec![],
        creator: vec![],
        status: vec![],
        version: vec![],
        acl: vec![],
    };
    for h in headers {
        c.id.push(&h.id.0);
        c.creator.push(&h.creator);
        c.status.push(status_text(h.status));
        c.version.push(h.version as i32);
        c.acl.push(h.acl_id.as_deref());
    }
    c
}

const HEADER_CAST: &str = "$1::text[], $2::text[], $3::text[], $4::int[], $5::text[]";
const HEADER_SET: &str = "creator = excluded.creator, status = excluded.status, version = excluded.version, acl_id = excluded.acl_id";

fn upsert_rows(t: &mut postgres::Transaction<'_>, cache: &mut Statements, schema: &str, rows: &[Row]) -> Result<(), PgError> {
    let sources: Vec<&SourceRow> = rows.iter().filter_map(|r| if let Row::Source(x) = r { Some(x) } else { None }).collect();
    let pages: Vec<&PageRow> = rows.iter().filter_map(|r| if let Row::Page(x) = r { Some(x) } else { None }).collect();
    let edges: Vec<&EdgeRow> = rows.iter().filter_map(|r| if let Row::Edge(x) = r { Some(x) } else { None }).collect();
    for row in rows {
        upsert_json_row(t, cache, schema, row)?;
    }
    if !sources.is_empty() {
        let h = header_columns(sources.iter().map(|r| &r.header));
        let sha: Vec<&str> = sources.iter().map(|r| r.sha256.as_str()).collect();
        let media: Vec<&str> = sources.iter().map(|r| r.media_type.as_str()).collect();
        let tier: Vec<&str> = sources.iter().map(|r| r.trust_tier.as_str()).collect();
        let at: Vec<i64> = sources.iter().map(|r| r.recorded_at_ms).collect();
        let name: Vec<&str> = sources.iter().map(|r| r.name.as_str()).collect();
        let size: Vec<i64> = sources.iter().map(|r| r.size as i64).collect();
        let logical: Vec<&str> = sources.iter().map(|r| r.logical_id.0.as_str()).collect();
        let vno: Vec<i32> = sources.iter().map(|r| r.version_no as i32).collect();
        let meta: Vec<Value> = sources.iter().map(|r| r.metadata.clone()).collect();
        let params: [&(dyn ToSql + Sync); 14] = [
            &h.id, &h.creator, &h.status, &h.version, &h.acl, &sha, &media, &tier, &at, &name, &size, &logical, &vno, &meta,
        ];
        exec(
            t,
            cache,
            &format!("INSERT INTO {schema}.sources (id, creator, status, version, acl_id, sha256, media_type, trust_tier, recorded_at_ms, name, size, logical_id, version_no, metadata) SELECT * FROM unnest({HEADER_CAST}, $6::text[], $7::text[], $8::text[], $9::bigint[], $10::text[], $11::bigint[], $12::text[], $13::int[], $14::jsonb[]) ON CONFLICT (id) DO UPDATE SET {HEADER_SET}, sha256 = excluded.sha256, media_type = excluded.media_type, trust_tier = excluded.trust_tier, recorded_at_ms = excluded.recorded_at_ms, name = excluded.name, size = excluded.size, logical_id = excluded.logical_id, version_no = excluded.version_no, metadata = excluded.metadata"),
            &params,
        )
        .map_err(backend)?;
    }
    if !pages.is_empty() {
        let h = header_columns(pages.iter().map(|r| &r.header));
        let title: Vec<&str> = pages.iter().map(|r| r.title.as_str()).collect();
        let body: Vec<&str> = pages.iter().map(|r| r.body.as_str()).collect();
        let at: Vec<i64> = pages.iter().map(|r| r.updated_at_ms).collect();
        let slug: Vec<&str> = pages.iter().map(|r| r.slug.as_str()).collect();
        let redirects: Vec<Value> = pages.iter().map(|r| serde_json::json!(r.redirects)).collect();
        let params: [&(dyn ToSql + Sync); 10] = [&h.id, &h.creator, &h.status, &h.version, &h.acl, &title, &body, &at, &slug, &redirects];
        exec(
            t,
            cache,
            &format!("INSERT INTO {schema}.pages SELECT * FROM unnest({HEADER_CAST}, $6::text[], $7::text[], $8::bigint[], $9::text[], $10::jsonb[]) ON CONFLICT (id) DO UPDATE SET {HEADER_SET}, title = excluded.title, body = excluded.body, updated_at_ms = excluded.updated_at_ms, slug = excluded.slug, redirects = excluded.redirects"),
            &params,
        )
        .map_err(backend)?;
    }
    if !edges.is_empty() {
        let h = header_columns(edges.iter().map(|r| &r.header));
        let from: Vec<&str> = edges.iter().map(|r| r.from.0.as_str()).collect();
        let to: Vec<&str> = edges.iter().map(|r| r.to.0.as_str()).collect();
        let kind: Vec<&str> = edges.iter().map(|r| r.kind.as_str()).collect();
        let block: Vec<Option<&str>> = edges.iter().map(|r| r.block_id.as_ref().map(|b| b.0.as_str())).collect();
        let params: [&(dyn ToSql + Sync); 9] = [&h.id, &h.creator, &h.status, &h.version, &h.acl, &from, &to, &kind, &block];
        exec(
            t,
            cache,
            &format!("INSERT INTO {schema}.edges SELECT * FROM unnest({HEADER_CAST}, $6::text[], $7::text[], $8::text[], $9::text[]) ON CONFLICT (id) DO UPDATE SET {HEADER_SET}, from_id = excluded.from_id, to_id = excluded.to_id, kind = excluded.kind, block_id = excluded.block_id"),
            &params,
        )
        .map_err(backend)?;
    }
    Ok(())
}

/// A repeatable-read transaction on a pooled connection, returned on drop.
pub struct PgSnapshot<'a> {
    store: &'a PgStore,
    vault: VaultId,
    schema: String,
    conn: RefCell<Option<Conn>>,
}

impl Drop for PgSnapshot<'_> {
    fn drop(&mut self) {
        if let Some(mut c) = self.conn.borrow_mut().take()
            && c.client.batch_execute("ROLLBACK").is_ok()
        {
            self.store.give_client(c);
        }
    }
}

fn header_from(r: &postgres::Row) -> Header {
    Header {
        id: ObjectId(r.get("id")),
        creator: r.get("creator"),
        status: status_from(r.get("status")),
        version: r.get::<_, i32>("version") as u32,
        acl_id: r.get("acl_id"),
    }
}

/// Newer kinds keep the full row as JSONB beside their indexed columns, so a field added to the row needs no new SQL.
fn json_row(r: &postgres::Row) -> Row {
    serde_json::from_value(r.get("row")).expect("a row this binary wrote")
}

fn json_revision(r: &postgres::Row) -> Option<RevisionRow> {
    match json_row(r) {
        Row::Revision(x) => Some(x),
        _ => None,
    }
}

fn row_from(table: Table, r: &postgres::Row) -> Row {
    if matches!(
        table,
        Table::Segment
            | Table::Assertion
            | Table::Evidence
            | Table::ChangeSet
            | Table::Job
            | Table::Rendition
            | Table::EntityType
            | Table::Entity
            | Table::Derivation
            | Table::Extraction
            | Table::ReviewVerdict
            | Table::AutoCommit
            | Table::AuditRun
            | Table::AuditCheck
            | Table::AuditState
            | Table::AuditPlanted
            | Table::ChatSettings
            | Table::Block
            | Table::Slug
            | Table::Revision
    ) {
        return json_row(r);
    }
    let header = header_from(r);
    match table {
        Table::Source => Row::Source(SourceRow {
            header,
            sha256: r.get("sha256"),
            media_type: r.get("media_type"),
            trust_tier: r.get("trust_tier"),
            recorded_at_ms: r.get("recorded_at_ms"),
            name: r.get("name"),
            size: r.get::<_, i64>("size") as u64,
            logical_id: ObjectId(r.get("logical_id")),
            version_no: r.get::<_, i32>("version_no") as u32,
            metadata: r.get("metadata"),
        }),
        Table::Page => Row::Page(PageRow {
            header,
            title: r.get("title"),
            body: r.get("body"),
            updated_at_ms: r.get("updated_at_ms"),
            slug: r.get("slug"),
            redirects: serde_json::from_value(r.get("redirects")).expect("a row this binary wrote"),
        }),
        Table::Edge => Row::Edge(EdgeRow {
            header,
            from: ObjectId(r.get("from_id")),
            to: ObjectId(r.get("to_id")),
            kind: r.get("kind"),
            block_id: r.get::<_, Option<String>>("block_id").map(ObjectId),
        }),
        Table::Segment
        | Table::Assertion
        | Table::Evidence
        | Table::ChangeSet
        | Table::Job
        | Table::Rendition
        | Table::EntityType
        | Table::Entity
        | Table::Derivation
        | Table::Extraction
        | Table::ReviewVerdict
        | Table::AutoCommit
        | Table::AuditRun
        | Table::AuditCheck
        | Table::AuditState
        | Table::AuditPlanted
        | Table::ChatSettings
        | Table::Block
        | Table::Slug
        | Table::Revision => {
            unreachable!("handled above")
        }
    }
}

impl PgSnapshot<'_> {
    /// Runs a query with its statement prepared once per connection.
    fn with_statement<T>(&self, sql: &str, f: impl FnOnce(&mut Client, &Statement) -> Result<T, postgres::Error>) -> Result<T, PgError> {
        let mut slot = self.conn.borrow_mut();
        let conn = slot.as_mut().ok_or_else(|| PgError::Backend("snapshot closed".into()))?;
        let st = conn.statement(sql).map_err(backend)?;
        f(&mut conn.client, &st).map_err(backend)
    }

    pub fn head(&self) -> Result<Option<u64>, PgError> {
        let sql = format!("SELECT max(seq) FROM {}.events", self.schema);
        let v: Option<i64> = self.with_statement(&sql, |c, st| c.query_one(st, &[]))?.get(0);
        Ok(v.map(|h| h as u64))
    }

    pub fn get(&self, key: &RowKey) -> Result<Option<Row>, PgError> {
        let sql = format!("SELECT * FROM {}.{} WHERE id = $1", self.schema, table_name(key.table));
        let row = self.with_statement(&sql, |c, st| c.query_opt(st, &[&key.id.0]))?;
        Ok(row.as_ref().map(|r| row_from(key.table, r)))
    }

    /// One query per table the keys touch, never one per key.
    pub fn get_many(&self, keys: &[RowKey]) -> Result<Vec<Option<Row>>, PgError> {
        let mut found: HashMap<RowKey, Row> = HashMap::new();
        for table in Table::ALL {
            let ids: Vec<&str> = keys.iter().filter(|k| k.table == table).map(|k| k.id.0.as_str()).collect();
            if ids.is_empty() {
                continue;
            }
            let sql = format!("SELECT * FROM {}.{} WHERE id = ANY($1)", self.schema, table_name(table));
            for r in self.with_statement(&sql, |c, st| c.query(st, &[&ids]))? {
                let row = row_from(table, &r);
                found.insert(row.key(), row);
            }
        }
        Ok(keys.iter().map(|k| found.get(k).cloned()).collect())
    }

    /// A hash of each row, computed in the database, so the rows themselves don't have to cross the network. One query per table the keys touch.
    pub fn row_stamps(&self, keys: &[RowKey]) -> Result<Vec<Option<String>>, PgError> {
        let mut found: HashMap<RowKey, String> = HashMap::new();
        for table in Table::ALL {
            let ids: Vec<&str> = keys.iter().filter(|k| k.table == table).map(|k| k.id.0.as_str()).collect();
            if ids.is_empty() {
                continue;
            }
            let sql = format!("SELECT t.id, md5(t::text) FROM {}.{} t WHERE t.id = ANY($1)", self.schema, table_name(table));
            for r in self.with_statement(&sql, |c, st| c.query(st, &[&ids]))? {
                found.insert(RowKey { table, id: ObjectId(r.get(0)) }, r.get(1));
            }
        }
        Ok(keys.iter().map(|k| found.get(k).cloned()).collect())
    }

    pub fn rows(&self, table: Table, after: Option<&ObjectId>) -> Result<Vec<Row>, PgError> {
        let name = table_name(table);
        let (where_clause, limit) = (if after.is_some() { "WHERE id > $1" } else { "" }, PAGE_LIMIT);
        let sql = format!("SELECT * FROM {}.{name} {where_clause} ORDER BY id LIMIT {limit}", self.schema);
        let rows = match after {
            Some(a) => self.with_statement(&sql, |c, st| c.query(st, &[&a.0]))?,
            None => self.with_statement(&sql, |c, st| c.query(st, &[]))?,
        };
        Ok(rows.iter().map(|r| row_from(table, r)).collect())
    }

    /// What an index should hold, worked out here so the rows don't cross the network. The hash is `memex_core::audit::id_hash`.
    pub fn id_digest(&self, kind: DocKind) -> Result<IdDigest, PgError> {
        let s = &self.schema;
        let from = match kind {
            DocKind::Claim => format!("{s}.assertions a WHERE a.active AND a.row->'assertion'->>'superseded_by' IS NULL"),
            _ => format!(
                "{s}.segments a LEFT JOIN {s}.renditions r ON r.id = a.rendition_id WHERE a.row->'segment'->'header'->>'status' = 'active' AND (a.rendition_id IS NULL OR (r.row->'rendition'->>'superseded_by' IS NULL AND r.row->'rendition'->'header'->>'status' = 'active'))"
            ),
        };
        let sql =
            format!("SELECT count(*), coalesce(bit_xor(('x' || encode(substring(sha256(a.id::bytea) from 1 for 8), 'hex'))::bit(64)::bigint), 0) FROM {from}");
        let row = self.with_statement(&sql, |c, st| c.query_one(st, &[]))?;
        Ok(IdDigest {
            count: row.get::<_, i64>(0) as u64,
            xor: row.get(1),
        })
    }

    pub fn audit_checks(
        &self,
        run: &ObjectId,
        findings_only: bool,
        after: Option<&ObjectId>,
        limit: usize,
    ) -> Result<Vec<memex_types::AuditCheckRow>, PgError> {
        let sql = format!(
            "SELECT row FROM {}.audit_checks WHERE run_id = $1 AND ($2::text IS NULL OR id > $2) AND (NOT $3 OR outcome IN ('suspect', 'failed')) ORDER BY id LIMIT $4",
            self.schema
        );
        let rows = self.with_statement(&sql, |c, st| {
            c.query(st, &[&run.0, &after.map(|a| a.0.as_str()), &findings_only, &(limit as i64)])
        })?;
        Ok(rows
            .iter()
            .filter_map(|r| if let Row::AuditCheck(x) = json_row(r) { Some(x) } else { None })
            .collect())
    }

    pub fn claim_counted_correct(&self, claim: &ObjectId) -> Result<bool, PgError> {
        let sql = format!(
            "SELECT EXISTS (SELECT 1 FROM {}.audit_checks WHERE claim_id = $1 AND (row->'audit_check'->>'counted_correct')::boolean)",
            self.schema
        );
        Ok(self.with_statement(&sql, |c, st| c.query_one(st, &[&claim.0]))?.get(0))
    }

    pub fn renditions_of(&self, source_id: &ObjectId) -> Result<Vec<RenditionRow>, PgError> {
        let sql = format!("SELECT row FROM {}.renditions WHERE source_id = $1 ORDER BY id", self.schema);
        let rows = self.with_statement(&sql, |c, st| c.query(st, &[&source_id.0]))?;
        Ok(rows
            .iter()
            .filter_map(|r| if let Row::Rendition(x) = json_row(r) { Some(x) } else { None })
            .collect())
    }

    pub fn derivations_of(&self, rendition_id: &ObjectId, after: Option<&ObjectId>, limit: usize) -> Result<Vec<memex_types::DerivationRow>, PgError> {
        let sql = format!(
            "SELECT row FROM {}.derivations WHERE rendition_id = $1 AND ($2::text IS NULL OR id > $2) ORDER BY id LIMIT $3",
            self.schema
        );
        let after = after.map(|a| a.0.as_str());
        let rows = self.with_statement(&sql, |c, st| c.query(st, &[&rendition_id.0, &after, &(limit as i64)]))?;
        Ok(rows
            .iter()
            .filter_map(|r| if let Row::Derivation(x) = json_row(r) { Some(x) } else { None })
            .collect())
    }

    pub fn segments_of(&self, rendition_id: &ObjectId, after: Option<&ObjectId>, limit: usize) -> Result<Vec<SegmentRow>, PgError> {
        let sql = format!(
            "SELECT row FROM {}.segments WHERE rendition_id = $1 AND ($2::text IS NULL OR id > $2) ORDER BY id LIMIT $3",
            self.schema
        );
        let after = after.map(|a| a.0.as_str());
        let rows = self.with_statement(&sql, |c, st| c.query(st, &[&rendition_id.0, &after, &(limit as i64)]))?;
        Ok(rows
            .iter()
            .filter_map(|r| if let Row::Segment(x) = json_row(r) { Some(x) } else { None })
            .collect())
    }

    pub fn evidence_of(&self, assertion_ids: &[ObjectId]) -> Result<Vec<EvidenceRow>, PgError> {
        let ids: Vec<&str> = assertion_ids.iter().map(|i| i.0.as_str()).collect();
        let sql = format!("SELECT row FROM {}.evidence WHERE assertion_id = ANY($1) ORDER BY id", self.schema);
        let rows = self.with_statement(&sql, |c, st| c.query(st, &[&ids]))?;
        Ok(rows
            .iter()
            .filter_map(|r| if let Row::Evidence(x) = json_row(r) { Some(x) } else { None })
            .collect())
    }

    pub fn evidence_citing(&self, segment_ids: &[ObjectId]) -> Result<Vec<EvidenceRow>, PgError> {
        let ids: Vec<&str> = segment_ids.iter().map(|i| i.0.as_str()).collect();
        let sql = format!("SELECT row FROM {}.evidence WHERE segment_id = ANY($1) ORDER BY id", self.schema);
        let rows = self.with_statement(&sql, |c, st| c.query(st, &[&ids]))?;
        Ok(rows
            .iter()
            .filter_map(|r| if let Row::Evidence(x) = json_row(r) { Some(x) } else { None })
            .collect())
    }

    pub fn edges_touching(&self, ids: &[ObjectId], kind: &str) -> Result<Vec<EdgeRow>, PgError> {
        let ids: Vec<&str> = ids.iter().map(|i| i.0.as_str()).collect();
        let sql = format!(
            "SELECT * FROM {}.edges WHERE kind = $1 AND status = 'active' AND (from_id = ANY($2) OR to_id = ANY($2)) ORDER BY id",
            self.schema
        );
        let rows = self.with_statement(&sql, |c, st| c.query(st, &[&kind, &ids]))?;
        Ok(rows
            .iter()
            .filter_map(|r| if let Row::Edge(x) = row_from(Table::Edge, r) { Some(x) } else { None })
            .collect())
    }

    pub fn blocks_of(&self, pages: &[ObjectId], limit: usize) -> Result<Vec<BlockRow>, PgError> {
        let ids: Vec<&str> = pages.iter().map(|i| i.0.as_str()).collect();
        let sql = format!(
            "SELECT row FROM {}.blocks WHERE page_id = ANY($1) ORDER BY page_id, position LIMIT $2",
            self.schema
        );
        let rows = self.with_statement(&sql, |c, st| c.query(st, &[&ids, &(limit as i64)]))?;
        Ok(rows
            .iter()
            .filter_map(|r| if let Row::Block(x) = json_row(r) { Some(x) } else { None })
            .collect())
    }

    pub fn headings_of(&self, pages: &[ObjectId]) -> Result<Vec<BlockRow>, PgError> {
        let ids: Vec<&str> = pages.iter().map(|i| i.0.as_str()).collect();
        let sql = format!(
            "SELECT row FROM {}.blocks WHERE page_id = ANY($1) AND kind = 'heading' ORDER BY page_id, position",
            self.schema
        );
        let rows = self.with_statement(&sql, |c, st| c.query(st, &[&ids]))?;
        Ok(rows
            .iter()
            .filter_map(|r| if let Row::Block(x) = json_row(r) { Some(x) } else { None })
            .collect())
    }

    pub fn page_summaries(&self, after: Option<&ObjectId>, limit: usize) -> Result<Vec<PageRow>, PgError> {
        let sql = format!(
            "SELECT id, creator, status, version, acl_id, title, '' AS body, updated_at_ms, slug, redirects FROM {}.pages WHERE $1::text IS NULL OR id > $1 ORDER BY id LIMIT $2",
            self.schema
        );
        let after = after.map(|a| a.0.as_str());
        let rows = self.with_statement(&sql, |c, st| c.query(st, &[&after, &(limit as i64)]))?;
        Ok(rows
            .iter()
            .filter_map(|r| if let Row::Page(x) = row_from(Table::Page, r) { Some(x) } else { None })
            .collect())
    }

    pub fn revisions_of(&self, page: &ObjectId, after: Option<u32>, limit: usize) -> Result<Vec<RevisionRow>, PgError> {
        let sql = format!(
            "SELECT row #- '{{revision,blocks}}' AS row FROM {}.page_revisions WHERE page_id = $1 AND revision > $2 ORDER BY revision LIMIT $3",
            self.schema
        );
        let after = after.map_or(-1, |a| a as i32);
        let rows = self.with_statement(&sql, |c, st| c.query(st, &[&page.0, &after, &(limit as i64)]))?;
        Ok(rows.iter().filter_map(json_revision).collect())
    }

    pub fn edges_linked(
        &self,
        ids: &[ObjectId],
        kinds: &[&str],
        direction: EdgeDirection,
        after: Option<&ObjectId>,
        limit: usize,
    ) -> Result<Vec<EdgeRow>, PgError> {
        let ids: Vec<&str> = ids.iter().map(|i| i.0.as_str()).collect();
        let ends = match direction {
            EdgeDirection::From => "from_id = ANY($2)",
            EdgeDirection::To => "to_id = ANY($2)",
            EdgeDirection::Both => "(from_id = ANY($2) OR to_id = ANY($2))",
        };
        let sql = format!(
            "SELECT * FROM {}.edges WHERE kind = ANY($1) AND status = 'active' AND {ends} AND ($3::text IS NULL OR id > $3) ORDER BY id LIMIT $4",
            self.schema
        );
        let after = after.map(|a| a.0.as_str());
        let rows = self.with_statement(&sql, |c, st| c.query(st, &[&kinds, &ids, &after, &(limit as i64)]))?;
        Ok(rows
            .iter()
            .filter_map(|r| if let Row::Edge(x) = row_from(Table::Edge, r) { Some(x) } else { None })
            .collect())
    }

    pub fn unresolved_links(&self, after: Option<&ObjectId>, limit: usize) -> Result<Vec<EdgeRow>, PgError> {
        let s = &self.schema;
        let sql = format!(
            "SELECT e.* FROM {s}.edges AS e WHERE ($1::text IS NULL OR e.id > $1) AND (
                (e.kind = 'wiki_link' AND NOT EXISTS (SELECT 1 FROM {s}.page_slugs AS g WHERE 'slug:' || g.id = e.to_id))
                OR (e.kind = 'claim_ref' AND NOT EXISTS (SELECT 1 FROM {s}.assertions AS a WHERE a.id = e.to_id))
             ) ORDER BY e.id LIMIT $2"
        );
        let after = after.map(|a| a.0.as_str());
        let rows = self.with_statement(&sql, |c, st| c.query(st, &[&after, &(limit as i64)]))?;
        Ok(rows
            .iter()
            .filter_map(|r| if let Row::Edge(x) = row_from(Table::Edge, r) { Some(x) } else { None })
            .collect())
    }

    pub fn sections_of(&self, sections: &[(ObjectId, String)], limit: usize) -> Result<Vec<SegmentRow>, PgError> {
        let renditions: Vec<&str> = sections.iter().map(|s| s.0.0.as_str()).collect();
        let headings: Vec<&str> = sections.iter().map(|s| s.1.as_str()).collect();
        let sql = format!(
            "SELECT row FROM {s}.segments AS g JOIN unnest($1::text[], $2::text[]) AS w(r, h) ON g.rendition_id = w.r AND g.row->'segment'->>'heading' = w.h
             ORDER BY g.rendition_id, (g.row->'segment'->>'start')::bigint LIMIT $3",
            s = self.schema
        );
        let rows = self.with_statement(&sql, |c, st| c.query(st, &[&renditions, &headings, &(limit as i64)]))?;
        Ok(rows
            .iter()
            .filter_map(|r| if let Row::Segment(x) = json_row(r) { Some(x) } else { None })
            .collect())
    }

    pub fn broken_anchors(&self, after: Option<&ObjectId>, limit: usize) -> Result<Vec<EvidenceRow>, PgError> {
        let sql = format!(
            "SELECT row FROM {}.evidence WHERE anchor_broken AND ($1::text IS NULL OR id > $1) ORDER BY id LIMIT $2",
            self.schema
        );
        let after = after.map(|a| a.0.as_str());
        let rows = self.with_statement(&sql, |c, st| c.query(st, &[&after, &(limit as i64)]))?;
        Ok(rows
            .iter()
            .filter_map(|r| if let Row::Evidence(x) = json_row(r) { Some(x) } else { None })
            .collect())
    }

    pub fn source_by_hash(&self, sha256: &str) -> Result<Option<SourceRow>, PgError> {
        let sql = format!("SELECT * FROM {}.sources WHERE sha256 = $1 ORDER BY id LIMIT 1", self.schema);
        let row = self.with_statement(&sql, |c, st| c.query_opt(st, &[&sha256]))?;
        Ok(row
            .map(|r| row_from(Table::Source, &r))
            .and_then(|r| if let Row::Source(x) = r { Some(x) } else { None }))
    }

    pub fn source_versions(&self, logical_id: &ObjectId) -> Result<Vec<SourceRow>, PgError> {
        let sql = format!("SELECT * FROM {}.sources WHERE logical_id = $1 ORDER BY version_no", self.schema);
        let rows = self.with_statement(&sql, |c, st| c.query(st, &[&logical_id.0]))?;
        Ok(rows
            .iter()
            .filter_map(|r| if let Row::Source(x) = row_from(Table::Source, r) { Some(x) } else { None })
            .collect())
    }

    pub fn job_by_dedupe(&self, key: &str) -> Result<Option<JobRow>, PgError> {
        let sql = format!("SELECT row FROM {}.jobs WHERE dedupe_key = $1", self.schema);
        let row = self.with_statement(&sql, |c, st| c.query_opt(st, &[&key]))?;
        Ok(row.and_then(|r| if let Row::Job(j) = json_row(&r) { Some(j) } else { None }))
    }

    pub fn claimable_jobs(&self, kind: &str, now_ms: i64, limit: usize) -> Result<Vec<JobRow>, PgError> {
        let sql = format!(
            "SELECT row FROM {}.jobs WHERE kind = $1 AND ((state = 'queued' AND next_run_at_ms <= $2) OR (state = 'running' AND lease_until_ms <= $2)) ORDER BY id LIMIT $3",
            self.schema
        );
        let rows = self.with_statement(&sql, |c, st| c.query(st, &[&kind, &now_ms, &(limit as i64)]))?;
        Ok(rows.iter().filter_map(|r| if let Row::Job(j) = json_row(r) { Some(j) } else { None }).collect())
    }

    pub fn open_job_count(&self, except_kinds: &[&str]) -> Result<u64, PgError> {
        let sql = format!(
            "SELECT count(*) FROM {}.jobs WHERE state IN ('queued', 'running') AND NOT kind = ANY($1)",
            self.schema
        );
        let row = self.with_statement(&sql, |c, st| c.query_one(st, &[&except_kinds]))?;
        Ok(row.get::<_, i64>(0) as u64)
    }

    pub fn jobs(&self, kind: Option<&str>, state: Option<JobState>, after: Option<&ObjectId>, limit: usize) -> Result<Vec<JobRow>, PgError> {
        let sql = format!(
            "SELECT row FROM {}.jobs WHERE ($1::text IS NULL OR kind = $1) AND ($2::text IS NULL OR state = $2) AND ($3::text IS NULL OR id > $3) ORDER BY id LIMIT $4",
            self.schema
        );
        let state = state.map(state_text);
        let after = after.map(|a| a.0.as_str());
        let rows = self.with_statement(&sql, |c, st| c.query(st, &[&kind, &state, &after, &(limit as i64)]))?;
        Ok(rows.iter().filter_map(|r| if let Row::Job(j) = json_row(r) { Some(j) } else { None }).collect())
    }

    pub fn changeset_by_key(&self, key: &str) -> Result<Option<ChangeSetRow>, PgError> {
        let sql = format!("SELECT row FROM {}.changesets WHERE idempotency_key = $1", self.schema);
        let row = self.with_statement(&sql, |c, st| c.query_opt(st, &[&key]))?;
        Ok(row.map(|r| json_row(&r)).and_then(|r| if let Row::ChangeSet(c) = r { Some(c) } else { None }))
    }

    pub fn review_queue(&self, filter: &memex_types::ReviewFilter, after: Option<&ObjectId>, limit: usize) -> Result<Vec<ChangeSetRow>, PgError> {
        let sql = format!(
            "SELECT c.row FROM {s}.changesets c LEFT JOIN {s}.review_verdicts v ON v.id = c.id
             WHERE ($1::text IS NULL OR c.state = $1)
               AND ($2::text IS NULL OR c.extractor = $2)
               AND ($3::text IS NULL OR c.prompt_version = $3)
               AND ($4::text IS NULL OR $4 = ANY(c.item_types))
               AND ($5::text IS NULL OR $5 = ANY(c.source_ids))
               AND ($6::text IS NULL OR c.proposer = $6)
               AND ($7::text IS NULL OR v.verdict = $7 OR ($7 = 'none' AND v.id IS NULL))
               AND ($8::text IS NULL OR c.id > $8)
             ORDER BY c.id LIMIT $9",
            s = self.schema
        );
        let state = filter.state.map(|s| format!("{s:?}").to_lowercase());
        let verdict = filter.verdict.map(|v| match v {
            memex_types::VerdictFilter::Is(x) => x.as_str(),
            memex_types::VerdictFilter::None => "none",
        });
        let rows = self.with_statement(&sql, |c, st| {
            c.query(
                st,
                &[
                    &state,
                    &filter.extractor,
                    &filter.prompt_version,
                    &filter.item_type,
                    &filter.source_id.as_ref().map(|i| i.0.as_str()),
                    &filter.proposer,
                    &verdict,
                    &after.map(|a| a.0.as_str()),
                    &(limit as i64),
                ],
            )
        })?;
        Ok(rows
            .iter()
            .filter_map(|r| if let Row::ChangeSet(x) = json_row(r) { Some(x) } else { None })
            .collect())
    }

    pub fn review_counts(&self) -> Result<Vec<memex_types::QueueCount>, PgError> {
        let sql = format!(
            "SELECT coalesce(c.extractor, '') AS extractor, coalesce(c.prompt_version, '') AS prompt_version, t AS item_type, v.verdict, count(*) AS n
             FROM {s}.changesets c
             LEFT JOIN {s}.review_verdicts v ON v.id = c.id
             CROSS JOIN LATERAL unnest(coalesce(nullif(c.item_types, '{{}}'), ARRAY['unknown'])) AS t
             WHERE c.state = 'pending'
             GROUP BY 1, 2, 3, 4
             ORDER BY 1, 2, 3, 4",
            s = self.schema
        );
        let rows = self.with_statement(&sql, |c, st| c.query(st, &[]))?;
        Ok(rows
            .iter()
            .map(|r| memex_types::QueueCount {
                extractor: r.get("extractor"),
                prompt_version: r.get("prompt_version"),
                item_type: r.get("item_type"),
                verdict: r.get::<_, Option<String>>("verdict").and_then(|v| match v.as_str() {
                    "approve" => Some(memex_types::Verdict::Approve),
                    "reject" => Some(memex_types::Verdict::Reject),
                    "abstain" => Some(memex_types::Verdict::Abstain),
                    _ => None,
                }),
                count: r.get::<_, i64>("n") as u64,
            })
            .collect())
    }

    pub fn pending_count(&self) -> Result<u64, PgError> {
        let sql = format!("SELECT count(*) AS n FROM {}.changesets WHERE state = 'pending'", self.schema);
        let row = self.with_statement(&sql, |c, st| c.query_one(st, &[]))?;
        Ok(row.get::<_, i64>("n") as u64)
    }

    pub fn pending_citing_rendition(&self, rendition_id: &ObjectId, after: Option<&ObjectId>, limit: usize) -> Result<Vec<ChangeSetRow>, PgError> {
        let sql = format!(
            "SELECT row FROM {}.changesets WHERE state = 'pending' AND $1 = ANY(rendition_ids) AND ($2::text IS NULL OR id > $2) ORDER BY id LIMIT $3",
            self.schema
        );
        let after = after.map(|a| a.0.as_str());
        let rows = self.with_statement(&sql, |c, st| c.query(st, &[&rendition_id.0, &after, &(limit as i64)]))?;
        Ok(rows
            .iter()
            .filter_map(|r| if let Row::ChangeSet(x) = json_row(r) { Some(x) } else { None })
            .collect())
    }

    /// One query, however many hashes and pairs.
    pub fn assertions_matching(&self, hashes: &[String], pairs: &[(String, String)]) -> Result<Vec<AssertionRow>, PgError> {
        let subjects: Vec<&str> = pairs.iter().map(|p| p.0.as_str()).collect();
        let predicates: Vec<&str> = pairs.iter().map(|p| p.1.as_str()).collect();
        let sql = format!(
            "SELECT row FROM {}.assertions a WHERE active AND (canonical_hash = ANY($1) OR EXISTS (SELECT 1 FROM unnest($2::text[], $3::text[]) AS w(s, p) WHERE w.s = a.subject AND w.p = a.predicate))",
            self.schema
        );
        let rows = self.with_statement(&sql, |c, st| c.query(st, &[&hashes, &subjects, &predicates]))?;
        Ok(rows
            .iter()
            .filter_map(|r| if let Row::Assertion(a) = json_row(r) { Some(a) } else { None })
            .collect())
    }

    /// One query, in id order.
    pub fn assertions_about(&self, subjects: &[String]) -> Result<Vec<AssertionRow>, PgError> {
        let sql = format!("SELECT row FROM {}.assertions WHERE active AND subject = ANY($1) ORDER BY id", self.schema);
        let rows = self.with_statement(&sql, |c, st| c.query(st, &[&subjects]))?;
        Ok(rows
            .iter()
            .filter_map(|r| if let Row::Assertion(a) = json_row(r) { Some(a) } else { None })
            .collect())
    }

    pub fn entity_type_versions(&self, names: &[String]) -> Result<Vec<EntityTypeRow>, PgError> {
        let sql = format!("SELECT row FROM {}.entity_types WHERE name = ANY($1) ORDER BY name, version_no", self.schema);
        let rows = self.with_statement(&sql, |c, st| c.query(st, &[&names]))?;
        Ok(rows
            .iter()
            .filter_map(|r| if let Row::EntityType(t) = json_row(r) { Some(t) } else { None })
            .collect())
    }

    pub fn latest_entity_types(&self, after: Option<&str>, limit: usize) -> Result<Vec<EntityTypeRow>, PgError> {
        let sql = format!(
            "SELECT DISTINCT ON (name) row FROM {}.entity_types WHERE ($1::text IS NULL OR name > $1) ORDER BY name, version_no DESC LIMIT $2",
            self.schema
        );
        let rows = self.with_statement(&sql, |c, st| c.query(st, &[&after, &(limit as i64)]))?;
        Ok(rows
            .iter()
            .filter_map(|r| if let Row::EntityType(t) = json_row(r) { Some(t) } else { None })
            .collect())
    }

    pub fn entities_of_type(&self, type_name: &str, after: Option<&ObjectId>, limit: usize) -> Result<Vec<EntityRow>, PgError> {
        let sql = format!(
            "SELECT row FROM {}.entities WHERE current AND type_name = $1 AND ($2::text IS NULL OR id > $2) ORDER BY id LIMIT $3",
            self.schema
        );
        let after = after.map(|a| a.0.as_str());
        let rows = self.with_statement(&sql, |c, st| c.query(st, &[&type_name, &after, &(limit as i64)]))?;
        Ok(rows
            .iter()
            .filter_map(|r| if let Row::Entity(e) = json_row(r) { Some(e) } else { None })
            .collect())
    }

    /// One query. The first filter drives it through its B-tree, and any others join on the entity. Only current entities have values here.
    pub fn entities_matching(&self, type_name: &str, filters: &[FieldFilter], after: Option<&ObjectId>, limit: usize) -> Result<Vec<EntityRow>, PgError> {
        let mut params: Vec<Box<dyn ToSql + Sync>> = vec![Box::new(type_name.to_string()), Box::new(after.map(|a| a.0.clone())), Box::new(limit as i64)];
        let mut from = String::new();
        let mut conditions = String::new();
        for (i, f) in filters.iter().enumerate() {
            let alias = format!("f{i}");
            if i == 0 {
                from.push_str(&format!("{}.entity_fields f0", self.schema));
            } else {
                from.push_str(&format!(" JOIN {}.entity_fields {alias} ON {alias}.entity_id = f0.entity_id", self.schema));
            }
            params.push(Box::new(f.field.clone()));
            let field_param = params.len();
            let (column, value): (&str, Box<dyn ToSql + Sync>) = match &f.value {
                FilterValue::Text(t) => ("text_value", Box::new(t.clone())),
                FilterValue::Number(n) => ("number_value", Box::new(*n)),
                FilterValue::Time(t) => ("time_value", Box::new(*t)),
            };
            params.push(value);
            let value_param = params.len();
            conditions.push_str(&format!(
                " AND {alias}.type_name = $1 AND {alias}.field = ${field_param} AND {alias}.{column} {op} ${value_param} AND {alias}.{column} IS NOT NULL",
                op = f.op.as_sql()
            ));
        }
        let sql = format!(
            "SELECT e.row FROM (SELECT DISTINCT f0.entity_id FROM {from} WHERE ($2::text IS NULL OR f0.entity_id > $2){conditions} ORDER BY f0.entity_id LIMIT $3) m JOIN {s}.entities e ON e.id = m.entity_id ORDER BY e.id",
            s = self.schema
        );
        let refs: Vec<&(dyn ToSql + Sync)> = params.iter().map(|p| p.as_ref() as &(dyn ToSql + Sync)).collect();
        let rows = self.with_statement(&sql, |c, st| c.query(st, &refs))?;
        Ok(rows
            .iter()
            .filter_map(|r| if let Row::Entity(e) = json_row(r) { Some(e) } else { None })
            .collect())
    }

    /// Two queries in this snapshot: rows with a value, walking the sorted column's B-tree from the cursor, then, if the page isn't full, rows with none, in id order. Filters on other fields are semi-joins, so a list field's several values can't repeat a row.
    pub fn entities_sorted(
        &self,
        type_name: &str,
        filters: &[FieldFilter],
        sort: &FieldSort,
        after: Option<&SortAfter>,
        limit: usize,
    ) -> Result<Vec<EntityRow>, PgError> {
        let s = &self.schema;
        let column = match sort.column {
            SortColumn::Text => "text_value",
            SortColumn::Number => "number_value",
            SortColumn::Time => "time_value",
        };
        let (dir, past) = if sort.descending { ("DESC", "<") } else { ("ASC", ">") };
        let on_sorted = |f: &FieldFilter| f.field == sort.field && filter_param(&f.value).0 == column;
        // A filter on the sorted field bounds the scan of its own B-tree, rather than testing each row the scan reaches.
        let filter_sql = |params: &mut Vec<Box<dyn ToSql + Sync>>, entity: &str, sorted: bool| {
            let mut out = String::new();
            for (i, f) in filters.iter().enumerate() {
                if sorted && on_sorted(f) {
                    params.push(filter_param(&f.value).1);
                    out.push_str(&format!(" AND v.{column} {} ${}", f.op.as_sql(), params.len()));
                    continue;
                }
                params.push(Box::new(f.field.clone()));
                let field_param = params.len();
                let (col, value) = filter_param(&f.value);
                params.push(value);
                out.push_str(&format!(
                    " AND EXISTS (SELECT 1 FROM {s}.entity_fields x{i} WHERE x{i}.entity_id = {entity} AND x{i}.type_name = $1 AND x{i}.field = ${field_param} AND x{i}.{col} {op} ${value_param})",
                    op = f.op.as_sql(),
                    value_param = params.len()
                ));
            }
            out
        };
        let mut out = vec![];
        if after.is_none_or(|a| a.value.is_some()) {
            let mut params: Vec<Box<dyn ToSql + Sync>> = vec![Box::new(type_name.to_string()), Box::new(sort.field.clone()), Box::new(limit as i64)];
            let mut conditions = String::new();
            if let Some(SortAfter { value: Some(v), id }) = after {
                params.push(filter_param(v).1);
                params.push(Box::new(id.0.clone()));
                conditions.push_str(&format!(" AND (v.{column}, v.entity_id) {past} (${}, ${})", params.len() - 1, params.len()));
            }
            conditions.push_str(&filter_sql(&mut params, "v.entity_id", true));
            let sql = format!(
                "SELECT e.row FROM {s}.entity_fields v JOIN {s}.entities e ON e.id = v.entity_id WHERE v.type_name = $1 AND v.field = $2 AND v.{column} IS NOT NULL{conditions} ORDER BY v.{column} {dir}, v.entity_id {dir} LIMIT $3"
            );
            out = self.entity_rows(&sql, &params)?;
        }
        // A row with no value can't pass a filter on the sorted field.
        if out.len() < limit && !filters.iter().any(on_sorted) {
            let after_id = after.filter(|a| a.value.is_none()).map(|a| a.id.0.clone());
            let mut params: Vec<Box<dyn ToSql + Sync>> = vec![
                Box::new(type_name.to_string()),
                Box::new(sort.field.clone()),
                Box::new((limit - out.len()) as i64),
                Box::new(after_id),
            ];
            let conditions = filter_sql(&mut params, "e.id", false);
            let sql = format!(
                "SELECT e.row FROM {s}.entities e WHERE e.current AND e.type_name = $1 AND ($4::text IS NULL OR e.id {past} $4)
                 AND NOT EXISTS (SELECT 1 FROM {s}.entity_fields v WHERE v.entity_id = e.id AND v.type_name = $1 AND v.field = $2 AND v.{column} IS NOT NULL){conditions}
                 ORDER BY e.id {dir} LIMIT $3"
            );
            out.extend(self.entity_rows(&sql, &params)?);
        }
        Ok(out)
    }

    fn entity_rows(&self, sql: &str, params: &[Box<dyn ToSql + Sync>]) -> Result<Vec<EntityRow>, PgError> {
        let refs: Vec<&(dyn ToSql + Sync)> = params.iter().map(|p| p.as_ref() as &(dyn ToSql + Sync)).collect();
        let rows = self.with_statement(sql, |c, st| c.query(st, &refs))?;
        Ok(rows
            .iter()
            .filter_map(|r| if let Row::Entity(e) = json_row(r) { Some(e) } else { None })
            .collect())
    }

    pub fn entities_with_value(&self, text: &str, limit: usize) -> Result<Vec<ValueMatch>, PgError> {
        let sql = format!(
            "SELECT entity_id, type_name, field FROM {}.entity_fields WHERE text_value = $1 ORDER BY entity_id LIMIT $2",
            self.schema
        );
        let rows = self.with_statement(&sql, |c, st| c.query(st, &[&text, &(limit as i64)]))?;
        Ok(rows
            .iter()
            .map(|r| ValueMatch {
                entity_id: ObjectId(r.get(0)),
                type_name: r.get(1),
                field: r.get(2),
            })
            .collect())
    }

    pub fn shared_values(
        &self,
        type_name: &str,
        fields: &[String],
        after: Option<(&str, &str)>,
        limit: usize,
        max_ids: usize,
    ) -> Result<Vec<SharedValue>, PgError> {
        let sql = format!(
            "SELECT field, text_value, (array_agg(DISTINCT entity_id ORDER BY entity_id))[1:$6] FROM {}.entity_fields
             WHERE type_name = $1 AND field = ANY($2) AND text_value IS NOT NULL AND ($3::text IS NULL OR (field, text_value) > ($3, $4))
             GROUP BY field, text_value HAVING count(DISTINCT entity_id) > 1 ORDER BY field, text_value LIMIT $5",
            self.schema
        );
        let (after_field, after_text) = (after.map(|a| a.0), after.map(|a| a.1));
        let rows = self.with_statement(&sql, |c, st| {
            c.query(st, &[&type_name, &fields, &after_field, &after_text, &(limit as i64), &(max_ids as i32)])
        })?;
        Ok(rows
            .iter()
            .map(|r| SharedValue {
                field: r.get(0),
                text: r.get(1),
                entity_ids: r.get::<_, Vec<String>>(2).into_iter().map(ObjectId).collect(),
            })
            .collect())
    }

    /// One statement however many documents, for a page of the live board. The newest rendition of each comes with its extraction, the job behind that, and the pending ChangeSets citing it, which are counted once for all of them.
    pub fn document_facts(&self, sources: &[ObjectId]) -> Result<Vec<memex_types::DocumentFacts>, PgError> {
        let ids: Vec<&str> = sources.iter().map(|i| i.0.as_str()).collect();
        let sql = format!(
            "WITH wanted AS (SELECT ord, id FROM unnest($1::text[]) WITH ORDINALITY AS w(id, ord)),
                  latest AS (SELECT w.ord, r.id, r.row FROM wanted w
                             LEFT JOIN LATERAL (SELECT id, row FROM {s}.renditions WHERE source_id = w.id ORDER BY id DESC LIMIT 1) r ON true),
                  pending AS (SELECT rid, count(*) AS n FROM {s}.changesets c, unnest(c.rendition_ids) AS rid
                              WHERE c.state = 'pending' AND rid = ANY(SELECT id FROM latest WHERE id IS NOT NULL) GROUP BY rid)
             SELECT l.row AS rendition, x.row AS extraction, j.row AS job, COALESCE(p.n, 0) AS pending
             FROM latest l
             LEFT JOIN {s}.extractions x ON x.id = l.id
             LEFT JOIN {s}.jobs j ON j.id = x.row->'extraction'->>'job_id'
             LEFT JOIN pending p ON p.rid = l.id
             ORDER BY l.ord",
            s = self.schema
        );
        let rows = self.with_statement(&sql, |c, st| c.query(st, &[&ids]))?;
        let cell = |r: &postgres::Row, name: &str| -> Option<Row> { r.get::<_, Option<Value>>(name).and_then(|v| serde_json::from_value(v).ok()) };
        Ok(sources
            .iter()
            .zip(&rows)
            .map(|(source, r)| memex_types::DocumentFacts {
                source_id: source.clone(),
                rendition: if let Some(Row::Rendition(x)) = cell(r, "rendition") { Some(x) } else { None },
                extraction: if let Some(Row::Extraction(x)) = cell(r, "extraction") {
                    Some(x)
                } else {
                    None
                },
                extraction_job: if let Some(Row::Job(x)) = cell(r, "job") { Some(x) } else { None },
                pending_review: r.get::<_, i64>("pending") as u64,
            })
            .collect())
    }

    /// One statement, which moves no job's row and no segment's text.
    pub fn open_embed_sources(&self, limit: usize) -> Result<Vec<(ObjectId, JobState)>, PgError> {
        let sql = format!(
            "SELECT DISTINCT seg.source_id, j.state FROM {s}.jobs j
             CROSS JOIN LATERAL jsonb_array_elements(j.row->'job'->'input'->'docs') AS d(doc)
             JOIN {s}.segments seg ON seg.id = d.doc->>'id'
             WHERE j.kind = 'embed' AND j.state IN ('queued', 'running', 'dead') AND d.doc->>'kind' = 'segment'
             LIMIT $1",
            s = self.schema
        );
        let rows = self.with_statement(&sql, |c, st| c.query(st, &[&(limit as i64)]))?;
        Ok(rows
            .iter()
            .filter_map(|r| {
                let state = match r.get::<_, &str>(1) {
                    "queued" => JobState::Queued,
                    "running" => JobState::Running,
                    "dead" => JobState::Dead,
                    _ => return None,
                };
                Some((ObjectId(r.get(0)), state))
            })
            .collect())
    }

    pub fn event_time(&self, seq: u64) -> Result<Option<i64>, PgError> {
        let sql = format!("SELECT time_ms FROM {}.events WHERE seq = $1", self.schema);
        let row = self.with_statement(&sql, |c, st| c.query_opt(st, &[&(seq as i64)]))?;
        Ok(row.map(|r| r.get(0)))
    }

    /// Events of one kind, above `after`, at most `limit`. Erasure is rare, so this scans rather than keeping an index.
    pub fn events_of_kind(&self, kind: &str, after: Option<u64>, limit: usize) -> Result<Vec<Event>, PgError> {
        let from = after.map_or(-1, |a| a as i64);
        let sql = format!(
            "SELECT seq, actor, time_ms, kind, schema_version, payload FROM {}.events WHERE kind = $1 AND seq > $2 ORDER BY seq LIMIT $3",
            self.schema
        );
        let rows = self.with_statement(&sql, |c, st| c.query(st, &[&kind, &from, &(limit as i64)]))?;
        Ok(rows
            .iter()
            .map(|r| Event {
                seq: r.get::<_, i64>(0) as u64,
                vault: self.vault.clone(),
                actor: r.get(1),
                time_ms: r.get(2),
                kind: r.get(3),
                schema_version: r.get::<_, i32>(4) as u32,
                payload: r.get(5),
            })
            .collect())
    }

    /// Segments naming this source, whatever rendition they're on, in id order.
    pub fn segments_of_source(&self, source: &ObjectId) -> Result<Vec<ObjectId>, PgError> {
        let sql = format!("SELECT id FROM {}.segments WHERE source_id = $1 ORDER BY id", self.schema);
        let rows = self.with_statement(&sql, |c, st| c.query(st, &[&source.0]))?;
        Ok(rows.iter().map(|r| ObjectId(r.get(0))).collect())
    }

    /// Active edges of any kind at either end of these ids, in id order.
    pub fn edges_of(&self, ids: &[ObjectId]) -> Result<Vec<EdgeRow>, PgError> {
        let ids: Vec<&str> = ids.iter().map(|i| i.0.as_str()).collect();
        let sql = format!(
            "SELECT * FROM {}.edges WHERE status = 'active' AND (from_id = ANY($1) OR to_id = ANY($1)) ORDER BY id",
            self.schema
        );
        let rows = self.with_statement(&sql, |c, st| c.query(st, &[&ids]))?;
        Ok(rows
            .iter()
            .filter_map(|r| if let Row::Edge(x) = row_from(Table::Edge, r) { Some(x) } else { None })
            .collect())
    }

    pub fn events(&self, after: Option<u64>) -> Result<Vec<Event>, PgError> {
        let from = after.map_or(-1, |a| a as i64);
        let sql = format!(
            "SELECT seq, actor, time_ms, kind, schema_version, payload FROM {}.events WHERE seq > $1 ORDER BY seq LIMIT {PAGE_LIMIT}",
            self.schema
        );
        let rows = self.with_statement(&sql, |c, st| c.query(st, &[&from]))?;
        Ok(rows
            .iter()
            .map(|r| Event {
                seq: r.get::<_, i64>(0) as u64,
                vault: self.vault.clone(),
                actor: r.get(1),
                time_ms: r.get(2),
                kind: r.get(3),
                schema_version: r.get::<_, i32>(4) as u32,
                payload: r.get(5),
            })
            .collect())
    }
}
