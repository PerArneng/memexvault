//! The Registry on PostgreSQL behind the core trait. Same connection pool as the store. It only translates.
use memex_core::{Registry, RegistryError};
use memex_store_postgres::{PgRegistryError, PgStore};
use memex_types::{Audit, Principal, TokenRow, VaultId, VaultRow};
use std::sync::Arc;

pub struct PostgresRegistry(pub Arc<PgStore>);

fn e(x: PgRegistryError) -> RegistryError {
    match x {
        PgRegistryError::Conflict(m) => RegistryError::Conflict(m),
        PgRegistryError::NotFound => RegistryError::NotFound,
        PgRegistryError::Backend(m) => RegistryError::Backend(m),
    }
}

impl Registry for PostgresRegistry {
    fn principal_count(&self) -> Result<u64, RegistryError> {
        self.0.reg_principal_count().map_err(e)
    }
    fn insert_principal(&self, p: &Principal, a: &Audit) -> Result<(), RegistryError> {
        self.0.reg_insert_principal(p, a).map_err(e)
    }
    fn principal(&self, id: &str) -> Result<Option<Principal>, RegistryError> {
        self.0.reg_principal(id).map_err(e)
    }
    fn principals(&self, after: Option<&str>, limit: usize) -> Result<Vec<Principal>, RegistryError> {
        self.0.reg_principals(after, limit).map_err(e)
    }
    fn set_principal_disabled(&self, id: &str, disabled: bool, a: &Audit) -> Result<(), RegistryError> {
        self.0.reg_set_disabled(id, disabled, a).map_err(e)
    }
    fn insert_token(&self, t: &TokenRow, a: &Audit) -> Result<(), RegistryError> {
        self.0.reg_insert_token(t, a).map_err(e)
    }
    fn token_by_hash(&self, hash: &str) -> Result<Option<TokenRow>, RegistryError> {
        self.0.reg_token_by_hash(hash).map_err(e)
    }
    fn token(&self, id: &str) -> Result<Option<TokenRow>, RegistryError> {
        self.0.reg_token(id).map_err(e)
    }
    fn tokens(&self, owner: Option<&str>, after: Option<&str>, limit: usize) -> Result<Vec<TokenRow>, RegistryError> {
        self.0.reg_tokens(owner, after, limit).map_err(e)
    }
    fn revoke_token(&self, id: &str, at: i64, a: &Audit) -> Result<(), RegistryError> {
        self.0.reg_revoke_token(id, at, a).map_err(e)
    }
    fn touch_token(&self, id: &str, at: i64) -> Result<(), RegistryError> {
        self.0.reg_touch_token(id, at).map_err(e)
    }
    fn insert_vault(&self, v: &VaultRow, a: &Audit) -> Result<(), RegistryError> {
        self.0.reg_insert_vault(v, a).map_err(e)
    }
    fn vault(&self, id: &VaultId) -> Result<Option<VaultRow>, RegistryError> {
        self.0.reg_vault(id).map_err(e)
    }
    fn vaults(&self, only: Option<&[VaultId]>, after: Option<&VaultId>, limit: usize) -> Result<Vec<VaultRow>, RegistryError> {
        self.0.reg_vaults(only, after, limit).map_err(e)
    }
    fn rename_vault(&self, id: &VaultId, name: &str, a: &Audit) -> Result<(), RegistryError> {
        self.0.reg_rename_vault(id, name, a).map_err(e)
    }
    fn delete_vault(&self, id: &VaultId, a: &Audit) -> Result<(), RegistryError> {
        self.0.reg_delete_vault(id, a).map_err(e)
    }
    fn audit_log(&self, limit: usize) -> Result<Vec<Audit>, RegistryError> {
        self.0.reg_audit_log(limit).map_err(e)
    }
}
