//! Edge: the Store on PostgreSQL. All SQL lives here. One schema per vault, with real tables. The binary crate wraps this behind the core trait.
mod migrate;
mod net;
mod registry;
mod store;

pub use migrate::{LATEST_SERVER_VERSION, LATEST_VAULT_VERSION};
pub use net::{NetMutations, net_mutations};
pub use registry::{PgRegistryError, schema_for};
pub use store::{PgError, PgLease, PgSnapshot, PgStore};
