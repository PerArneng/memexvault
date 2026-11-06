//! The contract any Registry must meet. Names must be unique to each run, since a real backend keeps them.
use crate::{Registry, RegistryError};
use memex_types::{Audit, Grant, Principal, PrincipalKind, Scope, TokenRow, VaultId, VaultRow};

fn audit(action: &str) -> Audit {
    Audit {
        actor: "contract".into(),
        time_ms: 1,
        action: action.into(),
        target: "t".into(),
    }
}

fn principal(id: &str, name: &str) -> Principal {
    Principal {
        id: id.into(),
        name: name.into(),
        kind: PrincipalKind::Agent,
        server_admin: false,
        disabled: false,
        created_by: "contract".into(),
        created_at_ms: 5,
    }
}

/// `tag` makes ids and names unique, so this can run on a shared database.
pub fn run_all<R: Registry>(r: &R, tag: &str) {
    let id = |s: &str| format!("{tag}-{s}");

    // Principals.
    r.insert_principal(&principal(&id("p1"), &id("alice")), &audit("principal.create"))
        .expect("insert");
    r.insert_principal(&principal(&id("p2"), &id("bob")), &audit("principal.create"))
        .expect("insert");
    assert!(matches!(
        r.insert_principal(&principal(&id("p3"), &id("alice")), &audit("x")),
        Err(RegistryError::Conflict(_))
    ));
    assert_eq!(r.principal(&id("p1")).expect("get").expect("found").name, id("alice"));
    assert_eq!(r.principal(&id("nope")).expect("get"), None);
    assert!(r.principal_count().expect("count") >= 2);
    let page = r.principals(Some(&format!("{tag}-")), 1).expect("list");
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].id, id("p1"));
    let next = r.principals(Some(&id("p1")), 1).expect("list");
    assert_eq!(next[0].id, id("p2"));
    r.set_principal_disabled(&id("p1"), true, &audit("principal.disable")).expect("disable");
    assert!(r.principal(&id("p1")).expect("get").expect("found").disabled);
    assert_eq!(r.set_principal_disabled(&id("nope"), true, &audit("x")), Err(RegistryError::NotFound));

    // Vaults.
    let vault = |s: &str, name: &str| VaultRow {
        id: VaultId(id(s)),
        name: id(name),
        created_by: id("p1"),
        created_at_ms: 7,
    };
    r.insert_vault(&vault("v1", "first"), &audit("vault.create")).expect("insert");
    r.insert_vault(&vault("v2", "second"), &audit("vault.create")).expect("insert");
    assert!(matches!(r.insert_vault(&vault("v3", "first"), &audit("x")), Err(RegistryError::Conflict(_))));
    r.rename_vault(&VaultId(id("v1")), &id("renamed"), &audit("vault.rename")).expect("rename");
    assert_eq!(r.vault(&VaultId(id("v1"))).expect("get").expect("found").name, id("renamed"));
    assert!(matches!(
        r.rename_vault(&VaultId(id("v2")), &id("renamed"), &audit("x")),
        Err(RegistryError::Conflict(_))
    ));
    assert_eq!(r.rename_vault(&VaultId(id("nope")), "x", &audit("x")), Err(RegistryError::NotFound));
    let only = [VaultId(id("v2"))];
    let listed = r.vaults(Some(&only), None, 10).expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, only[0]);
    assert!(r.vaults(Some(&[]), None, 10).expect("list").is_empty());
    r.delete_vault(&VaultId(id("v2")), &audit("vault.delete")).expect("delete");
    assert_eq!(r.vault(&VaultId(id("v2"))).expect("get"), None);
    assert_eq!(r.delete_vault(&VaultId(id("v2")), &audit("x")), Err(RegistryError::NotFound));

    // Tokens.
    let token = TokenRow {
        id: id("t1"),
        principal_id: id("p2"),
        hash: id("hash1"),
        name: "ci".into(),
        grants: vec![Grant {
            vault: VaultId(id("v1")),
            scope: Scope::Propose,
        }],
        created_by: id("p1"),
        created_at_ms: 9,
        expires_at_ms: Some(1000),
        last_used_at_ms: None,
        revoked_at_ms: None,
    };
    r.insert_token(&token, &audit("token.create")).expect("insert");
    assert_eq!(r.token(&id("t1")).expect("get").expect("found"), token);
    assert_eq!(r.token_by_hash(&id("hash1")).expect("get").expect("found"), token);
    assert_eq!(r.token_by_hash("no such hash").expect("get"), None);
    assert_eq!(r.tokens(Some(&id("p2")), None, 10).expect("list"), vec![token.clone()]);
    assert!(r.tokens(Some(&id("p1")), None, 10).expect("list").is_empty());
    r.touch_token(&id("t1"), 50).expect("touch");
    assert_eq!(r.token(&id("t1")).expect("get").expect("found").last_used_at_ms, Some(50));
    r.revoke_token(&id("t1"), 60, &audit("token.revoke")).expect("revoke");
    assert_eq!(r.token(&id("t1")).expect("get").expect("found").revoked_at_ms, Some(60));
    assert_eq!(r.revoke_token(&id("nope"), 60, &audit("x")), Err(RegistryError::NotFound));

    // Every write above left an audit entry, newest first.
    let log = r.audit_log(3).expect("audit");
    assert_eq!(log.len(), 3);
    assert_eq!(log[0].action, "token.revoke");
    assert_eq!(log[1].action, "token.create");
    assert_eq!(log[0].actor, "contract");
}
