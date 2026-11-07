//! The server registry in the `memex_server` schema. Each write and its audit row are one transaction.
use crate::PgStore;
use crate::store::{PgError, describe, schema_of_vault};
use memex_types::{Audit, Grant, Principal, PrincipalKind, SessionRow, TokenRow, VaultId, VaultRow};
use postgres::Row;
use postgres::error::SqlState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PgRegistryError {
    Conflict(String),
    NotFound,
    Backend(String),
}

fn map(e: postgres::Error) -> PgRegistryError {
    if e.code() == Some(&SqlState::UNIQUE_VIOLATION) {
        PgRegistryError::Conflict("that name is already taken".into())
    } else {
        PgRegistryError::Backend(describe(&e))
    }
}

fn from_store(e: PgError) -> PgRegistryError {
    PgRegistryError::Backend(e.to_string())
}

fn kind_text(k: PrincipalKind) -> &'static str {
    match k {
        PrincipalKind::Human => "human",
        PrincipalKind::Agent => "agent",
    }
}

fn principal_from(r: &Row) -> Principal {
    Principal {
        id: r.get("id"),
        name: r.get("name"),
        kind: if r.get::<_, String>("kind") == "agent" {
            PrincipalKind::Agent
        } else {
            PrincipalKind::Human
        },
        server_admin: r.get("server_admin"),
        disabled: r.get("disabled"),
        created_by: r.get("created_by"),
        created_at_ms: r.get("created_at_ms"),
    }
}

fn token_from(r: &Row) -> Result<TokenRow, PgRegistryError> {
    let grants: serde_json::Value = r.get("grants");
    let grants: Vec<Grant> = serde_json::from_value(grants).map_err(|e| PgRegistryError::Backend(e.to_string()))?;
    Ok(TokenRow {
        id: r.get("id"),
        principal_id: r.get("principal_id"),
        hash: r.get("hash"),
        name: r.get("name"),
        grants,
        created_by: r.get("created_by"),
        created_at_ms: r.get("created_at_ms"),
        expires_at_ms: r.get("expires_at_ms"),
        last_used_at_ms: r.get("last_used_at_ms"),
        revoked_at_ms: r.get("revoked_at_ms"),
    })
}

fn vault_from(r: &Row) -> VaultRow {
    VaultRow {
        id: VaultId(r.get("vault_id")),
        name: r.get("name"),
        created_by: r.get("created_by"),
        created_at_ms: r.get("created_at_ms"),
    }
}

const AUDIT_SQL: &str = "INSERT INTO memex_server.audit (actor, time_ms, action, target) VALUES ($1, $2, $3, $4)";

impl PgStore {
    fn write(&self, audit: Option<&Audit>, f: impl FnOnce(&mut postgres::Transaction<'_>) -> Result<u64, postgres::Error>) -> Result<u64, PgRegistryError> {
        let mut conn = self.take_client().map_err(from_store)?;
        let mut t = conn.client.transaction().map_err(map)?;
        let changed = f(&mut t).map_err(map)?;
        if changed == 0 {
            return Err(PgRegistryError::NotFound);
        }
        if let Some(a) = audit {
            t.execute(AUDIT_SQL, &[&a.actor, &a.time_ms, &a.action, &a.target]).map_err(map)?;
        }
        t.commit().map_err(map)?;
        self.give_client(conn);
        Ok(changed)
    }

    fn read<T>(&self, f: impl FnOnce(&mut crate::store::Conn) -> Result<T, PgRegistryError>) -> Result<T, PgRegistryError> {
        let mut conn = self.take_client().map_err(from_store)?;
        let out = f(&mut conn)?;
        self.give_client(conn);
        Ok(out)
    }

    pub fn reg_principal_count(&self) -> Result<u64, PgRegistryError> {
        self.read(|c| Ok(c.query_one("SELECT count(*) FROM memex_server.principals", &[]).map_err(map)?.get::<_, i64>(0) as u64))
    }

    pub fn reg_insert_principal(&self, p: &Principal, a: &Audit) -> Result<(), PgRegistryError> {
        self.write(Some(a), |t| {
            t.execute(
                "INSERT INTO memex_server.principals (id, name, kind, server_admin, disabled, created_by, created_at_ms) VALUES ($1, $2, $3, $4, $5, $6, $7)",
                &[
                    &p.id,
                    &p.name,
                    &kind_text(p.kind),
                    &p.server_admin,
                    &p.disabled,
                    &p.created_by,
                    &p.created_at_ms,
                ],
            )
        })
        .map(|_| ())
    }

    pub fn reg_principal(&self, id: &str) -> Result<Option<Principal>, PgRegistryError> {
        self.read(|c| {
            Ok(c.query_opt("SELECT * FROM memex_server.principals WHERE id = $1", &[&id])
                .map_err(map)?
                .as_ref()
                .map(principal_from))
        })
    }

    pub fn reg_principals(&self, after: Option<&str>, limit: usize) -> Result<Vec<Principal>, PgRegistryError> {
        self.read(|c| {
            let rows = c
                .query(
                    "SELECT * FROM memex_server.principals WHERE $1::text IS NULL OR id > $1 ORDER BY id LIMIT $2",
                    &[&after, &(limit as i64)],
                )
                .map_err(map)?;
            Ok(rows.iter().map(principal_from).collect())
        })
    }

    pub fn reg_set_disabled(&self, id: &str, disabled: bool, a: &Audit) -> Result<(), PgRegistryError> {
        self.write(Some(a), |t| {
            t.execute("UPDATE memex_server.principals SET disabled = $2 WHERE id = $1", &[&id, &disabled])
        })
        .map(|_| ())
    }

    pub fn reg_insert_token(&self, tk: &TokenRow, a: &Audit) -> Result<(), PgRegistryError> {
        let grants = serde_json::to_value(&tk.grants).map_err(|e| PgRegistryError::Backend(e.to_string()))?;
        self.write(Some(a), |t| {
            t.execute(
                "INSERT INTO memex_server.tokens (id, principal_id, hash, name, grants, created_by, created_at_ms, expires_at_ms, last_used_at_ms, revoked_at_ms) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
                &[&tk.id, &tk.principal_id, &tk.hash, &tk.name, &grants, &tk.created_by, &tk.created_at_ms, &tk.expires_at_ms, &tk.last_used_at_ms, &tk.revoked_at_ms],
            )
        })
        .map(|_| ())
    }

    pub fn reg_token_by_hash(&self, hash: &str) -> Result<Option<TokenRow>, PgRegistryError> {
        self.read(|c| {
            c.query_opt("SELECT * FROM memex_server.tokens WHERE hash = $1", &[&hash])
                .map_err(map)?
                .as_ref()
                .map(token_from)
                .transpose()
        })
    }

    pub fn reg_token(&self, id: &str) -> Result<Option<TokenRow>, PgRegistryError> {
        self.read(|c| {
            c.query_opt("SELECT * FROM memex_server.tokens WHERE id = $1", &[&id])
                .map_err(map)?
                .as_ref()
                .map(token_from)
                .transpose()
        })
    }

    pub fn reg_tokens(&self, owner: Option<&str>, after: Option<&str>, limit: usize) -> Result<Vec<TokenRow>, PgRegistryError> {
        self.read(|c| {
            let rows = c
                .query(
                    "SELECT * FROM memex_server.tokens WHERE ($1::text IS NULL OR principal_id = $1) AND ($2::text IS NULL OR id > $2) ORDER BY id LIMIT $3",
                    &[&owner, &after, &(limit as i64)],
                )
                .map_err(map)?;
            rows.iter().map(token_from).collect()
        })
    }

    pub fn reg_revoke_token(&self, id: &str, at_ms: i64, a: &Audit) -> Result<(), PgRegistryError> {
        self.write(Some(a), |t| {
            t.execute("UPDATE memex_server.tokens SET revoked_at_ms = $2 WHERE id = $1", &[&id, &at_ms])
        })
        .map(|_| ())
    }

    pub fn reg_touch_token(&self, id: &str, at_ms: i64) -> Result<(), PgRegistryError> {
        self.write(None, |t| {
            t.execute("UPDATE memex_server.tokens SET last_used_at_ms = $2 WHERE id = $1", &[&id, &at_ms])
        })
        .map(|_| ())
    }

    pub fn reg_insert_session(&self, s: &SessionRow, a: &Audit) -> Result<(), PgRegistryError> {
        self.write(Some(a), |t| {
            t.execute(
                "INSERT INTO memex_server.sessions (hash, token_id, principal_id, created_at_ms, expires_at_ms) VALUES ($1, $2, $3, $4, $5)",
                &[&s.hash, &s.token_id, &s.principal_id, &s.created_at_ms, &s.expires_at_ms],
            )
        })
        .map(|_| ())
    }

    pub fn reg_session_by_hash(&self, hash: &str) -> Result<Option<SessionRow>, PgRegistryError> {
        self.read(|c| {
            Ok(c.query_opt("SELECT * FROM memex_server.sessions WHERE hash = $1", &[&hash])
                .map_err(map)?
                .map(|r| SessionRow {
                    hash: r.get("hash"),
                    token_id: r.get("token_id"),
                    principal_id: r.get("principal_id"),
                    created_at_ms: r.get("created_at_ms"),
                    expires_at_ms: r.get("expires_at_ms"),
                }))
        })
    }

    pub fn reg_delete_session(&self, hash: &str, a: &Audit) -> Result<(), PgRegistryError> {
        self.write(Some(a), |t| t.execute("DELETE FROM memex_server.sessions WHERE hash = $1", &[&hash]))
            .map(|_| ())
    }

    pub fn reg_delete_expired_sessions(&self, now_ms: i64) -> Result<u64, PgRegistryError> {
        match self.write(None, |t| t.execute("DELETE FROM memex_server.sessions WHERE expires_at_ms <= $1", &[&now_ms])) {
            Err(PgRegistryError::NotFound) => Ok(0),
            other => other,
        }
    }

    pub fn reg_insert_vault(&self, v: &VaultRow, a: &Audit) -> Result<(), PgRegistryError> {
        self.write(Some(a), |t| {
            t.execute(
                "INSERT INTO memex_server.vault_meta (vault_id, name, created_by, created_at_ms) VALUES ($1, $2, $3, $4)",
                &[&v.id.0, &v.name, &v.created_by, &v.created_at_ms],
            )
        })
        .map(|_| ())
    }

    pub fn reg_vault(&self, id: &VaultId) -> Result<Option<VaultRow>, PgRegistryError> {
        self.read(|c| {
            Ok(c.query_opt("SELECT * FROM memex_server.vault_meta WHERE vault_id = $1", &[&id.0])
                .map_err(map)?
                .as_ref()
                .map(vault_from))
        })
    }

    pub fn reg_vaults(&self, only: Option<&[VaultId]>, after: Option<&VaultId>, limit: usize) -> Result<Vec<VaultRow>, PgRegistryError> {
        let only: Option<Vec<&str>> = only.map(|o| o.iter().map(|v| v.0.as_str()).collect());
        let after = after.map(|a| a.0.as_str());
        self.read(|c| {
            let rows = c
                .query(
                    "SELECT * FROM memex_server.vault_meta WHERE ($1::text[] IS NULL OR vault_id = ANY($1)) AND ($2::text IS NULL OR vault_id > $2) ORDER BY vault_id LIMIT $3",
                    &[&only, &after, &(limit as i64)],
                )
                .map_err(map)?;
            Ok(rows.iter().map(vault_from).collect())
        })
    }

    pub fn reg_rename_vault(&self, id: &VaultId, name: &str, a: &Audit) -> Result<(), PgRegistryError> {
        self.write(Some(a), |t| {
            t.execute("UPDATE memex_server.vault_meta SET name = $2 WHERE vault_id = $1", &[&id.0, &name])
        })
        .map(|_| ())
    }

    pub fn reg_delete_vault(&self, id: &VaultId, a: &Audit) -> Result<(), PgRegistryError> {
        self.write(Some(a), |t| t.execute("DELETE FROM memex_server.vault_meta WHERE vault_id = $1", &[&id.0]))
            .map(|_| ())
    }

    pub fn reg_audit_log(&self, limit: usize) -> Result<Vec<Audit>, PgRegistryError> {
        self.read(|c| {
            let rows = c
                .query(
                    "SELECT actor, time_ms, action, target FROM memex_server.audit ORDER BY id DESC LIMIT $1",
                    &[&(limit as i64)],
                )
                .map_err(map)?;
            Ok(rows
                .iter()
                .map(|r| Audit {
                    actor: r.get(0),
                    time_ms: r.get(1),
                    action: r.get(2),
                    target: r.get(3),
                })
                .collect())
        })
    }
}

/// Exposes the schema name for operators' tools and tests.
pub fn schema_for(vault: &VaultId) -> String {
    schema_of_vault(vault)
}
