//! Frontend: axum. Handlers parse, call the core on a blocking thread, and format. No rules live here.
mod activity;
mod browser;
mod conn_watch;
mod error;
mod routes;

pub use browser::{CSRF_HEADER, ConsoleLookup, SESSION_COOKIE, embedded_asset};
pub use routes::{BODY_LIMIT, openapi, parse_range, router, router_with, router_with_console, serve, serve_with};
