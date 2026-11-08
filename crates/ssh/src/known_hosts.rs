//! Host key check against `known_hosts`: known, unknown (asks once and can be learned),
//! changed, of another type than the one recorded, or revoked (all refused, never overridable).
//!
//! The file is read as OpenSSH reads it: fields split on spaces and tabs, `@revoked` and
//! `@cert-authority` markers, comma lists with `!` and the wildcards `*` and `?`, hashed names
//! (`|1|salt|hash`), names compared without regard to case. Not read: `/etc/ssh/ssh_known_hosts`
//! (docs/gaps.md).

use std::path::Path;

use data_encoding::BASE64_MIME;
use hmac::{Hmac, KeyInit, Mac};
use russh::Preferred;
use russh::keys::known_hosts::learn_known_hosts_path;
use russh::keys::{Algorithm, HashAlg, PublicKey, parse_public_key_base64};
use sha1::Sha1;

/// How `known_hosts` names an SSH server: `host` on port 22, else `[host]:port`.
pub fn host_port(host: &str, port: u16) -> String {
    if port == 22 { host.to_owned() } else { format!("[{host}]:{port}") }
}

/// The key's SHA256 fingerprint as OpenSSH prints it: `SHA256:<base64>`.
pub fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

/// The key's type as `known_hosts` writes it, e.g. `ssh-ed25519`.
pub fn key_type(key: &PublicKey) -> String {
    key.algorithm().as_str().to_owned()
}

/// What the check decided about a server's host key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Recorded in `known_hosts`.
    Known,
    /// Not recorded, the user accepted its fingerprint, and it is now recorded.
    Learned,
    /// Not recorded and accepted for this connection only: there is no `.ssh` directory, or
    /// writing `known_hosts` failed (`unsaved` says why, for the user).
    Trusted { unsaved: Option<String> },
    /// Not recorded and not (or not this key) accepted.
    Unknown { key_type: String, fingerprint: String },
    /// Another key of the same type is recorded at this 1-based line of the file.
    Changed { line: usize },
    /// Keys of other types only are recorded for this host: what a man in the middle that
    /// cannot show the recorded key looks like. Never overridable.
    OtherType { key_type: String, recorded: Vec<String>, lines: Vec<usize> },
    /// This key is marked `@revoked` for this host at this 1-based line.
    Revoked { line: usize },
    /// `known_hosts` exists but could not be read, or a line for this host is malformed.
    Failed(String),
    /// The server offered something this client does not check (a host certificate).
    Unsupported(String),
}

impl Verdict {
    pub(crate) fn accepts(&self) -> bool {
        matches!(self, Verdict::Known | Verdict::Learned | Verdict::Trusted { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Marker {
    None,
    Revoked,
    CertAuthority,
}

struct Entry {
    line: usize,
    marker: Marker,
    key: PublicKey,
}

/// The entries of `file` for `host:port`. `Err` when the file cannot be read, or a line for this
/// host holds no key.
fn entries_for(host: &str, port: u16, file: &Path) -> Result<Vec<Entry>, String> {
    let bytes = match std::fs::read(file) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{} cannot be read ({e})", file.display())),
    };
    let text = String::from_utf8_lossy(&bytes);
    let name = host_port(host, port).to_lowercase();
    let mut found = Vec::new();
    for (i, raw) in text.split('\n').enumerate() {
        let line = i.saturating_add(1);
        let mut fields = raw.split([' ', '\t', '\r']).filter(|f| !f.is_empty());
        let Some(mut first) = fields.next() else { continue };
        if first.starts_with('#') {
            continue;
        }
        let marker = match first {
            "@revoked" => Marker::Revoked,
            "@cert-authority" => Marker::CertAuthority,
            m if m.starts_with('@') => continue,
            _ => Marker::None,
        };
        if marker != Marker::None {
            let Some(names) = fields.next() else { continue };
            first = names;
        }
        if !names_match(first, &name) {
            continue;
        }
        let (Some(_type), Some(encoded)) = (fields.next(), fields.next()) else {
            return Err(format!("line {line} of {} has no key", file.display()));
        };
        if BASE64_MIME.decode(encoded.as_bytes()).is_err() {
            return Err(format!("line {line} of {} holds a key that is not base64", file.display()));
        }
        // A key type dbee does not know (a security-key type, say) is no key to compare.
        if let Ok(key) = parse_public_key_base64(encoded) {
            found.push(Entry { line, marker, key });
        }
    }
    Ok(found)
}

/// Whether the names field of a `known_hosts` line covers `name` (lower case): a hashed name, or
/// a comma list of patterns where `!` excludes.
fn names_match(field: &str, name: &str) -> bool {
    if field.starts_with("|1|") {
        return hashed_match(field, name);
    }
    let mut matched = false;
    for pattern in field.split(',') {
        let (negated, pattern) = match pattern.strip_prefix('!') {
            Some(p) => (true, p),
            None => (false, pattern),
        };
        if wildcard(&pattern.to_lowercase(), name) {
            if negated {
                return false;
            }
            matched = true;
        }
    }
    matched
}

fn hashed_match(field: &str, name: &str) -> bool {
    let mut parts = field.split('|').skip(2);
    let (Some(Ok(salt)), Some(Ok(hash))) =
        (parts.next().map(|p| BASE64_MIME.decode(p.as_bytes())), parts.next().map(|p| BASE64_MIME.decode(p.as_bytes())))
    else {
        return false;
    };
    Hmac::<Sha1>::new_from_slice(&salt).is_ok_and(|mac| mac.chain_update(name).verify_slice(&hash).is_ok())
}

/// `pattern` with `*` (any run) and `?` (one character) against `text`; no backtracking beyond
/// the last `*`, so no input makes it slow.
fn wildcard(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        match p.get(pi) {
            Some('*') => {
                star = Some((pi, ti));
                pi = pi.saturating_add(1);
            }
            Some(c) if *c == '?' || Some(c) == t.get(ti) => {
                pi = pi.saturating_add(1);
                ti = ti.saturating_add(1);
            }
            _ => match star {
                Some((sp, st)) => {
                    pi = sp.saturating_add(1);
                    ti = st.saturating_add(1);
                    star = Some((sp, ti));
                }
                None => return false,
            },
        }
    }
    p.iter().skip(pi).all(|c| *c == '*')
}

/// Check `key` for `host:port` against `file`; with `trust` equal to the key's fingerprint an
/// unknown key is learned. A changed, other-type or revoked key is refused whatever `trust` says.
pub(crate) fn check(host: &str, port: u16, key: &PublicKey, file: Option<&Path>, trust: Option<&str>) -> Verdict {
    // A name that `known_hosts` cannot hold would write a line that vouches for other hosts.
    if !dbee_config::is_host_name(host) {
        return Verdict::Failed(format!("{host:?} is not a host name or an IP address"));
    }
    let recorded = match file.map(|f| entries_for(host, port, f)) {
        Some(Ok(entries)) => entries,
        Some(Err(m)) => return Verdict::Failed(m),
        None => Vec::new(),
    };
    if let Some(revoked) = recorded.iter().find(|e| e.marker == Marker::Revoked && e.key == *key) {
        return Verdict::Revoked { line: revoked.line };
    }
    let keys: Vec<&Entry> = recorded.iter().filter(|e| e.marker == Marker::None).collect();
    if keys.iter().any(|e| e.key == *key) {
        return Verdict::Known;
    }
    if let Some(same_type) = keys.iter().find(|e| e.key.algorithm() == key.algorithm()) {
        return Verdict::Changed { line: same_type.line };
    }
    if !keys.is_empty() {
        let mut types: Vec<String> = Vec::new();
        for e in &keys {
            let t = key_type(&e.key);
            if !types.contains(&t) {
                types.push(t);
            }
        }
        return Verdict::OtherType { key_type: key_type(key), recorded: types, lines: keys.iter().map(|e| e.line).collect() };
    }
    let fingerprint = fingerprint(key);
    if trust != Some(fingerprint.as_str()) {
        return Verdict::Unknown { key_type: key_type(key), fingerprint };
    }
    let Some(f) = file else { return Verdict::Trusted { unsaved: None } };
    match learn_known_hosts_path(host, port, key, f) {
        Ok(()) => Verdict::Learned,
        Err(e) => {
            log::warn!("ssh: could not save the host key of {} to known_hosts", host_port(host, port));
            let entry = host_port(host, port);
            Verdict::Trusted {
                unsaved: Some(format!("The host key of {entry} is trusted for this connection only: saving it to {} failed ({e}).", f.display())),
            }
        }
    }
}

/// russh's host key algorithms, with the types `file` records for this host first (as OpenSSH
/// does), so a server with several host keys presents the one already known instead of asking
/// about another. Without a file, or when it cannot be read, the default order.
pub(crate) fn preferred_key_algorithms(host: &str, port: u16, file: Option<&Path>) -> Vec<Algorithm> {
    let default: Vec<Algorithm> = Preferred::DEFAULT.key.to_vec();
    let recorded = match file.map(|f| entries_for(host, port, f)) {
        Some(Ok(entries)) => entries,
        _ => return default,
    };
    let mut order: Vec<Algorithm> = Vec::with_capacity(default.len());
    for entry in recorded.iter().filter(|e| e.marker == Marker::None) {
        let alg = entry.key.algorithm();
        for a in &default {
            let same = *a == alg || (matches!(a, Algorithm::Rsa { .. }) && matches!(alg, Algorithm::Rsa { .. }));
            if same && !order.contains(a) {
                order.push(a.clone());
            }
        }
    }
    for a in default {
        if !order.contains(&a) {
            order.push(a);
        }
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbee_testkit::temp_home;

    const K1: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIAJ8AonQpGdILvxzaGZgY9v2ImqeHwRkUXrAJ3pR4KLE";
    const K1_FP: &str = "SHA256:LL6CACwW9o4ABCBREsVMKKD+US+WvvnaxKCsi9qONWo";
    const K2: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIMuChpSM3ce/aKy093h/xziCZE2hxztZACerKdv4BrkL";
    const ECDSA: &str = "AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBEgM887NED7wEb/MfE1ApBUZTo5U5rrM+GKiL5PUNYDLkBOGN04gQ3YHkVjeKWfn3h/SpPODL2xHfQ5A+U7EeiE=";

    #[test]
    fn host_names_as_known_hosts_writes_them() {
        assert_eq!(host_port("db.example.com", 22), "db.example.com");
        assert_eq!(host_port("127.0.0.1", 2222), "[127.0.0.1]:2222");
    }

    #[test]
    fn fingerprint_and_type_of_a_key() {
        let k = parse_public_key_base64(K1).unwrap();
        assert_eq!(fingerprint(&k), K1_FP);
        assert_eq!(key_type(&k), "ssh-ed25519");
    }

    #[test]
    fn known_unknown_learned_changed() {
        let home = temp_home(None, None);
        let file = home.ssh_dir().join("known_hosts");
        let k1 = parse_public_key_base64(K1).unwrap();
        let unknown = Verdict::Unknown { key_type: "ssh-ed25519".into(), fingerprint: K1_FP.into() };

        // No file: unknown, and a wrong or missing trust learns nothing.
        assert_eq!(check("h", 2222, &k1, Some(&file), None), unknown);
        assert_eq!(check("h", 2222, &k1, Some(&file), Some("SHA256:something-else")), unknown);
        assert!(!file.exists());
        // No ssh_dir at all: unknown; trusting it works for this session but nothing is written.
        assert_eq!(check("h", 2222, &k1, None, None), unknown);
        assert_eq!(check("h", 2222, &k1, None, Some(K1_FP)), Verdict::Trusted { unsaved: None });

        // The accepted fingerprint is learned, then known.
        assert_eq!(check("h", 2222, &k1, Some(&file), Some(K1_FP)), Verdict::Learned);
        // russh starts a new file with an empty line.
        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(text.trim_start_matches('\n'), format!("[h]:2222 ssh-ed25519 {K1}\n"));
        assert_eq!(check("h", 2222, &k1, Some(&file), None), Verdict::Known);
        // Another port of the same host is a different entry.
        assert!(matches!(check("h", 22, &k1, Some(&file), None), Verdict::Unknown { .. }));

        // A different key of the same type is a change, never overridable. The line number counts
        // comment lines too, so it points at the line to remove.
        let changed = format!("# comment\n\nother ssh-ed25519 {K1}\n[h]:2222 ssh-ed25519 {K2}\n");
        std::fs::write(&file, &changed).unwrap();
        assert_eq!(check("h", 2222, &k1, Some(&file), None), Verdict::Changed { line: 4 });
        assert_eq!(check("h", 2222, &k1, Some(&file), Some(K1_FP)), Verdict::Changed { line: 4 });
        assert_eq!(std::fs::read_to_string(&file).unwrap(), changed);

        // A recorded key of another type only: refused, whatever `trust` says, and nothing is learned.
        let other = format!("[h]:2222 ecdsa-sha2-nistp256 {ECDSA}\n");
        std::fs::write(&file, &other).unwrap();
        let refused = Verdict::OtherType { key_type: "ssh-ed25519".into(), recorded: vec!["ecdsa-sha2-nistp256".into()], lines: vec![1] };
        assert_eq!(check("h", 2222, &k1, Some(&file), None), refused);
        assert_eq!(check("h", 2222, &k1, Some(&file), Some(K1_FP)), refused);
        assert!(!refused.accepts());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), other);

        // A broken line for this host is a known_hosts problem.
        std::fs::write(&file, "[h]:2222 ssh-ed25519 !!!\n").unwrap();
        assert!(matches!(check("h", 2222, &k1, Some(&file), None), Verdict::Failed(_)));
    }

    #[test]
    fn the_file_is_read_as_openssh_reads_it() {
        let home = temp_home(None, None);
        let file = home.ssh_dir().join("known_hosts");
        let k1 = parse_public_key_base64(K1).unwrap();
        let k2 = parse_public_key_base64(K2).unwrap();
        let verdict = |text: String, key: &PublicKey| {
            std::fs::write(&file, text).unwrap();
            check("h", 2222, key, Some(&file), None)
        };
        // Tabs separate the fields; the line number is the file's.
        assert_eq!(verdict(format!("# c\n\n[h]:2222\tssh-ed25519\t{K2}\n"), &k1), Verdict::Changed { line: 3 });
        assert_eq!(verdict(format!("[h]:2222\tssh-ed25519\t{K1}\n"), &k1), Verdict::Known);
        assert_eq!(verdict(format!("  [h]:2222 \t ssh-ed25519 {K1} a comment\r\n"), &k1), Verdict::Known);
        // A revoked key is refused, with or without a trusted fingerprint; others are not revoked.
        let revoked = format!("@revoked [h]:2222 ssh-ed25519 {K1}\n");
        assert_eq!(verdict(revoked.clone(), &k1), Verdict::Revoked { line: 1 });
        std::fs::write(&file, &revoked).unwrap();
        assert_eq!(check("h", 2222, &k1, Some(&file), Some(K1_FP)), Verdict::Revoked { line: 1 });
        assert_eq!(std::fs::read_to_string(&file).unwrap(), revoked);
        assert!(matches!(verdict(format!("@revoked\t[h]:2222\tssh-ed25519\t{K1}\n"), &k1), Verdict::Revoked { .. }));
        assert!(matches!(verdict(revoked, &k2), Verdict::Unknown { .. }));
        // A certificate authority is no host key: the line neither vouches for a key nor changes it.
        assert!(matches!(verdict(format!("@cert-authority [h]:2222 ssh-ed25519 {K2}\n"), &k1), Verdict::Unknown { .. }));
        // Names compare without regard to case, with wildcards and exclusions.
        assert_eq!(verdict(format!("[H]:2222 ssh-ed25519 {K1}\n"), &k1), Verdict::Known);
        assert_eq!(verdict(format!("[*]:2222 ssh-ed25519 {K1}\n"), &k1), Verdict::Known);
        assert_eq!(verdict(format!("[?]:2222,other ssh-ed25519 {K1}\n"), &k1), Verdict::Known);
        assert!(matches!(verdict(format!("*,![h]:2222 ssh-ed25519 {K1}\n"), &k1), Verdict::Unknown { .. }));
        // Hashed names (the salt and hash of `[h]:2222`).
        let mac = {
            let salt = b"0123456789abcdefghij";
            let mut m = Hmac::<Sha1>::new_from_slice(salt).unwrap();
            m.update(b"[h]:2222");
            format!("|1|{}|{}", data_encoding::BASE64.encode(salt), data_encoding::BASE64.encode(&m.finalize().into_bytes()))
        };
        assert_eq!(verdict(format!("{mac} ssh-ed25519 {K1}\n"), &k1), Verdict::Known);
        assert_eq!(verdict(format!("{mac} ssh-ed25519 {K2}\n"), &k1), Verdict::Changed { line: 1 });
        // Two keys of one type recorded (a rotation): the one shown is known.
        assert_eq!(verdict(format!("[h]:2222 ssh-ed25519 {K2}\n[h]:2222 ssh-ed25519 {K1}\n"), &k1), Verdict::Known);
        // A line for another host is none of this host's business, however broken.
        assert!(matches!(verdict("other ssh-ed25519\n@odd x y z\n@revoked\n".into(), &k1), Verdict::Unknown { .. }));
        // The wildcard matcher stays quick on a hostile pattern.
        assert!(!wildcard(&"*a".repeat(2000), &"b".repeat(4000)));
    }

    #[test]
    fn a_host_known_hosts_cannot_hold_is_refused_before_anything_is_written() {
        let home = temp_home(None, None);
        let file = home.ssh_dir().join("known_hosts");
        let k1 = parse_public_key_base64(K1).unwrap();
        for host in ["a,b", "a b", "a\nb", "#a", "[a]", ""] {
            let v = check(host, 2222, &k1, Some(&file), Some(K1_FP));
            assert!(matches!(v, Verdict::Failed(_)), "{host:?}: {v:?}");
        }
        assert!(!file.exists());
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_file_fails_and_a_failed_save_says_why() {
        use std::os::unix::fs::PermissionsExt;
        let home = temp_home(None, None);
        let file = home.ssh_dir().join("known_hosts");
        let k1 = parse_public_key_base64(K1).unwrap();
        // It records another key for the host: read as empty, that change would pass as unknown.
        std::fs::write(&file, format!("[h]:2222 ssh-ed25519 {K2}\n")).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read(&file).is_ok() {
            return; // root reads anything
        }
        for trust in [None, Some(K1_FP)] {
            let v = check("h", 2222, &k1, Some(&file), trust);
            assert!(matches!(&v, Verdict::Failed(m) if m.contains("known_hosts cannot be read")), "{v:?}");
        }
        // Readable but not writable: trusted for this connection, and the verdict says why it was
        // not saved.
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444)).unwrap();
        let v = check("h", 22, &k1, Some(&file), Some(K1_FP));
        let Verdict::Trusted { unsaved: Some(why) } = &v else { panic!("{v:?}") };
        assert!(
            why.starts_with("The host key of h is trusted for this connection only: saving it to ") && why.contains("known_hosts failed"),
            "{why}"
        );
        assert!(v.accepts());
    }

    #[test]
    fn recorded_key_types_come_first() {
        use russh::keys::{Algorithm, EcdsaCurve};
        let home = temp_home(None, None);
        let file = home.ssh_dir().join("known_hosts");
        let default = preferred_key_algorithms("h", 2222, Some(&file));
        assert_eq!(default.first(), Some(&Algorithm::Ed25519));
        assert_eq!(default, preferred_key_algorithms("h", 2222, None));

        std::fs::write(&file, format!("[h]:2222 ecdsa-sha2-nistp256 {ECDSA}\nother ssh-ed25519 {K1}\n")).unwrap();
        let algs = preferred_key_algorithms("h", 2222, Some(&file));
        assert_eq!(algs.first(), Some(&Algorithm::Ecdsa { curve: EcdsaCurve::NistP256 }));
        // Same set, just reordered.
        let (mut a, mut b) = (algs.clone(), default.clone());
        a.sort_by_key(|x| x.to_string());
        b.sort_by_key(|x| x.to_string());
        assert_eq!(a, b);
        // Other hosts keep the default order; a broken file does too.
        assert_eq!(preferred_key_algorithms("other", 22, Some(&file)), default);
        std::fs::write(&file, "[h]:2222 ssh-ed25519 !!!\n").unwrap();
        assert_eq!(preferred_key_algorithms("h", 2222, Some(&file)), default);
    }
}
