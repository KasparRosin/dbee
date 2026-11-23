//! Where the manifest comes from, and the one HTTPS GET that reads it.
//!
//! The check is a single request for
//! `https://github.com/<owner>/<name>/releases/latest/download/SHA256SUMS.txt`, GitHub's own
//! redirect to the newest release that is not a pre-release. No API, no JSON, and no URL read
//! from an answer: every URL is built here. The request carries `User-Agent: dbee/<version>` and
//! nothing else of dbee's; HTTPS only, redirects included.
//!
//! For tests, [`MANIFEST_URL_ENV`] names another base URL, taken only when it is plain HTTP on
//! `127.0.0.1` with a port, so it can never point a copy at another machine.

use std::sync::Arc;
use std::time::Duration;

use semver::Version;
use ureq::config::AutoHeaderValue;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

use crate::UpdateError;
use crate::manifest::{MANIFEST_NAME, MAX_MANIFEST};

/// The repository whose releases a build checks, unless it was built for another.
pub const DEFAULT_REPO: &str = "KasparRosin/dbee";

/// The test aid: a base URL the manifest is read from instead, `http://127.0.0.1:<port>[/path]`.
pub const MANIFEST_URL_ENV: &str = "DBEE_UPDATE_MANIFEST_URL";

/// The test aid's second half: the version this copy pretends to be, read only together with a
/// [`MANIFEST_URL_ENV`] that was taken.
pub const PRETEND_VERSION_ENV: &str = "DBEE_UPDATE_PRETEND_VERSION";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(15);
/// GitHub answers `latest/download` with two redirects.
const MAX_REDIRECTS: u32 = 5;

/// A GitHub repository, `owner/name`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repo(String);

impl Repo {
    /// `owner/name`, each of letters, digits, `.`, `_` and `-`; anything else is no repository.
    pub fn parse(text: &str) -> Option<Repo> {
        let (owner, name) = text.split_once('/')?;
        let part = |s: &str| {
            !s.is_empty() && s.len() <= 100 && s != "." && s != ".." && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        };
        (part(owner) && part(name)).then(|| Repo(text.to_string()))
    }

    /// The repository this binary was built for: `GITHUB_REPOSITORY` at build time (a fork's
    /// release workflow sets it, so its builds check its own releases), else [`DEFAULT_REPO`].
    pub fn built_in() -> Repo {
        option_env!("GITHUB_REPOSITORY").and_then(Repo::parse).unwrap_or_default()
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for Repo {
    fn default() -> Repo {
        Repo(DEFAULT_REPO.to_string())
    }
}

/// The page of release `version`: `https://github.com/<repo>/releases/tag/v<version>`.
pub fn release_page(repo: &Repo, version: &Version) -> String {
    format!("https://github.com/{}/releases/tag/v{version}", repo.as_str())
}

/// Where the manifest is read from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// The repository's newest release on GitHub.
    GitHub(Repo),
    /// A test server on this machine; the text is its base URL, checked by [`Source::loopback`].
    Loopback(String),
}

impl Source {
    /// The source `text` names as a test aid: `http://127.0.0.1:<port>` and an optional path of
    /// letters, digits, `.`, `_`, `-` and `/`. Anything else (another host, HTTPS, `localhost`,
    /// user info, a query) is `None`, and the caller goes on as if nothing was set.
    pub fn loopback(text: &str) -> Option<Source> {
        let rest = text.strip_prefix("http://127.0.0.1:")?;
        let (port, path) = match rest.find('/') {
            Some(at) => rest.split_at_checked(at)?,
            None => (rest, ""),
        };
        let port_ok =
            !port.is_empty() && port.len() <= 5 && !port.starts_with('0') && port.chars().all(|c| c.is_ascii_digit()) && port.parse::<u16>().is_ok();
        let path_ok = path.len() <= 200 && path.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'));
        (port_ok && path_ok).then(|| Source::Loopback(format!("http://127.0.0.1:{port}{}", path.trim_end_matches('/'))))
    }

    /// The manifest's URL.
    pub fn manifest_url(&self) -> String {
        match self {
            Source::GitHub(repo) => format!("https://github.com/{}/releases/latest/download/{MANIFEST_NAME}", repo.as_str()),
            Source::Loopback(base) => format!("{base}/{MANIFEST_NAME}"),
        }
    }

    pub fn is_loopback(&self) -> bool {
        matches!(self, Source::Loopback(_))
    }
}

/// Reads the newest release's manifest. The engine holds one behind this trait, so its tests
/// never touch the network.
pub trait Fetch: Send + Sync {
    /// The manifest's bytes, at most [`MAX_MANIFEST`]. It blocks, so it runs on a blocking thread.
    fn manifest(&self) -> Result<Vec<u8>, UpdateError>;
}

/// The real fetch: one GET with rustls, ring and the system's certificate verifier.
pub struct HttpFetch {
    agent: ureq::Agent,
    url: String,
}

impl std::fmt::Debug for HttpFetch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpFetch").field("url", &self.url).finish_non_exhaustive()
    }
}

impl HttpFetch {
    /// A fetch from `source`, naming itself `dbee/<version>`.
    pub fn new(source: &Source, version: &str) -> HttpFetch {
        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .root_certs(RootCerts::PlatformVerifier)
            .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .build();
        let mut config = ureq::Agent::config_builder()
            .tls_config(tls)
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(TOTAL_TIMEOUT))
            .user_agent(format!("dbee/{version}"))
            .accept(AutoHeaderValue::None)
            .accept_encoding(AutoHeaderValue::None)
            .max_idle_connections(0);
        config = match source {
            // Every hop of a redirect is checked for HTTPS too.
            Source::GitHub(_) => config.https_only(true).max_redirects(MAX_REDIRECTS),
            // The test server is reached directly and followed nowhere.
            Source::Loopback(_) => config.https_only(false).max_redirects(0).proxy(None),
        };
        HttpFetch { agent: config.build().new_agent(), url: source.manifest_url() }
    }
}

impl Fetch for HttpFetch {
    fn manifest(&self) -> Result<Vec<u8>, UpdateError> {
        let limit = u64::try_from(MAX_MANIFEST).unwrap_or(u64::MAX);
        let read = self.agent.get(&self.url).call().and_then(|mut answer| {
            // A redirect that was not followed, or any other answer that is not the file.
            if answer.status() != ureq::http::StatusCode::OK {
                return Err(ureq::Error::StatusCode(answer.status().as_u16()));
            }
            answer.body_mut().with_config().limit(limit).read_to_vec()
        });
        read.map_err(|e| {
            // The error's own text can quote a URL from a redirect, so it is logged, not shown.
            log::debug!("update check: the request failed: {e}");
            UpdateError::Fetch(why(&e).to_string())
        })
    }
}

/// dbee's own words for a failed request.
fn why(e: &ureq::Error) -> &'static str {
    match e {
        ureq::Error::StatusCode(404) => "no release was found (404)",
        ureq::Error::StatusCode(_) => "the server refused the request",
        ureq::Error::Timeout(_) => "it took too long",
        ureq::Error::HostNotFound => "the server's name was not found",
        ureq::Error::BodyExceedsLimit(_) => "the answer is larger than 64 KiB",
        ureq::Error::RequireHttpsOnly(_) => "the answer led away from HTTPS",
        ureq::Error::TooManyRedirects | ureq::Error::RedirectFailed => "the answer redirected too often",
        ureq::Error::Tls(_) | ureq::Error::Rustls(_) => "the server's certificate or the encryption was refused",
        ureq::Error::Io(_) | ureq::Error::ConnectionFailed => "the connection failed",
        _ => "the request failed",
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    use super::*;

    #[test]
    fn the_urls_come_from_the_template() {
        let repo = Repo::default();
        assert_eq!(Source::GitHub(repo.clone()).manifest_url(), "https://github.com/KasparRosin/dbee/releases/latest/download/SHA256SUMS.txt");
        assert_eq!(release_page(&repo, &Version::new(0, 5, 0)), "https://github.com/KasparRosin/dbee/releases/tag/v0.5.0");
        let fork = Repo::parse("someone/dbee-fork").unwrap();
        assert_eq!(Source::GitHub(fork).manifest_url(), "https://github.com/someone/dbee-fork/releases/latest/download/SHA256SUMS.txt");
    }

    #[test]
    fn only_an_owner_and_a_name_make_a_repository() {
        for text in ["KasparRosin/dbee", "a/b", "a-b/c.d_e"] {
            assert_eq!(Repo::parse(text).unwrap().as_str(), text);
        }
        for text in [
            "",
            "dbee",
            "/dbee",
            "a/",
            "a/b/c",
            "a/b?x",
            "a/b#x",
            "a b/c",
            "a/../b",
            "../b",
            "a/..",
            "./b",
            "a@b/c",
            "a/b\n",
            "ä/b",
            "https://github.com/a/b",
        ] {
            assert_eq!(Repo::parse(text), None, "{text:?}");
        }
        assert_eq!(Repo::parse(&format!("{}/b", "a".repeat(101))), None);
    }

    #[test]
    fn the_test_source_is_taken_only_on_loopback() {
        for (text, url) in [
            ("http://127.0.0.1:8123", "http://127.0.0.1:8123/SHA256SUMS.txt"),
            ("http://127.0.0.1:8123/", "http://127.0.0.1:8123/SHA256SUMS.txt"),
            ("http://127.0.0.1:65535/a/b-c_d.e/", "http://127.0.0.1:65535/a/b-c_d.e/SHA256SUMS.txt"),
            ("http://127.0.0.1:1/x", "http://127.0.0.1:1/x/SHA256SUMS.txt"),
        ] {
            let source = Source::loopback(text).unwrap();
            assert!(source.is_loopback());
            assert_eq!(source.manifest_url(), url);
        }
        for text in [
            "",
            "http://127.0.0.1",
            "http://127.0.0.1/",
            "http://127.0.0.1:",
            "http://127.0.0.1:/x",
            "http://127.0.0.1:0",
            "http://127.0.0.1:08123",
            "http://127.0.0.1:65536",
            "http://127.0.0.1:123456",
            "http://127.0.0.1:+80",
            "http://127.0.0.1:80a",
            "https://127.0.0.1:8123",
            "http://localhost:8123",
            "http://127.0.0.2:8123",
            "http://127.0.0.1.example.test:8123",
            "http://127.0.0.1:8123@example.test/",
            "http://example.test@127.0.0.1:8123/",
            "http://127.0.0.1:8123/?next=http://example.test",
            "http://127.0.0.1:8123/#x",
            "http://127.0.0.1:8123/a b",
            "http://127.0.0.1:8123/%2e",
            "http://127.0.0.1:8123\\@example.test",
            "http://[::1]:8123",
            "HTTP://127.0.0.1:8123",
            " http://127.0.0.1:8123",
            "http://example.test/",
            "file:///etc/passwd",
        ] {
            assert_eq!(Source::loopback(text), None, "{text:?}");
        }
        assert_eq!(Source::loopback(&format!("http://127.0.0.1:8123/{}", "a".repeat(201))), None);
    }

    /// A server on a loopback port that answers one request with `answer`, and hands back the
    /// request's lines.
    fn serve_once(answer: Vec<u8>) -> (Source, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut lines = Vec::new();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let line = line.trim_end().to_string();
                if line.is_empty() {
                    break;
                }
                lines.push(line);
            }
            stream.write_all(&answer).unwrap();
            lines
        });
        (Source::loopback(&format!("http://127.0.0.1:{port}/releases")).unwrap(), handle)
    }

    fn answer(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
        let mut out = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n", body.len()).into_bytes();
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn the_request_is_one_get_that_names_dbee_and_nothing_else() {
        let body = b"0000  not a manifest, which is the parser's to say\n";
        let (source, server) = serve_once(answer("200 OK", "", body));
        let got = HttpFetch::new(&source, "0.3.0").manifest().unwrap();
        assert_eq!(got, body);
        let lines = server.join().unwrap();
        assert_eq!(lines.first().map(String::as_str), Some("GET /releases/SHA256SUMS.txt HTTP/1.1"));
        let mut headers: Vec<String> = lines.iter().skip(1).map(|l| l.to_ascii_lowercase()).collect();
        headers.sort();
        let host = headers.iter().find(|h| h.starts_with("host: ")).unwrap();
        assert!(host.starts_with("host: 127.0.0.1:"), "{host}");
        let others: Vec<&String> = headers.iter().filter(|h| !h.starts_with("host: ")).collect();
        assert_eq!(others, ["user-agent: dbee/0.3.0"], "nothing but the host and dbee's name is sent");
    }

    #[test]
    fn an_answer_over_the_cap_is_refused() {
        let (source, server) = serve_once(answer("200 OK", "", &vec![b'a'; MAX_MANIFEST + 1]));
        assert_eq!(HttpFetch::new(&source, "0.3.0").manifest(), Err(UpdateError::Fetch("the answer is larger than 64 KiB".into())));
        server.join().unwrap();
        let (source, server) = serve_once(answer("200 OK", "", &vec![b'a'; MAX_MANIFEST]));
        assert_eq!(HttpFetch::new(&source, "0.3.0").manifest().map(|b| b.len()), Ok(MAX_MANIFEST));
        server.join().unwrap();
    }

    #[test]
    fn a_missing_release_and_a_server_error_are_errors() {
        let (source, server) = serve_once(answer("404 Not Found", "", b"Not Found"));
        assert_eq!(HttpFetch::new(&source, "0.3.0").manifest(), Err(UpdateError::Fetch("no release was found (404)".into())));
        server.join().unwrap();
        let (source, server) = serve_once(answer("503 Service Unavailable", "", b""));
        assert_eq!(HttpFetch::new(&source, "0.3.0").manifest(), Err(UpdateError::Fetch("the server refused the request".into())));
        server.join().unwrap();
    }

    /// The test source follows no redirect, so a local server cannot send a copy elsewhere.
    #[test]
    fn the_test_source_follows_no_redirect() {
        let (source, server) = serve_once(answer("302 Found", "Location: http://192.0.2.1/SHA256SUMS.txt\r\n", b""));
        assert_eq!(HttpFetch::new(&source, "0.3.0").manifest(), Err(UpdateError::Fetch("the server refused the request".into())));
        server.join().unwrap();
    }

    #[test]
    fn a_port_nobody_listens_on_is_an_error() {
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let source = Source::loopback(&format!("http://127.0.0.1:{port}")).unwrap();
        assert!(matches!(HttpFetch::new(&source, "0.3.0").manifest(), Err(UpdateError::Fetch(_))));
    }

    /// The real source refuses plain HTTP before anything is sent, on the first hop as on a
    /// redirect's (ureq checks each hop's scheme the same way).
    #[test]
    fn the_real_source_is_https_only() {
        let fetch = HttpFetch::new(&Source::GitHub(Repo::default()), "0.3.0");
        assert!(fetch.url.starts_with("https://github.com/"));
        assert!(fetch.agent.config().https_only());
        assert_eq!(fetch.agent.config().max_redirects(), MAX_REDIRECTS);
        let (source, _server) = serve_once(Vec::new());
        let plain = HttpFetch { agent: fetch.agent, url: source.manifest_url() };
        assert_eq!(plain.manifest(), Err(UpdateError::Fetch("the answer led away from HTTPS".into())));
    }
}
