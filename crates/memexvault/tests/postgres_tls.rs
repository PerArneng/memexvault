//! Spec 059: TLS to Postgres against real servers. Needs compose's `postgres-tls` service and the plaintext one; AGENTS.md has the commands.
//! `db.memex.test` is a name only in the test certificate: `hostaddr` sends the connection to 127.0.0.1 while the name is the one checked.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]
use memex_store_postgres::{PgError, PgStore};

const TLS_SERVER: &str = "postgres:memex@127.0.0.1:55459/memex";
const NAMED_TLS_SERVER: &str = "postgres:memex@db.memex.test:55459/memex";

fn ca(name: &str) -> String {
    let dir = std::env::var("MEMEX_TEST_TLS_CERTS").expect("MEMEX_TEST_TLS_CERTS");
    format!("{dir}/{name}.crt")
}

/// The plaintext test server's user, host and database, with no scheme or options.
fn plain_server() -> String {
    let url = std::env::var("MEMEX_TEST_DATABASE_URL").expect("MEMEX_TEST_DATABASE_URL");
    url.strip_prefix("postgres://").expect("a postgres:// URL").to_string()
}

fn ssl_in_use(url: &str) -> (bool, Option<String>) {
    let store = PgStore::connect(url, 1).unwrap_or_else(|e| panic!("{url}: {e}"));
    let mut conn = store.take_conn().unwrap();
    let row = conn
        .client
        .query_one("SELECT ssl, version FROM pg_stat_ssl WHERE pid = pg_backend_pid()", &[])
        .unwrap();
    (row.get(0), row.get(1))
}

fn failure(url: &str) -> PgError {
    match PgStore::connect(url, 1) {
        Ok(_) => panic!("{url} connected"),
        Err(e) => e,
    }
}

#[test]
#[ignore = "needs the TLS test server"]
fn tls_verify_full_connects_with_the_right_ca() {
    let (ssl, version) = ssl_in_use(&format!("postgres://{TLS_SERVER}?sslmode=verify-full&sslrootcert={}", ca("ca")));
    assert!(ssl);
    println!("negotiated {version:?}");
}

#[test]
#[ignore = "needs the TLS test server"]
fn tls_verify_full_refuses_the_wrong_ca() {
    let e = failure(&format!("postgres://{TLS_SERVER}?sslmode=verify-full&sslrootcert={}", ca("wrong-ca"))).to_string();
    assert!(e.contains("UnknownIssuer"), "{e}");
}

#[test]
#[ignore = "needs the TLS test server"]
fn tls_verify_full_refuses_the_wrong_host_name() {
    let url = format!(
        "postgres://postgres:memex@other.memex.test:55459/memex?hostaddr=127.0.0.1&sslmode=verify-full&sslrootcert={}",
        ca("ca")
    );
    let e = failure(&url).to_string();
    assert!(e.contains("not valid for name"), "{e}");
}

#[test]
#[ignore = "needs the TLS test server"]
fn tls_require_encrypts_without_checking_the_certificate() {
    assert!(ssl_in_use(&format!("postgres://{TLS_SERVER}?sslmode=require")).0);
}

#[test]
#[ignore = "needs the TLS test server"]
fn tls_a_non_local_host_verifies_by_default() {
    assert!(ssl_in_use(&format!("postgres://{NAMED_TLS_SERVER}?hostaddr=127.0.0.1&sslrootcert={}", ca("ca"))).0);
    let e = failure(&format!("postgres://{NAMED_TLS_SERVER}?hostaddr=127.0.0.1&sslrootcert={}", ca("wrong-ca"))).to_string();
    assert!(e.contains("UnknownIssuer"), "{e}");
}

#[test]
#[ignore = "needs the plaintext test server"]
fn tls_a_non_local_host_refuses_plaintext_by_default() {
    let plain = plain_server().replace("127.0.0.1", "db.memex.test");
    let url = format!("postgres://{plain}?hostaddr=127.0.0.1");
    // The server is reachable and takes plaintext when told to, so what refuses below is the default.
    assert!(!ssl_in_use(&format!("{url}&sslmode=disable")).0);
    let e = failure(&format!("{url}&sslrootcert={}", ca("ca"))).to_string();
    assert!(e.contains("does not support TLS"), "{e}");
    assert!(matches!(failure(&url), PgError::Refused(_)));
}

#[test]
#[ignore = "needs the plaintext test server"]
fn tls_a_local_host_stays_plaintext_by_default() {
    assert!(!ssl_in_use(&format!("postgres://{}", plain_server())).0);
    assert!(!ssl_in_use(&format!("postgres://{TLS_SERVER}")).0);
}

#[test]
#[ignore = "needs the plaintext test server"]
fn tls_require_refuses_a_server_without_tls() {
    let e = failure(&format!("postgres://{}?sslmode=require", plain_server())).to_string();
    assert!(e.contains("does not support TLS"), "{e}");
}
