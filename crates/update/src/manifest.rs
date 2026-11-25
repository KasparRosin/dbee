//! A release's `SHA256SUMS.txt`, read as hostile input.
//!
//! The file is what `sha256sum` writes over a release's assets: one line per file, 64 hex digits,
//! two spaces and the file's name. Every name must be an asset name,
//! `dbee(-cli)?-<semver>-<platform>-<arch>[-portable].<ext>`, all of one version, each once. The
//! platform, the architecture and the extension are read by their shape, not from a fixed list,
//! so a copy still reads the manifest of a later release that adds a platform. A line naming the
//! manifest itself is passed over.

use semver::Version;

use crate::UpdateError;
use crate::version::parse_version;

/// The manifest's file name in a release.
pub const MANIFEST_NAME: &str = "SHA256SUMS.txt";

/// The largest manifest read, in bytes.
pub const MAX_MANIFEST: usize = 64 * 1024;

/// The most lines a manifest may have.
pub const MAX_LINES: usize = 256;

/// The longest line, in bytes.
const MAX_LINE: usize = 256;

/// One file of a release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    /// The file's name, which matched the asset grammar.
    pub name: String,
    pub sha256: [u8; 32],
}

/// What a manifest says: the release's version and its files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub version: Version,
    pub assets: Vec<Asset>,
}

impl Manifest {
    /// Reads `bytes`, refusing anything that is not a release's manifest.
    pub fn parse(bytes: &[u8]) -> Result<Manifest, UpdateError> {
        if bytes.len() > MAX_MANIFEST {
            return Err(bad("it is larger than 64 KiB"));
        }
        let text = std::str::from_utf8(bytes).map_err(|_| bad("it is not text"))?;
        let text = text.strip_suffix('\n').unwrap_or(text);
        let mut version: Option<Version> = None;
        let mut assets: Vec<Asset> = Vec::new();
        for (n, line) in text.split('\n').enumerate() {
            if n >= MAX_LINES {
                return Err(bad("it has more than 256 lines"));
            }
            if line.len() > MAX_LINE {
                return Err(bad("a line is too long"));
            }
            let (hex, name) = line.split_once("  ").ok_or_else(|| bad("a line is not a checksum, two spaces and a file name"))?;
            let sha256 = sha256_from_hex(hex).ok_or_else(|| bad("a checksum is not 64 hex digits"))?;
            if name == MANIFEST_NAME {
                continue;
            }
            let named = asset_version(name).ok_or_else(|| bad("a file name is not an asset's"))?;
            match &version {
                Some(v) if *v != named => return Err(bad("its files are of more than one version")),
                Some(_) => {}
                None => version = Some(named),
            }
            if assets.iter().any(|a| a.name == name) {
                return Err(bad("a file is listed twice"));
            }
            assets.push(Asset { name: name.to_string(), sha256 });
        }
        let version = version.ok_or_else(|| bad("it lists no file"))?;
        Ok(Manifest { version, assets })
    }

    /// The file of this name, when the release has it.
    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == name)
    }
}

fn bad(why: &str) -> UpdateError {
    UpdateError::Manifest(why.to_string())
}

/// 64 hex digits as 32 bytes.
fn sha256_from_hex(hex: &str) -> Option<[u8; 32]> {
    let digits = hex.as_bytes();
    if digits.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (byte, [high, low]) in out.iter_mut().zip(digits.as_chunks::<2>().0) {
        let high = char::from(*high).to_digit(16)?;
        let low = char::from(*low).to_digit(16)?;
        *byte = u8::try_from(high.checked_mul(16)?.checked_add(low)?).ok()?;
    }
    Some(out)
}

/// The version an asset name carries, when `name` is one:
/// `dbee(-cli)?-<semver>-<platform>-<arch>[-portable].<ext>`, read from its end, since a
/// pre-release version holds hyphens and dots of its own.
pub(crate) fn asset_version(name: &str) -> Option<Version> {
    let rest = name.strip_prefix("dbee-")?;
    let rest = rest.strip_prefix("cli-").unwrap_or(rest);
    // The last hyphen is followed by `<arch>.<ext>`, or by `portable.<ext>`.
    let (rest, last) = rest.rsplit_once('-')?;
    let (word, ext) = last.split_once('.')?;
    let (rest, arch) = if word == "portable" { rest.rsplit_once('-')? } else { (rest, word) };
    let (version, platform) = rest.rsplit_once('-')?;
    let lower_or_digit = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    let part = |s: &str, extra: &[char]| !s.is_empty() && s.len() <= 16 && s.chars().all(|c| lower_or_digit(c) || extra.contains(&c));
    if !part(platform, &[]) || !part(arch, &['_']) {
        return None;
    }
    // `tar.gz`, `AppImage.zsync`: letters and digits, with single dots between them.
    let ext_ok = ext.len() <= 24 && ext.split('.').all(|piece| !piece.is_empty() && piece.chars().all(|c| c.is_ascii_alphanumeric()));
    if !ext_ok {
        return None;
    }
    parse_version(version).ok()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
pub(crate) mod tests {
    use super::*;

    const HASH: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";

    /// Every asset name a release has (docs/releasing.md, "What gets built"), for version `v`.
    pub(crate) fn release_names(v: &str) -> Vec<String> {
        let mut names = vec![format!("dbee-{v}-macos-universal.dmg"), format!("dbee-cli-{v}-macos-universal.zip")];
        for arch in ["x64", "arm64", "x86"] {
            names.push(format!("dbee-{v}-windows-{arch}.msi"));
            names.push(format!("dbee-{v}-windows-{arch}-portable.zip"));
        }
        for arch in ["x86_64", "aarch64"] {
            for ext in ["AppImage", "AppImage.zsync", "flatpak", "deb", "rpm", "tar.gz"] {
                names.push(format!("dbee-{v}-linux-{arch}.{ext}"));
            }
        }
        for ext in ["deb", "rpm", "tar.gz"] {
            names.push(format!("dbee-{v}-linux-riscv64.{ext}"));
        }
        names.push(format!("dbee-{v}-freebsd-x86_64.tar.gz"));
        names
    }

    pub(crate) fn manifest_text(v: &str) -> String {
        release_names(v).iter().map(|name| format!("{HASH}  {name}\n")).collect()
    }

    fn refused(text: &str) -> String {
        match Manifest::parse(text.as_bytes()) {
            Err(UpdateError::Manifest(why)) => why,
            other => panic!("not refused: {other:?}"),
        }
    }

    #[test]
    fn a_releases_manifest_is_read_whole() {
        let m = Manifest::parse(manifest_text("0.5.0").as_bytes()).unwrap();
        assert_eq!(m.version, Version::new(0, 5, 0));
        assert_eq!(m.assets.len(), 24);
        assert_eq!(m.assets.iter().map(|a| a.name.clone()).collect::<Vec<_>>(), release_names("0.5.0"));
        let deb = m.asset("dbee-0.5.0-linux-x86_64.deb").unwrap();
        assert_eq!(deb.sha256[0], 0x9f);
        assert_eq!(deb.sha256[31], 0x08);
        assert!(m.asset("dbee-0.5.0-linux-x86_64.exe").is_none());
    }

    #[test]
    fn a_pre_releases_names_are_read() {
        let m = Manifest::parse(manifest_text("0.0.1-rc.1").as_bytes()).unwrap();
        assert_eq!(m.version, Version::parse("0.0.1-rc.1").unwrap());
        assert_eq!(m.assets.len(), 24);
        assert_eq!(asset_version("dbee-1.0.0-beta-2.x-linux-x86_64.deb"), Version::parse("1.0.0-beta-2.x").ok());
    }

    #[test]
    fn the_manifests_own_line_and_a_missing_last_newline_are_fine() {
        let text = format!("{HASH}  SHA256SUMS.txt\n{HASH}  dbee-0.5.0-linux-x86_64.deb");
        let m = Manifest::parse(text.as_bytes()).unwrap();
        assert_eq!(m.assets.len(), 1);
    }

    #[test]
    fn a_later_releases_new_platform_is_read() {
        let text = format!("{HASH}  dbee-0.9.0-linux-x86_64.deb\n{HASH}  dbee-0.9.0-openbsd-arm64.pkg.tar.zst\n");
        assert_eq!(Manifest::parse(text.as_bytes()).unwrap().assets.len(), 2);
    }

    #[test]
    fn size_and_line_caps_hold() {
        let line = format!("{HASH}  dbee-0.5.0-linux-x86_64.deb\n");
        assert_eq!(refused(&line.repeat(1000)), "it is larger than 64 KiB");
        let many: String = (0..257).map(|n| format!("{HASH}  dbee-0.5.0-linux-a{n}.deb\n")).collect();
        assert!(many.len() <= MAX_MANIFEST);
        assert_eq!(refused(&many), "it has more than 256 lines");
        let exactly: String = (0..256).map(|n| format!("{HASH}  dbee-0.5.0-linux-a{n}.deb\n")).collect();
        assert_eq!(Manifest::parse(exactly.as_bytes()).unwrap().assets.len(), 256);
        assert_eq!(refused(&format!("{HASH}  dbee-0.5.0-linux-x86_64.{}\n", "a.".repeat(150))), "a line is too long");
        assert!(Manifest::parse(&vec![b'a'; MAX_MANIFEST + 1]).is_err());
    }

    #[test]
    fn lines_that_are_no_checksum_lines_are_refused() {
        let name = "dbee-0.5.0-linux-x86_64.deb";
        for text in [
            String::new(),
            "\n".to_string(),
            format!("{HASH} {name}\n"),
            format!("{HASH} *{name}\n"),
            format!("{HASH}\t{name}\n"),
            format!("{}  {name}\n", &HASH[..63]),
            format!("{HASH}0  {name}\n"),
            format!("{}g  {name}\n", &HASH[..63]),
            format!("+{}  {name}\n", &HASH[..63]),
            format!("{HASH}  {name}\r\n"),
            format!("{HASH}  {name}\n\n"),
            format!("\n{HASH}  {name}\n"),
            format!("{HASH}  {name}\n{HASH}"),
            format!("{HASH}   {name}\n"),
            format!("{HASH}  {name} \n"),
            "<html><body>Not Found</body></html>".to_string(),
        ] {
            assert!(Manifest::parse(text.as_bytes()).is_err(), "{text:?}");
        }
        assert!(Manifest::parse(&[0xff, 0xfe, 0x00]).is_err());
        assert_eq!(refused(&format!("{HASH}  SHA256SUMS.txt\n")), "it lists no file");
    }

    #[test]
    fn names_outside_the_grammar_are_refused() {
        for name in [
            "dbee.deb",
            "dbee-0.5.0.deb",
            "dbee-0.5.0-linux.deb",
            "dbee-0.5.0-linux-x86_64",
            "dbee-0.5.0-linux-x86_64.",
            "dbee-0.5.0-linux-x86_64..deb",
            "dbee-0.5.0-linux-x86_64.tar..gz",
            "dbee-0.5-linux-x86_64.deb",
            "dbee-v0.5.0-linux-x86_64.deb",
            "dbee-0.5.0-Linux-x86_64.deb",
            "dbee-0.5.0-linux-X86.deb",
            "dbee-0.5.0-linux-x86_64.d/b",
            "dbee-0.5.0-linux-x86_64.deb?x=1",
            "dbee-0.5.0-linux-x86 64.deb",
            "../dbee-0.5.0-linux-x86_64.deb",
            "dbee-0.5.0-linux-../x.deb",
            "dbee-0.5.0-linux-x86_64-portable",
            "dbee-0.5.0-linux--portable.zip",
            "other-0.5.0-linux-x86_64.deb",
            "dbee-gui-0.5.0-linux-x86_64.deb",
            "dbee-0.5.0-linux-x86_64.deb\u{0}",
            "dbee-0.5.0-linux-x86_64.dêb",
            "https://example.test/dbee-0.5.0-linux-x86_64.deb",
            "",
        ] {
            assert!(asset_version(name).is_none(), "{name:?}");
            assert_eq!(refused(&format!("{HASH}  {name}\n")), "a file name is not an asset's", "{name:?}");
        }
    }

    #[test]
    fn two_versions_or_a_file_twice_are_refused() {
        let text = format!("{HASH}  dbee-0.5.0-linux-x86_64.deb\n{HASH}  dbee-0.5.1-linux-x86_64.rpm\n");
        assert_eq!(refused(&text), "its files are of more than one version");
        let text = format!("{HASH}  dbee-0.5.0-linux-x86_64.deb\n{HASH}  dbee-0.5.0-linux-x86_64.deb\n");
        assert_eq!(refused(&text), "a file is listed twice");
    }

    #[test]
    fn checksums_read_in_either_case() {
        let upper = HASH.to_uppercase();
        let m = Manifest::parse(format!("{upper}  dbee-0.5.0-linux-x86_64.deb\n").as_bytes()).unwrap();
        assert_eq!(m.assets[0].sha256[0], 0x9f);
    }
}
