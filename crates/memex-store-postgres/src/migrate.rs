//! Versioned SQL, embedded in the binary. `{s}` stands for the vault's schema name.
pub const SERVER_MIGRATIONS: &[&str] = &[
    "CREATE TABLE memex_server.vaults (vault_id text PRIMARY KEY, schema_name text NOT NULL UNIQUE, created_at timestamptz NOT NULL DEFAULT now());",
    "
    CREATE TABLE memex_server.vault_meta (
        vault_id text COLLATE \"C\" PRIMARY KEY,
        name text NOT NULL UNIQUE,
        created_by text NOT NULL,
        created_at_ms bigint NOT NULL
    );
    CREATE TABLE memex_server.principals (
        id text COLLATE \"C\" PRIMARY KEY,
        name text NOT NULL UNIQUE,
        kind text NOT NULL,
        server_admin boolean NOT NULL,
        disabled boolean NOT NULL,
        created_by text NOT NULL,
        created_at_ms bigint NOT NULL
    );
    CREATE TABLE memex_server.tokens (
        id text COLLATE \"C\" PRIMARY KEY,
        principal_id text NOT NULL REFERENCES memex_server.principals,
        hash text NOT NULL UNIQUE,
        name text NOT NULL,
        grants jsonb NOT NULL,
        created_by text NOT NULL,
        created_at_ms bigint NOT NULL,
        expires_at_ms bigint,
        last_used_at_ms bigint,
        revoked_at_ms bigint
    );
    CREATE INDEX ON memex_server.tokens (principal_id);
    CREATE TABLE memex_server.audit (
        id bigserial PRIMARY KEY,
        actor text NOT NULL,
        time_ms bigint NOT NULL,
        action text NOT NULL,
        target text NOT NULL
    );
    ",
];

pub const VAULT_MIGRATIONS: &[&str] = &[
    "
    CREATE TABLE {s}.events (
        seq bigint PRIMARY KEY,
        actor text NOT NULL,
        time_ms bigint NOT NULL,
        kind text NOT NULL,
        schema_version integer NOT NULL,
        payload jsonb NOT NULL
    );
    CREATE TABLE {s}.sources (
        id text COLLATE \"C\" PRIMARY KEY,
        creator text NOT NULL,
        status text NOT NULL,
        version integer NOT NULL,
        acl_id text,
        sha256 text NOT NULL,
        media_type text NOT NULL,
        trust_tier text NOT NULL,
        recorded_at_ms bigint NOT NULL
    );
    CREATE INDEX ON {s}.sources (sha256);
    CREATE TABLE {s}.pages (
        id text COLLATE \"C\" PRIMARY KEY,
        creator text NOT NULL,
        status text NOT NULL,
        version integer NOT NULL,
        acl_id text,
        title text NOT NULL,
        body text NOT NULL,
        updated_at_ms bigint NOT NULL
    );
    CREATE TABLE {s}.edges (
        id text COLLATE \"C\" PRIMARY KEY,
        creator text NOT NULL,
        status text NOT NULL,
        version integer NOT NULL,
        acl_id text,
        from_id text NOT NULL,
        to_id text NOT NULL,
        kind text NOT NULL
    );
    CREATE INDEX ON {s}.edges (from_id);
    CREATE INDEX ON {s}.edges (to_id);
",
    "
    CREATE TABLE {s}.segments (
        id text COLLATE \"C\" PRIMARY KEY,
        row jsonb NOT NULL,
        source_id text NOT NULL,
        text text NOT NULL
    );
    CREATE INDEX ON {s}.segments (source_id);
    CREATE TABLE {s}.assertions (
        id text COLLATE \"C\" PRIMARY KEY,
        row jsonb NOT NULL,
        subject text NOT NULL,
        predicate text NOT NULL,
        canonical_hash text NOT NULL,
        active boolean NOT NULL
    );
    CREATE INDEX ON {s}.assertions (canonical_hash) WHERE active;
    CREATE INDEX ON {s}.assertions (subject, predicate) WHERE active;
    CREATE TABLE {s}.evidence (
        id text COLLATE \"C\" PRIMARY KEY,
        row jsonb NOT NULL,
        assertion_id text NOT NULL,
        segment_id text NOT NULL
    );
    CREATE INDEX ON {s}.evidence (assertion_id);
    CREATE INDEX ON {s}.evidence (segment_id);
    CREATE TABLE {s}.changesets (
        id text COLLATE \"C\" PRIMARY KEY,
        row jsonb NOT NULL,
        state text NOT NULL,
        idempotency_key text NOT NULL UNIQUE,
        proposer text NOT NULL
    );
    CREATE INDEX ON {s}.changesets (state);
",
    "
    ALTER TABLE {s}.sources
        ADD COLUMN name text NOT NULL DEFAULT '',
        ADD COLUMN size bigint NOT NULL DEFAULT 0,
        ADD COLUMN logical_id text COLLATE \"C\",
        ADD COLUMN version_no integer NOT NULL DEFAULT 1,
        ADD COLUMN metadata jsonb NOT NULL DEFAULT '{}';
    UPDATE {s}.sources SET logical_id = id;
    ALTER TABLE {s}.sources ALTER COLUMN logical_id SET NOT NULL;
    CREATE INDEX ON {s}.sources (logical_id);
    CREATE TABLE {s}.jobs (
        id text COLLATE \"C\" PRIMARY KEY,
        row jsonb NOT NULL,
        kind text NOT NULL,
        state text NOT NULL,
        dedupe_key text NOT NULL UNIQUE,
        next_run_at_ms bigint NOT NULL,
        lease_until_ms bigint
    );
    CREATE INDEX ON {s}.jobs (kind, state, next_run_at_ms);
    CREATE INDEX ON {s}.jobs (state);
",
    "
    CREATE TABLE {s}.renditions (
        id text COLLATE \"C\" PRIMARY KEY,
        row jsonb NOT NULL,
        source_id text NOT NULL
    );
    CREATE INDEX ON {s}.renditions (source_id);
    ALTER TABLE {s}.segments ADD COLUMN rendition_id text COLLATE \"C\";
    CREATE INDEX ON {s}.segments (rendition_id, id);
    ALTER TABLE {s}.evidence ADD COLUMN anchor_broken boolean NOT NULL DEFAULT false;
    CREATE INDEX ON {s}.evidence (id) WHERE anchor_broken;
",
];

pub const LATEST_SERVER_VERSION: i32 = SERVER_MIGRATIONS.len() as i32;
pub const LATEST_VAULT_VERSION: i32 = VAULT_MIGRATIONS.len() as i32;
