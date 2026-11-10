use memex_types::{Grant, Principal, Scope, VaultId};

/// An authenticated request's identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    pub principal: Principal,
    pub token_id: String,
    pub grants: Vec<Grant>,
    /// It signed in through the console, rather than sending its token. An agent can't decide review items this way.
    pub via_session: bool,
}

/// Every operation that needs a decision. Health, version and whoami need only a caller or nothing, so they aren't here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation<'a> {
    VaultCreate,
    /// Everyone may list, and sees only their own vaults.
    VaultList,
    VaultRead(&'a VaultId),
    VaultRename(&'a VaultId),
    VaultDelete(&'a VaultId),
    /// Whether a vault's content may go to the LLM, and what it may spend.
    VaultChatSettings(&'a VaultId),
    PrincipalList,
    PrincipalCreate,
    PrincipalDisable,
    /// Counters and sizes of the caches, which hold nothing from a vault.
    CacheStats,
    TokenCreate {
        grants: &'a [Grant],
    },
    /// Everyone may list their own. Listing others' needs server admin.
    TokenList {
        all: bool,
    },
    /// The token's owner, or a server admin.
    TokenRevoke {
        owner: &'a str,
    },
    /// Content in a vault, as later specs add it.
    VaultContentRead(&'a VaultId),
    VaultContentPropose(&'a VaultId),
    /// Approving, rejecting and reverting ChangeSets.
    VaultContentReview(&'a VaultId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// The caller may know the object exists, but can't do this.
    Forbidden,
    /// The caller has no access to this vault, so must not learn whether it exists.
    Hidden,
}

fn scope_in(caller: &Caller, vault: &VaultId) -> Option<Scope> {
    caller.grants.iter().filter(|g| &g.vault == vault).map(|g| g.scope).max()
}

fn vault_needs(caller: &Caller, vault: &VaultId, need: Scope) -> Decision {
    if caller.principal.server_admin {
        return Decision::Allow;
    }
    match scope_in(caller, vault) {
        None => Decision::Hidden,
        Some(have) if have >= need => Decision::Allow,
        Some(_) => Decision::Forbidden,
    }
}

fn server_admin_only(caller: &Caller) -> Decision {
    if caller.principal.server_admin {
        Decision::Allow
    } else {
        Decision::Forbidden
    }
}

/// Pure: a function of the caller and the operation. A disabled principal never reaches here, because authentication refuses it.
pub fn authorize(caller: &Caller, op: Operation<'_>) -> Decision {
    match op {
        Operation::VaultCreate | Operation::PrincipalList | Operation::PrincipalCreate | Operation::PrincipalDisable | Operation::CacheStats => {
            server_admin_only(caller)
        }
        Operation::VaultList => Decision::Allow,
        Operation::VaultRead(v) | Operation::VaultContentRead(v) => vault_needs(caller, v, Scope::Read),
        Operation::VaultContentPropose(v) => vault_needs(caller, v, Scope::Propose),
        Operation::VaultRename(v) | Operation::VaultDelete(v) | Operation::VaultChatSettings(v) | Operation::VaultContentReview(v) => {
            vault_needs(caller, v, Scope::Admin)
        }
        // A vault admin can hand out access to that vault, and no one can grant what they don't hold.
        Operation::TokenCreate { grants } => {
            if caller.principal.server_admin {
                return Decision::Allow;
            }
            let mut decision = if grants.is_empty() { Decision::Forbidden } else { Decision::Allow };
            for g in grants {
                match vault_needs(caller, &g.vault, Scope::Admin) {
                    Decision::Allow => {}
                    Decision::Hidden => return Decision::Hidden,
                    Decision::Forbidden => decision = Decision::Forbidden,
                }
            }
            decision
        }
        Operation::TokenList { all } => {
            if all {
                server_admin_only(caller)
            } else {
                Decision::Allow
            }
        }
        Operation::TokenRevoke { owner } => {
            if owner == caller.principal.id || caller.principal.server_admin {
                Decision::Allow
            } else {
                Decision::Hidden
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use memex_types::PrincipalKind;

    fn vault(n: &str) -> VaultId {
        VaultId(n.into())
    }

    fn caller(admin: bool, grants: &[(&str, Scope)]) -> Caller {
        Caller {
            principal: Principal {
                id: "me".into(),
                name: "me".into(),
                kind: PrincipalKind::Human,
                server_admin: admin,
                disabled: false,
                created_by: "x".into(),
                created_at_ms: 0,
            },
            token_id: "t".into(),
            grants: grants.iter().map(|(v, s)| Grant { vault: vault(v), scope: *s }).collect(),
            via_session: false,
        }
    }

    use Decision::{Allow, Forbidden, Hidden};
    use Scope::{Admin, Propose, Read};

    #[test]
    fn scopes_against_vault_operations_for_own_and_other_vault() {
        let (a, b) = (vault("a"), vault("b"));
        // scope in vault a, operation builder, expected on vault a, expected on vault b
        type Op = for<'x> fn(&'x VaultId) -> Operation<'x>;
        let cases: &[(Scope, Op, Decision)] = &[
            (Read, |v| Operation::VaultRead(v), Allow),
            (Read, |v| Operation::VaultContentRead(v), Allow),
            (Read, |v| Operation::VaultContentPropose(v), Forbidden),
            (Read, |v| Operation::VaultContentReview(v), Forbidden),
            (Propose, |v| Operation::VaultContentReview(v), Forbidden),
            (Admin, |v| Operation::VaultContentReview(v), Allow),
            (Read, |v| Operation::VaultRename(v), Forbidden),
            (Read, |v| Operation::VaultDelete(v), Forbidden),
            (Propose, |v| Operation::VaultRead(v), Allow),
            (Propose, |v| Operation::VaultContentRead(v), Allow),
            (Propose, |v| Operation::VaultContentPropose(v), Allow),
            (Propose, |v| Operation::VaultRename(v), Forbidden),
            (Propose, |v| Operation::VaultDelete(v), Forbidden),
            (Admin, |v| Operation::VaultRead(v), Allow),
            (Admin, |v| Operation::VaultContentRead(v), Allow),
            (Admin, |v| Operation::VaultContentPropose(v), Allow),
            (Admin, |v| Operation::VaultRename(v), Allow),
            (Admin, |v| Operation::VaultDelete(v), Allow),
        ];
        for (scope, op, own) in cases {
            let c = caller(false, &[("a", *scope)]);
            assert_eq!(authorize(&c, op(&a)), *own, "{scope:?} own {:?}", op(&a));
            assert_eq!(authorize(&c, op(&b)), Hidden, "{scope:?} other {:?}", op(&b));
        }
    }

    #[test]
    fn server_admin_can_do_all_of_it_with_no_grants() {
        let c = caller(true, &[]);
        let v = vault("a");
        let grants = [Grant {
            vault: v.clone(),
            scope: Admin,
        }];
        for op in [
            Operation::VaultCreate,
            Operation::VaultList,
            Operation::VaultRead(&v),
            Operation::VaultRename(&v),
            Operation::VaultDelete(&v),
            Operation::PrincipalList,
            Operation::PrincipalCreate,
            Operation::PrincipalDisable,
            Operation::TokenCreate { grants: &grants },
            Operation::TokenList { all: true },
            Operation::TokenRevoke { owner: "someone" },
            Operation::VaultContentPropose(&v),
        ] {
            assert_eq!(authorize(&c, op), Allow, "{op:?}");
        }
    }

    #[test]
    fn server_level_operations_are_forbidden_to_everyone_else() {
        let c = caller(false, &[("a", Admin)]);
        for op in [
            Operation::VaultCreate,
            Operation::PrincipalList,
            Operation::PrincipalCreate,
            Operation::PrincipalDisable,
            Operation::TokenList { all: true },
        ] {
            assert_eq!(authorize(&c, op), Forbidden, "{op:?}");
        }
        assert_eq!(authorize(&c, Operation::VaultList), Allow);
        assert_eq!(authorize(&c, Operation::TokenList { all: false }), Allow);
    }

    #[test]
    fn nobody_grants_what_they_do_not_administer() {
        let c = caller(false, &[("a", Admin), ("b", Read)]);
        let grant = |v: &str, s| [Grant { vault: vault(v), scope: s }];
        assert_eq!(authorize(&c, Operation::TokenCreate { grants: &grant("a", Read) }), Allow);
        assert_eq!(authorize(&c, Operation::TokenCreate { grants: &grant("a", Admin) }), Allow);
        assert_eq!(authorize(&c, Operation::TokenCreate { grants: &grant("b", Read) }), Forbidden);
        assert_eq!(authorize(&c, Operation::TokenCreate { grants: &grant("c", Read) }), Hidden);
        assert_eq!(authorize(&c, Operation::TokenCreate { grants: &[] }), Forbidden);
        let mixed = [
            Grant {
                vault: vault("a"),
                scope: Read,
            },
            Grant {
                vault: vault("c"),
                scope: Read,
            },
        ];
        assert_eq!(authorize(&c, Operation::TokenCreate { grants: &mixed }), Hidden);
    }

    #[test]
    fn revoking_is_for_the_owner_or_a_server_admin() {
        let c = caller(false, &[]);
        assert_eq!(authorize(&c, Operation::TokenRevoke { owner: "me" }), Allow);
        assert_eq!(authorize(&c, Operation::TokenRevoke { owner: "other" }), Hidden);
    }

    #[test]
    fn the_highest_grant_for_a_vault_counts() {
        let c = caller(false, &[("a", Read), ("a", Admin)]);
        assert_eq!(authorize(&c, Operation::VaultDelete(&vault("a"))), Allow);
    }
}
