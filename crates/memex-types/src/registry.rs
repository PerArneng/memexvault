use crate::VaultId;
use serde::{Deserialize, Serialize};

/// What a token may do in one vault. Each level includes the one before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Read,
    Propose,
    Admin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    pub vault: VaultId,
    pub scope: Scope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalKind {
    Human,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    pub id: String,
    pub name: String,
    pub kind: PrincipalKind,
    pub server_admin: bool,
    pub disabled: bool,
    pub created_by: String,
    pub created_at_ms: i64,
}

/// A token as stored. The token itself is never stored, only its hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenRow {
    pub id: String,
    pub principal_id: String,
    pub hash: String,
    pub name: String,
    pub grants: Vec<Grant>,
    pub created_by: String,
    pub created_at_ms: i64,
    pub expires_at_ms: Option<i64>,
    pub last_used_at_ms: Option<i64>,
    pub revoked_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultRow {
    pub id: VaultId,
    pub name: String,
    pub created_by: String,
    pub created_at_ms: i64,
}

/// Who changed the registry, written with each change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Audit {
    pub actor: String,
    pub time_ms: i64,
    pub action: String,
    pub target: String,
}
