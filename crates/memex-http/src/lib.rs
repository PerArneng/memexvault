//! Frontend: axum. Handlers parse, call the core on a blocking thread, and format. No rules live here.
mod error;
mod routes;

pub use routes::{BODY_LIMIT, openapi, parse_range, router, serve};
