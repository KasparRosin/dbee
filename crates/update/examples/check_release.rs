//! Checks a folder of release files before the release is drafted: `SHA256SUMS.txt` is a manifest
//! the app accepts, of the version being released, and with a key pinned in this tree,
//! `SHA256SUMS.txt.sig` is a signature by it. The keys are this release's own: a copy in use holds
//! the keys of the release it was built from, which this cannot see. Run by
//! `.github/actions/draft-release`:
//!
//! ```sh
//! cargo run -p dbee-update --example check_release -- artifacts 0.5.0
//! ```
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::path::Path;
use std::process::ExitCode;

use dbee_update::{MANIFEST_NAME, Manifest, PINNED_KEYS, ReleaseCheck, SIGNATURE_NAME, check_release};

fn main() -> ExitCode {
    let Some(dir) = std::env::args_os().nth(1) else {
        eprintln!("usage: check_release <folder of release files> [version]");
        return ExitCode::from(2);
    };
    let version = std::env::args().nth(2);
    match check(Path::new(&dir), version.as_deref()) {
        Ok(found) => {
            println!("{found}");
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("error: {why}");
            ExitCode::FAILURE
        }
    }
}

fn check(dir: &Path, version: Option<&str>) -> Result<String, String> {
    let manifest_path = dir.join(MANIFEST_NAME);
    let bytes = std::fs::read(&manifest_path).map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let manifest = Manifest::parse(&bytes).map_err(|e| e.to_string())?;
    if let Some(version) = version
        && manifest.version.to_string() != version
    {
        return Err(format!("{MANIFEST_NAME} lists files of {}, and the release is {version}", manifest.version));
    }
    let signature_path = dir.join(SIGNATURE_NAME);
    let signature = match std::fs::read(&signature_path) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("{}: {e}", signature_path.display())),
    };
    let files = if manifest.assets.len() == 1 { "file" } else { "files" };
    let listed = format!("{MANIFEST_NAME} lists {} {files} of {}", manifest.assets.len(), manifest.version);
    match check_release(&bytes, signature.as_deref(), PINNED_KEYS).map_err(|e| e.to_string())? {
        ReleaseCheck::Signed => Ok(format!("{listed}; {SIGNATURE_NAME} is by a key this release pins")),
        ReleaseCheck::NoKey => {
            let has = if signature.is_some() { "is there but was not checked" } else { "is not there" };
            println!("::warning::No release key is pinned in the app (crates/update/src/signature.rs), so no copy will install this release itself.");
            Ok(format!("{listed}; {SIGNATURE_NAME} {has}"))
        }
    }
}
