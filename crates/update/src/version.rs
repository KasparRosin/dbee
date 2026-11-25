//! Which release a copy is offered: only one strictly newer than itself, and never a pre-release.

use semver::Version;

use crate::UpdateError;

/// The longest version text read.
const MAX_VERSION: usize = 64;

/// A version as releases write it: semver, in its canonical spelling (`0.5.0`, `0.5.0-rc.1`).
pub fn parse_version(text: &str) -> Result<Version, UpdateError> {
    if text.len() > MAX_VERSION {
        return Err(UpdateError::Manifest("a version is too long".into()));
    }
    let version = Version::parse(text).map_err(|_| UpdateError::Manifest("a version is not semver".into()))?;
    if version.to_string() != text {
        return Err(UpdateError::Manifest("a version is not written as semver writes it".into()));
    }
    Ok(version)
}

/// Whether a copy at `current` is offered `latest`: `latest` is a final release and comes after
/// `current` in semver's precedence (so a pre-release copy is offered its final release, and
/// build metadata counts for nothing).
pub fn is_newer(current: &Version, latest: &Version) -> bool {
    latest.pre.is_empty() && latest.cmp_precedence(current) == std::cmp::Ordering::Greater
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn only_a_strictly_newer_release_is_offered() {
        for (current, latest, offered) in [
            ("0.3.0", "0.3.1", true),
            ("0.3.0", "0.4.0", true),
            ("0.3.0", "1.0.0", true),
            ("0.9.0", "0.10.0", true),
            ("0.3.0", "0.3.0", false),
            ("0.3.1", "0.3.0", false),
            ("1.0.0", "0.9.9", false),
        ] {
            assert_eq!(is_newer(&v(current), &v(latest)), offered, "{current} -> {latest}");
        }
    }

    #[test]
    fn a_pre_release_is_never_offered() {
        assert!(!is_newer(&v("0.3.0"), &v("0.4.0-rc.1")));
        assert!(!is_newer(&v("0.4.0-rc.1"), &v("0.4.0-rc.2")));
        assert!(!is_newer(&v("0.3.0"), &v("1.0.0-alpha")));
    }

    #[test]
    fn a_pre_release_copy_is_offered_its_final_release() {
        assert!(is_newer(&v("0.4.0-rc.1"), &v("0.4.0")));
        assert!(is_newer(&v("0.4.0-rc.1"), &v("0.4.1")));
        assert!(!is_newer(&v("0.4.1-rc.1"), &v("0.4.0")));
    }

    #[test]
    fn build_metadata_counts_for_nothing() {
        assert!(!is_newer(&v("0.3.0"), &v("0.3.0+build.7")));
        assert!(!is_newer(&v("0.3.0+a"), &v("0.3.0+b")));
        assert!(is_newer(&v("0.3.0+z"), &v("0.3.1+a")));
    }

    #[test]
    fn versions_are_read_strictly() {
        assert_eq!(parse_version("0.5.0").unwrap(), v("0.5.0"));
        assert_eq!(parse_version("0.0.1-rc.1").unwrap(), v("0.0.1-rc.1"));
        for text in ["", "v0.5.0", "0.5", "0.5.0 ", " 0.5.0", "01.5.0", "0.5.0\n", "latest", "0.5.0-", "١.٢.٣"] {
            assert!(parse_version(text).is_err(), "{text:?}");
        }
        assert!(parse_version(&format!("1.0.0-{}", "a".repeat(80))).is_err());
    }
}
