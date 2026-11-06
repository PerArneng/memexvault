use crate::{Registry, RegistryError};
use memex_types::{Audit, Principal, SessionRow, TokenRow, VaultId, VaultRow};
use std::collections::BTreeMap;
use std::sync::Mutex;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct State {
    principals: BTreeMap<String, Principal>,
    tokens: BTreeMap<String, TokenRow>,
    sessions: BTreeMap<String, SessionRow>,
    vaults: BTreeMap<VaultId, VaultRow>,
    audit: Vec<Audit>,
}

/// An in-memory Registry with the same uniqueness rules as a real one.
#[derive(Debug, Default)]
pub struct MemRegistry {
    state: Mutex<State>,
}

impl MemRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Everything but audit entries and last-used times, so a test can show a denied request changed nothing.
    pub fn fingerprint(&self) -> String {
        let s = self.state.lock().expect("lock");
        let tokens: Vec<TokenRow> = s
            .tokens
            .values()
            .map(|t| TokenRow {
                last_used_at_ms: None,
                ..t.clone()
            })
            .collect();
        format!("{:?} {:?} {:?} {:?} {}", s.principals, tokens, s.sessions, s.vaults, s.audit.len())
    }
}

impl Registry for MemRegistry {
    fn principal_count(&self) -> Result<u64, RegistryError> {
        Ok(self.state.lock().expect("lock").principals.len() as u64)
    }

    fn insert_principal(&self, p: &Principal, audit: &Audit) -> Result<(), RegistryError> {
        let mut s = self.state.lock().expect("lock");
        if s.principals.values().any(|o| o.name == p.name) {
            return Err(RegistryError::Conflict(format!("a principal named {} exists", p.name)));
        }
        s.principals.insert(p.id.clone(), p.clone());
        s.audit.push(audit.clone());
        Ok(())
    }

    fn principal(&self, id: &str) -> Result<Option<Principal>, RegistryError> {
        Ok(self.state.lock().expect("lock").principals.get(id).cloned())
    }

    fn principals(&self, after: Option<&str>, limit: usize) -> Result<Vec<Principal>, RegistryError> {
        let s = self.state.lock().expect("lock");
        Ok(s.principals
            .values()
            .filter(|p| after.is_none_or(|a| p.id.as_str() > a))
            .take(limit)
            .cloned()
            .collect())
    }

    fn set_principal_disabled(&self, id: &str, disabled: bool, audit: &Audit) -> Result<(), RegistryError> {
        let mut s = self.state.lock().expect("lock");
        s.principals.get_mut(id).ok_or(RegistryError::NotFound)?.disabled = disabled;
        s.audit.push(audit.clone());
        Ok(())
    }

    fn insert_token(&self, t: &TokenRow, audit: &Audit) -> Result<(), RegistryError> {
        let mut s = self.state.lock().expect("lock");
        s.tokens.insert(t.id.clone(), t.clone());
        s.audit.push(audit.clone());
        Ok(())
    }

    fn token_by_hash(&self, hash: &str) -> Result<Option<TokenRow>, RegistryError> {
        Ok(self.state.lock().expect("lock").tokens.values().find(|t| t.hash == hash).cloned())
    }

    fn token(&self, id: &str) -> Result<Option<TokenRow>, RegistryError> {
        Ok(self.state.lock().expect("lock").tokens.get(id).cloned())
    }

    fn tokens(&self, owner: Option<&str>, after: Option<&str>, limit: usize) -> Result<Vec<TokenRow>, RegistryError> {
        let s = self.state.lock().expect("lock");
        let it = s
            .tokens
            .values()
            .filter(|t| owner.is_none_or(|o| t.principal_id == o) && after.is_none_or(|a| t.id.as_str() > a));
        Ok(it.take(limit).cloned().collect())
    }

    fn revoke_token(&self, id: &str, at_ms: i64, audit: &Audit) -> Result<(), RegistryError> {
        let mut s = self.state.lock().expect("lock");
        s.tokens.get_mut(id).ok_or(RegistryError::NotFound)?.revoked_at_ms = Some(at_ms);
        s.audit.push(audit.clone());
        Ok(())
    }

    fn touch_token(&self, id: &str, at_ms: i64) -> Result<(), RegistryError> {
        let mut s = self.state.lock().expect("lock");
        s.tokens.get_mut(id).ok_or(RegistryError::NotFound)?.last_used_at_ms = Some(at_ms);
        Ok(())
    }

    fn insert_session(&self, session: &SessionRow, audit: &Audit) -> Result<(), RegistryError> {
        let mut s = self.state.lock().expect("lock");
        s.sessions.insert(session.hash.clone(), session.clone());
        s.audit.push(audit.clone());
        Ok(())
    }

    fn session_by_hash(&self, hash: &str) -> Result<Option<SessionRow>, RegistryError> {
        Ok(self.state.lock().expect("lock").sessions.get(hash).cloned())
    }

    fn delete_session(&self, hash: &str, audit: &Audit) -> Result<(), RegistryError> {
        let mut s = self.state.lock().expect("lock");
        s.sessions.remove(hash).ok_or(RegistryError::NotFound)?;
        s.audit.push(audit.clone());
        Ok(())
    }

    fn delete_expired_sessions(&self, now_ms: i64) -> Result<u64, RegistryError> {
        let mut s = self.state.lock().expect("lock");
        let before = s.sessions.len();
        s.sessions.retain(|_, row| row.expires_at_ms > now_ms);
        Ok((before - s.sessions.len()) as u64)
    }

    fn insert_vault(&self, v: &VaultRow, audit: &Audit) -> Result<(), RegistryError> {
        let mut s = self.state.lock().expect("lock");
        if s.vaults.values().any(|o| o.name == v.name) {
            return Err(RegistryError::Conflict(format!("a vault named {} exists", v.name)));
        }
        s.vaults.insert(v.id.clone(), v.clone());
        s.audit.push(audit.clone());
        Ok(())
    }

    fn vault(&self, id: &VaultId) -> Result<Option<VaultRow>, RegistryError> {
        Ok(self.state.lock().expect("lock").vaults.get(id).cloned())
    }

    fn vaults(&self, only: Option<&[VaultId]>, after: Option<&VaultId>, limit: usize) -> Result<Vec<VaultRow>, RegistryError> {
        let s = self.state.lock().expect("lock");
        let it = s
            .vaults
            .values()
            .filter(|v| only.is_none_or(|o| o.contains(&v.id)) && after.is_none_or(|a| &v.id > a));
        Ok(it.take(limit).cloned().collect())
    }

    fn rename_vault(&self, id: &VaultId, name: &str, audit: &Audit) -> Result<(), RegistryError> {
        let mut s = self.state.lock().expect("lock");
        if s.vaults.values().any(|o| o.name == name && &o.id != id) {
            return Err(RegistryError::Conflict(format!("a vault named {name} exists")));
        }
        s.vaults.get_mut(id).ok_or(RegistryError::NotFound)?.name = name.into();
        s.audit.push(audit.clone());
        Ok(())
    }

    fn delete_vault(&self, id: &VaultId, audit: &Audit) -> Result<(), RegistryError> {
        let mut s = self.state.lock().expect("lock");
        s.vaults.remove(id).ok_or(RegistryError::NotFound)?;
        s.audit.push(audit.clone());
        Ok(())
    }

    fn audit_log(&self, limit: usize) -> Result<Vec<Audit>, RegistryError> {
        Ok(self.state.lock().expect("lock").audit.iter().rev().take(limit).cloned().collect())
    }
}
