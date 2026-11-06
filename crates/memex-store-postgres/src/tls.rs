//! How to reach Postgres: reads `sslmode` and `sslrootcert` from the connection URL and picks plaintext or rustls.
//!
//! `disable` is plaintext. `require` encrypts but accepts any certificate. `verify-full` checks the certificate against the CA in
//! `sslrootcert`, and the host name against the certificate. With no `sslmode`, a host on this machine gets `disable` and any other
//! host gets `verify-full`. The `postgres` crate rejects both options, so they're taken out before it parses the rest.
use crate::store::{PgError, describe};
use percent_encoding::percent_decode_str;
use postgres::config::{Host, SslMode};
use postgres::{Client, Config, NoTls};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use std::net::IpAddr;
use std::sync::Arc;
use tokio_postgres_rustls::MakeRustlsConnect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Disable,
    Require,
    VerifyFull,
}

/// A parsed connection URL and the TLS it needs, ready to open connections.
pub(crate) struct Target {
    config: Config,
    tls: Option<MakeRustlsConnect>,
}

impl Target {
    /// Refuses a URL that asks for something it can't honor. Reads the `sslrootcert` file once, so a new CA needs a restart.
    pub(crate) fn parse(url: &str) -> Result<Self, PgError> {
        let plan = plan(url)?;
        let mut config = plan.config;
        let tls = match plan.mode {
            Mode::Disable => {
                config.ssl_mode(SslMode::Disable);
                None
            }
            Mode::Require => {
                config.ssl_mode(SslMode::Require);
                Some(MakeRustlsConnect::new(unverified_config()?))
            }
            Mode::VerifyFull => {
                config.ssl_mode(SslMode::Require);
                let path = plan.root_cert.as_deref().unwrap_or_default();
                let pem = read_root_cert(path)?;
                Some(MakeRustlsConnect::new(verified_config(&pem, path)?))
            }
        };
        Ok(Self { config, tls })
    }

    pub(crate) fn connect(&self) -> Result<Client, postgres::Error> {
        match &self.tls {
            Some(tls) => self.config.connect(tls.clone()),
            None => self.config.connect(NoTls),
        }
    }
}

// Edge: the CA file is named in the URL, and this crate has no file trait to go through.
#[allow(clippy::disallowed_methods)]
fn read_root_cert(path: &str) -> Result<Vec<u8>, PgError> {
    std::fs::read(path).map_err(|e| PgError::Refused(format!("can't read sslrootcert {path}: {e}")))
}

struct Plan {
    config: Config,
    mode: Mode,
    root_cert: Option<String>,
}

fn plan(url: &str) -> Result<Plan, PgError> {
    let options = split_options(url);
    let config: Config = options.rest.parse().map_err(|e| PgError::Backend(describe(&e)))?;
    let mode = match options.sslmode.as_deref() {
        None if is_local(&config) => Mode::Disable,
        None | Some("verify-full") => Mode::VerifyFull,
        Some("disable") => Mode::Disable,
        Some("require") => Mode::Require,
        Some(other) => {
            return Err(PgError::Refused(format!(
                "sslmode {other} isn't supported: use disable, require or verify-full"
            )));
        }
    };
    match (mode, &options.sslrootcert) {
        (Mode::VerifyFull, None) => Err(PgError::Refused(
            "sslmode verify-full, the default for a host on another machine, needs sslrootcert set to the CA's PEM file".into(),
        )),
        (Mode::Disable | Mode::Require, Some(_)) => Err(PgError::Refused("sslrootcert is only checked with sslmode verify-full".into())),
        _ => Ok(Plan {
            config,
            mode,
            root_cert: options.sslrootcert,
        }),
    }
}

/// Local only if every host and address is. Each host must be, since a connection may reach any of them.
fn is_local(config: &Config) -> bool {
    let host_is_local = |host: &Host| match host {
        Host::Tcp(name) => name.eq_ignore_ascii_case("localhost") || name.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback()),
        // A Unix socket never leaves the machine.
        #[allow(unreachable_patterns)]
        _ => true,
    };
    config.get_hosts().iter().all(host_is_local) && config.get_hostaddrs().iter().all(IpAddr::is_loopback)
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Options {
    /// The URL without the two options below.
    rest: String,
    sslmode: Option<String>,
    sslrootcert: Option<String>,
}

impl Options {
    /// Takes the option if it's one of ours. As in libpq, the last one wins.
    fn take(&mut self, key: &str, value: String) -> bool {
        match key {
            "sslmode" => self.sslmode = Some(value),
            "sslrootcert" => self.sslrootcert = Some(value),
            _ => return false,
        }
        true
    }
}

fn split_options(url: &str) -> Options {
    if url.starts_with("postgres://") || url.starts_with("postgresql://") {
        split_url(url)
    } else {
        split_keywords(url)
    }
}

fn split_url(url: &str) -> Options {
    let mut options = Options::default();
    let Some((base, query)) = url.split_once('?') else {
        options.rest = url.to_string();
        return options;
    };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|param| {
            let Some((key, value)) = param.split_once('=') else {
                return true;
            };
            !options.take(key, percent_decode_str(value).decode_utf8_lossy().into_owned())
        })
        .collect();
    options.rest = if kept.is_empty() {
        base.to_string()
    } else {
        format!("{base}?{}", kept.join("&"))
    };
    options
}

/// The `host=h user=u password='a b'` form. A malformed string stays in `rest`, for the `postgres` crate to report.
fn split_keywords(s: &str) -> Options {
    let mut options = Options::default();
    let mut kept = vec![];
    let mut rest = s.trim_start();
    while let Some((key, after)) = rest.split_once('=') {
        let (value, tail) = keyword_value(after.trim_start());
        if !options.take(key.trim_end(), value) {
            kept.push(&rest[..rest.len() - tail.len()]);
        }
        rest = tail.trim_start();
    }
    kept.push(rest);
    options.rest = kept.join(" ").trim_end().to_string();
    options
}

/// A value, with quotes and backslashes removed, and what follows it.
fn keyword_value(s: &str) -> (String, &str) {
    let quoted = s.starts_with('\'');
    let mut chars = s.char_indices().skip(usize::from(quoted));
    let mut value = String::new();
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' => value.extend(chars.next().map(|(_, escaped)| escaped)),
            '\'' if quoted => return (value, &s[i + 1..]),
            c if !quoted && c.is_whitespace() => return (value, &s[i..]),
            c => value.push(c),
        }
    }
    (value, "")
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn refused(e: rustls::Error) -> PgError {
    PgError::Refused(format!("can't set up TLS: {e}"))
}

fn verified_config(pem: &[u8], path: &str) -> Result<ClientConfig, PgError> {
    let unreadable = |e: &dyn std::fmt::Display| PgError::Refused(format!("sslrootcert {path} isn't usable: {e}"));
    let mut roots = RootCertStore::empty();
    for cert in CertificateDer::pem_slice_iter(pem) {
        roots.add(cert.map_err(|e| unreadable(&e))?).map_err(|e| unreadable(&e))?;
    }
    if roots.is_empty() {
        return Err(unreadable(&"it holds no certificates"));
    }
    Ok(ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(refused)?
        .with_root_certificates(roots)
        .with_no_client_auth())
}

fn unverified_config() -> Result<ClientConfig, PgError> {
    let provider = provider();
    Ok(ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(refused)?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AnyCertificate(provider)))
        .with_no_client_auth())
}

/// For `sslmode=require`, which encrypts but doesn't say who the server is. The handshake signature is still checked.
#[derive(Debug)]
struct AnyCertificate(Arc<CryptoProvider>);

impl ServerCertVerifier for AnyCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(url: &str) -> Mode {
        plan(url).map(|p| p.mode).map_err(|e| e.to_string()).unwrap()
    }

    fn refusal(url: &str) -> String {
        match plan(url) {
            Err(PgError::Refused(m)) => m,
            Err(e) => panic!("{url}: expected a refusal, got {e}"),
            Ok(p) => panic!("{url}: expected a refusal, got {:?}", p.mode),
        }
    }

    #[test]
    fn a_local_host_defaults_to_plaintext() {
        for url in [
            "postgres://u:p@localhost:5432/d",
            "postgres://u:p@LOCALHOST/d",
            "postgres://u:p@127.0.0.1:55432/d",
            "postgres://u:p@127.8.8.8/d",
            "postgres://u:p@[::1]:5432/d",
            "postgres://u:p@localhost,127.0.0.1/d",
            "postgres://u@%2Fvar%2Frun%2Fpostgresql/d",
            "host=localhost user=u",
        ] {
            assert_eq!(mode(url), Mode::Disable, "{url}");
        }
    }

    #[test]
    fn any_other_host_defaults_to_verify_full_and_needs_a_ca() {
        for url in [
            "postgres://u:p@db.example.com/d",
            "postgres://u:p@10.0.0.5/d",
            "postgres://u:p@localhost.example.com/d",
            "postgres://u:p@[2001:db8::1]/d",
            "postgres://u:p@localhost,db.example.com/d",
            "postgres://u:p@localhost/d?hostaddr=10.0.0.5",
            "postgres://u:p@db.example.com/d?hostaddr=127.0.0.1",
            "host=db.example.com user=u",
        ] {
            assert!(refusal(url).contains("needs sslrootcert"), "{url}");
        }
        assert_eq!(mode("postgres://u:p@db.example.com/d?sslrootcert=/ca.crt"), Mode::VerifyFull);
    }

    #[test]
    fn an_explicit_sslmode_wins_over_the_default() {
        assert_eq!(mode("postgres://u:p@db.example.com/d?sslmode=disable"), Mode::Disable);
        assert_eq!(mode("postgres://u:p@db.example.com/d?sslmode=require"), Mode::Require);
        assert_eq!(mode("postgres://u:p@localhost/d?sslmode=require"), Mode::Require);
        assert_eq!(mode("postgres://u:p@localhost/d?sslmode=verify-full&sslrootcert=/ca.crt"), Mode::VerifyFull);
        assert_eq!(mode("postgres://u:p@localhost/d?sslmode=require&sslmode=disable"), Mode::Disable);
    }

    #[test]
    fn modes_we_do_not_offer_are_refused_not_weakened() {
        for bad in ["prefer", "allow", "verify-ca", "VERIFY-FULL", ""] {
            assert!(
                refusal(&format!("postgres://u:p@db.example.com/d?sslmode={bad}")).contains("isn't supported"),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_ca_that_would_not_be_checked_is_refused() {
        for m in ["disable", "require"] {
            assert!(
                refusal(&format!("postgres://u:p@h/d?sslmode={m}&sslrootcert=/ca.crt")).contains("only checked"),
                "{m}"
            );
        }
        assert!(refusal("postgres://u:p@localhost/d?sslrootcert=/ca.crt").contains("only checked"));
    }

    #[test]
    fn a_refusal_never_shows_the_password() {
        for url in ["postgres://u:hunter2@h/d?sslmode=nope", "postgres://u:hunter2@h/d", "host=h password=hunter2"] {
            assert!(!refusal(url).contains("hunter2"), "{url}");
        }
    }

    #[test]
    fn other_options_reach_the_postgres_crate() {
        let o = split_options("postgres://u:p@h/d?application_name=a%20b&sslmode=require&connect_timeout=3&sslrootcert=%2Fetc%2Fca.crt");
        assert_eq!(o.rest, "postgres://u:p@h/d?application_name=a%20b&connect_timeout=3");
        assert_eq!(o.sslmode.as_deref(), Some("require"));
        assert_eq!(o.sslrootcert.as_deref(), Some("/etc/ca.crt"));
        assert_eq!(split_options("postgresql://h/d?sslmode=disable").rest, "postgresql://h/d");
        assert_eq!(split_options("postgres://h/d").rest, "postgres://h/d");
    }

    #[test]
    fn a_keyword_string_gives_up_only_its_ssl_options() {
        let o = split_options(r"host=h sslmode = require password='a sslmode=disable \' b' sslrootcert='/my certs/ca.crt' user=u");
        assert_eq!(o.rest, r"host=h password='a sslmode=disable \' b' user=u");
        assert_eq!(o.sslmode.as_deref(), Some("require"));
        assert_eq!(o.sslrootcert.as_deref(), Some("/my certs/ca.crt"));
        let config: Config = o.rest.parse().unwrap();
        assert_eq!(config.get_password(), Some(&b"a sslmode=disable ' b"[..]));
        assert_eq!(config.get_user(), Some("u"));
    }

    #[test]
    fn a_malformed_url_is_still_reported() {
        assert!(matches!(plan("postgres://u:p@h/d?nonsense=1"), Err(PgError::Backend(_))));
        assert!(matches!(plan("host"), Err(PgError::Backend(_))));
    }

    #[test]
    fn a_ca_file_must_hold_certificates() {
        for pem in [
            &b""[..],
            b"not a certificate",
            b"-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n",
        ] {
            assert!(matches!(verified_config(pem, "/ca.crt"), Err(PgError::Refused(_))));
        }
    }

    #[test]
    fn a_missing_ca_file_is_refused() {
        let Err(PgError::Refused(m)) = Target::parse("postgres://u:p@h/d?sslrootcert=/no/such/ca.crt") else {
            panic!("expected a refusal");
        };
        assert!(m.contains("/no/such/ca.crt"), "{m}");
    }
}
