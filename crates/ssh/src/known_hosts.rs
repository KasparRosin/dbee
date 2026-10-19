//! Host key check against `known_hosts`: known, unknown (asks once and can be learned) or
//! changed (refused, never overridable).

use std::path::Path;

use russh::Preferred;
use russh::keys::known_hosts::{known_host_keys_path, learn_known_hosts_path};
use russh::keys::{Algorithm, Error as KeyError, HashAlg, PublicKey, check_known_hosts_path};

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
    /// writing `known_hosts` failed.
    Trusted,
    /// Not recorded and not (or not this key) accepted.
    Unknown { key_type: String, fingerprint: String },
    /// Another key of the same type is recorded at this 1-based line of the file.
    Changed { line: usize },
    /// `known_hosts` could not be read, or a line for this host is malformed.
    Failed(String),
    /// The server offered something this client does not check (a host certificate).
    Unsupported(String),
}

impl Verdict {
    pub(crate) fn accepts(&self) -> bool {
        matches!(self, Verdict::Known | Verdict::Learned | Verdict::Trusted)
    }
}

/// Check `key` for `host:port` against `file`; with `trust` equal to the key's fingerprint an
/// unknown key is learned. A changed key is refused whatever `trust` says.
pub(crate) fn check(host: &str, port: u16, key: &PublicKey, file: Option<&Path>, trust: Option<&str>) -> Verdict {
    let recorded = match file {
        Some(f) => check_known_hosts_path(host, port, key, f),
        None => Ok(false),
    };
    match recorded {
        Ok(true) => Verdict::Known,
        Ok(false) => {
            let fingerprint = fingerprint(key);
            if trust != Some(fingerprint.as_str()) {
                return Verdict::Unknown { key_type: key_type(key), fingerprint };
            }
            let Some(f) = file else { return Verdict::Trusted };
            match learn_known_hosts_path(host, port, key, f) {
                Ok(()) => Verdict::Learned,
                Err(e) => {
                    log::warn!("ssh: could not save the host key of {} to known_hosts: {e}", host_port(host, port));
                    Verdict::Trusted
                }
            }
        }
        Err(KeyError::KeyChanged { line }) => Verdict::Changed { line: file.map_or(line, |f| file_line(f, line)) },
        Err(e) => Verdict::Failed(e.to_string()),
    }
}

/// russh numbers only the lines that do not start with `#`; turn its number into the line number
/// in the file, so the message points at the line to remove.
fn file_line(file: &Path, counted: usize) -> usize {
    let Ok(bytes) = std::fs::read(file) else { return counted };
    let mut seen = 0usize;
    for (i, line) in bytes.split(|b| *b == b'\n').enumerate() {
        if line.first() != Some(&b'#') {
            seen = seen.saturating_add(1);
            if seen == counted {
                return i.saturating_add(1);
            }
        }
    }
    counted
}

/// russh's host key algorithms, with the types `file` records for this host first (as OpenSSH
/// does), so a server with several host keys presents the one already known instead of asking
/// about another. Without a file, or when it cannot be read, the default order.
pub(crate) fn preferred_key_algorithms(host: &str, port: u16, file: Option<&Path>) -> Vec<Algorithm> {
    let default: Vec<Algorithm> = Preferred::DEFAULT.key.to_vec();
    let recorded = match file.map(|f| known_host_keys_path(host, port, f)) {
        Some(Ok(keys)) => keys,
        _ => return default,
    };
    let mut order: Vec<Algorithm> = Vec::with_capacity(default.len());
    for (_, key) in &recorded {
        let alg = key.algorithm();
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
    use russh::keys::parse_public_key_base64;

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
        assert_eq!(check("h", 2222, &k1, None, Some(K1_FP)), Verdict::Trusted);

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

        // A recorded key of another type only: unknown, not changed.
        std::fs::write(&file, format!("[h]:2222 ecdsa-sha2-nistp256 {ECDSA}\n")).unwrap();
        assert_eq!(check("h", 2222, &k1, Some(&file), None), unknown);

        // A broken line for this host is a known_hosts problem.
        std::fs::write(&file, "[h]:2222 ssh-ed25519 !!!\n").unwrap();
        assert!(matches!(check("h", 2222, &k1, Some(&file), None), Verdict::Failed(_)));
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
