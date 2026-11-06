use memex_types::{Audit, Principal, TokenRow, VaultId, VaultRow};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    /// A unique name is already taken.
    Conflict(String),
    NotFound,
    Backend(String),
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict(m) => write!(f, "conflict: {m}"),
            Self::NotFound => f.write_str("not found"),
            Self::Backend(m) => write!(f, "registry backend: {m}"),
        }
    }
}

impl std::error::Error for RegistryError {}

/// Server-wide state, separate from any vault. Every write carries an audit entry, stored atomically with it, so no write lacks an actor.
pub trait Registry {
    fn principal_count(&self) -> Result<u64, RegistryError>;
    fn insert_principal(&self, p: &Principal, audit: &Audit) -> Result<(), RegistryError>;
    fn principal(&self, id: &str) -> Result<Option<Principal>, RegistryError>;
    /// In id order, after the given id, at most `limit`.
    fn principals(&self, after: Option<&str>, limit: usize) -> Result<Vec<Principal>, RegistryError>;
    fn set_principal_disabled(&self, id: &str, disabled: bool, audit: &Audit) -> Result<(), RegistryError>;

    fn insert_token(&self, t: &TokenRow, audit: &Audit) -> Result<(), RegistryError>;
    fn token_by_hash(&self, hash: &str) -> Result<Option<TokenRow>, RegistryError>;
    fn token(&self, id: &str) -> Result<Option<TokenRow>, RegistryError>;
    /// In id order. `owner` limits it to one principal's tokens.
    fn tokens(&self, owner: Option<&str>, after: Option<&str>, limit: usize) -> Result<Vec<TokenRow>, RegistryError>;
    fn revoke_token(&self, id: &str, at_ms: i64, audit: &Audit) -> Result<(), RegistryError>;
    /// Not audited: it records use, and doesn't change what anyone can do.
    fn touch_token(&self, id: &str, at_ms: i64) -> Result<(), RegistryError>;

    fn insert_vault(&self, v: &VaultRow, audit: &Audit) -> Result<(), RegistryError>;
    fn vault(&self, id: &VaultId) -> Result<Option<VaultRow>, RegistryError>;
    /// In id order. `only` limits it to the given vaults.
    fn vaults(&self, only: Option<&[VaultId]>, after: Option<&VaultId>, limit: usize) -> Result<Vec<VaultRow>, RegistryError>;
    fn rename_vault(&self, id: &VaultId, name: &str, audit: &Audit) -> Result<(), RegistryError>;
    fn delete_vault(&self, id: &VaultId, audit: &Audit) -> Result<(), RegistryError>;

    /// Newest first, for tests and for the console's history.
    fn audit_log(&self, limit: usize) -> Result<Vec<Audit>, RegistryError>;
}
