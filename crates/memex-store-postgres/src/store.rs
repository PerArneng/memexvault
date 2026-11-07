use crate::migrate::{LATEST_SERVER_VERSION, LATEST_VAULT_VERSION, SERVER_MIGRATIONS, VAULT_MIGRATIONS};
use crate::net::net_mutations;
use crate::tls::Target;
use memex_types::{
    AssertionRow, BlockKind, BlockRow, ChangeSetRow, DocKind, EdgeDirection, EdgeRow, EntityRow, EntityTypeRow, Event, EvidenceRow, FieldFilter, FilterValue, Header, IdDigest,
    JobRow, JobState, Mutation, ObjectId, PageRow, RenditionRow, RevisionRow, Row, RowKey, SegmentRow, SharedValue, SourceRow, Status, Table, ValueMatch,
    VaultId,
};
use postgres::{Client, Statement, types::ToSql};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

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
}

impl Conn {
    fn new(client: Client) -> Self {
        Self {
            client,
            statements: HashMap::new(),
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

struct Writer {
    token: u64,
    schema: String,
    conn: Conn,
}

/// Proof of holding a vault's advisory lock, on the writer's own connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PgLease {
    pub vault: VaultId,
    pub token: u64,
}

pub struct PgStore {
    target: Target,
    pool: Mutex<Vec<Conn>>,
    pool_max: usize,
    writers: Mutex<HashMap<VaultId, Arc<Mutex<Writer>>>>,
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
    pub fn connect(url: &str, pool_max: usize) -> Result<Self, PgError> {
        let store = Self {
            target: Target::parse(url)?,
            pool: Mutex::new(vec![]),
            pool_max,
            writers: Mutex::new(HashMap::new()),
            schemas: Mutex::new(HashMap::new()),
            next_token: AtomicU64::new(1),
        };
        store.new_client()?;
        Ok(store)
    }

    fn new_client(&self) -> Result<Client, PgError> {
        self.target.connect().map_err(backend)
    }

    pub fn take_conn(&self) -> Result<Conn, PgError> {
        let pooled = self.pool.lock().expect("lock").pop();
        match pooled {
            Some(c) if !c.client.is_closed() => Ok(c),
            _ => self.new_client().map(Conn::new),
        }
    }

    /// Returns a connection to the pool. It must have no open transaction.
    pub fn give_conn(&self, conn: Conn) {
        self.give_client(conn);
    }

    pub(crate) fn take_client(&self) -> Result<Conn, PgError> {
        self.take_conn()
    }

    pub(crate) fn give_client(&self, conn: Conn) {
        let mut pool = self.pool.lock().expect("lock");
        if pool.len() < self.pool_max && !conn.client.is_closed() {
            pool.push(conn);
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

    /// Takes the session advisory lock on a dedicated connection, creating the vault if it's new. Losing that connection loses the lease.
    pub fn open_vault(&self, vault: &VaultId) -> Result<PgLease, PgError> {
        let mut client = self.new_client()?;
        let got: bool = client
            .query_one("SELECT pg_try_advisory_lock($1)", &[&lock_key(vault)])
            .map_err(backend)?
            .get(0);
        if !got {
            return Err(PgError::WriterActive);
        }
        let schema = schema_of_vault(vault);
        client
            .execute(
                "INSERT INTO memex_server.vaults (vault_id, schema_name) VALUES ($1, $2) ON CONFLICT (vault_id) DO NOTHING",
                &[&vault.0, &schema],
            )
            .map_err(backend)?;
        migrate_vault(&mut client, schema.clone())?;
        let token = self.next_token.fetch_add(1, Ordering::Relaxed);
        self.writers.lock().expect("lock").insert(
            vault.clone(),
            Arc::new(Mutex::new(Writer {
                token,
                schema,
                conn: Conn::new(client),
            })),
        );
        Ok(PgLease { vault: vault.clone(), token })
    }

    fn writer(&self, lease: &PgLease) -> Result<Arc<Mutex<Writer>>, PgError> {
        let writer = self.writers.lock().expect("lock").get(&lease.vault).cloned().ok_or(PgError::LeaseLost)?;
        {
            let w = writer.lock().expect("lock");
            if w.token != lease.token || w.conn.client.is_closed() {
                return Err(PgError::LeaseLost);
            }
        }
        Ok(writer)
    }

    /// Ends the writer connection, which releases the advisory lock.
    pub fn close_vault(&self, lease: &PgLease) -> Result<(), PgError> {
        self.writer(lease)?;
        self.writers.lock().expect("lock").remove(&lease.vault);
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
        let mut guard = writer.lock().expect("lock");
        let Writer { schema, conn, .. } = &mut *guard;
        let sql = format!("UPDATE {schema}.cache_generation SET generation = generation + 1 RETURNING generation");
        let generation: i64 = conn.client.query_one(&sql, &[]).map_err(backend)?.get(0);
        Ok(generation as u64)
    }

    /// One SQL transaction for the whole batch. `batch` holds each event with its mutations. Durable when it returns.
    pub fn commit_batch(&self, lease: &PgLease, batch: &[(&Event, &[Mutation])]) -> Result<u64, PgError> {
        let last = batch.last().ok_or(PgError::EmptyBatch)?.0.seq;
        let writer = self.writer(lease)?;
        let mut guard = writer.lock().expect("lock");
        let Writer { schema, conn, .. } = &mut *guard;
        let result = commit_in_transaction(conn, schema, batch);
        if conn.client.is_closed() {
            return Err(PgError::LeaseLost);
        }
        result.map(|()| last)
    }

    /// Drops the vault's schema and ends its lease.
    pub fn delete_vault(&self, lease: &PgLease) -> Result<(), PgError> {
        self.writer(lease)?;
        self.drop_vault(&lease.vault)?;
        self.writers.lock().expect("lock").remove(&lease.vault);
        Ok(())
    }

    /// Physically reclaims deleted rows, for erasure.
    pub fn compact(&self, lease: &PgLease) -> Result<(), PgError> {
        let writer = self.writer(lease)?;
        let mut guard = writer.lock().expect("lock");
        let Writer { schema, conn, .. } = &mut *guard;
        let client = &mut conn.client;
        for t in [
            "events",
            "sources",
            "pages",
            "edges",
            "segments",
            "assertions",
            "evidence",
            "changesets",
            "jobs",
            "entity_types",
            "entities",
            "entity_fields",
            "blocks",
            "page_slugs",
            "page_revisions",
            "review_verdicts",
            "auto_commit",
            "audit_runs",
            "audit_checks",
            "audit_state",
            "audit_planted",
        ] {
            client.batch_execute(&format!("VACUUM {schema}.{t}")).map_err(backend)?;
        }
        Ok(())
    }

    /// Removes a vault's schema entirely. For tests and vault deletion.
    pub fn drop_vault(&self, vault: &VaultId) -> Result<(), PgError> {
        let mut conn = self.take_client()?;
        let c = &mut conn.client;
        let schema = schema_of_vault(vault);
        c.batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE")).map_err(backend)?;
        c.execute("DELETE FROM memex_server.vaults WHERE vault_id = $1", &[&vault.0]).map_err(backend)?;
        self.schemas.lock().expect("lock").remove(vault);
        self.give_client(conn);
        Ok(())
    }
}

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

fn commit_in_transaction(conn: &mut Conn, schema: &str, batch: &[(&Event, &[Mutation])]) -> Result<(), PgError> {
    let Conn { client, statements: cache } = conn;
    let mut t = client.transaction().map_err(backend)?;
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
        let sql = format!("SELECT count(*), coalesce(bit_xor(('x' || encode(substring(sha256(a.id::bytea) from 1 for 8), 'hex'))::bit(64)::bigint), 0) FROM {from}");
        let row = self.with_statement(&sql, |c, st| c.query_one(st, &[]))?;
        Ok(IdDigest {
            count: row.get::<_, i64>(0) as u64,
            xor: row.get(1),
        })
    }

    pub fn audit_checks(&self, run: &ObjectId, findings_only: bool, after: Option<&ObjectId>, limit: usize) -> Result<Vec<memex_types::AuditCheckRow>, PgError> {
        let sql = format!(
            "SELECT row FROM {}.audit_checks WHERE run_id = $1 AND ($2::text IS NULL OR id > $2) AND (NOT $3 OR outcome IN ('suspect', 'failed')) ORDER BY id LIMIT $4",
            self.schema
        );
        let rows = self.with_statement(&sql, |c, st| c.query(st, &[&run.0, &after.map(|a| a.0.as_str()), &findings_only, &(limit as i64)]))?;
        Ok(rows.iter().filter_map(|r| if let Row::AuditCheck(x) = json_row(r) { Some(x) } else { None }).collect())
    }

    pub fn claim_counted_correct(&self, claim: &ObjectId) -> Result<bool, PgError> {
        let sql = format!("SELECT EXISTS (SELECT 1 FROM {}.audit_checks WHERE claim_id = $1 AND (row->'audit_check'->>'counted_correct')::boolean)", self.schema);
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
