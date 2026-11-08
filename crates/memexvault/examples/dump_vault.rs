//! Spec 046: prints one vault's knowledge rows through the store's dump helper, so a browser test can compare them before and after a revert.
//! Read-only: it takes a snapshot and only reads rows, and it never migrates. `MEMEX_DATABASE_URL` names the database.
//! A vault's whole dump can't match after a revert, as the head moves and the revert is itself a ChangeSet, so this leaves out the head and everything but the tables the backend's own revert test compares (`state_without_changesets` in memex-core/tests/governance.rs).
// Reading env and args and printing to the terminal are this file's job.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]
use memex_core::{Snapshot, Store, canonical_dump};
use memex_store_postgres::PgStore;
use memex_types::{Row, Table, VaultId};
use memexvault::pg_edge::PostgresStore;
use std::process::ExitCode;
use std::sync::Arc;

/// The same six tables as `state_without_changesets`.
const KNOWLEDGE: [Table; 6] = [Table::Source, Table::Page, Table::Edge, Table::Segment, Table::Assertion, Table::Evidence];

fn run(vault: &str) -> Result<String, String> {
    let url = std::env::var("MEMEX_DATABASE_URL").map_err(|_| "MEMEX_DATABASE_URL isn't set".to_string())?;
    let store = PostgresStore(Arc::new(PgStore::connect(&url, 1).map_err(|e| e.to_string())?));
    let snap = store.snapshot(&VaultId(vault.into())).map_err(|e| e.to_string())?;
    let mut rows: Vec<Row> = vec![];
    for table in KNOWLEDGE {
        let mut after = None;
        loop {
            let page = snap.rows(table, after.as_ref()).map_err(|e| e.to_string())?;
            let Some(last) = page.last() else { break };
            after = Some(last.header().id.clone());
            rows.extend(page);
        }
    }
    Ok(canonical_dump(None, &rows).text)
}

fn main() -> ExitCode {
    let Some(vault) = std::env::args().nth(1) else {
        eprintln!("usage: dump_vault <vault id>");
        return ExitCode::from(2);
    };
    match run(&vault) {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("dump_vault: {e}");
            ExitCode::from(2)
        }
    }
}
