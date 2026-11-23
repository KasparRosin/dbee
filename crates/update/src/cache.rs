//! The last check's time and answer, kept so a restart does not ask again within [`FRESH`], and
//! when the next check is due.
//!
//! The file is `{"version": 1, "checkedAt": <seconds since 1970>, "latest": "0.5.0"}`. It is read
//! as hostile input: anything that is not this shape, or a time in the future, counts as no
//! file. Reading and writing the file is the caller's.

use std::time::Duration;

use semver::Version;

use crate::version::parse_version;

/// The file's name in the settings folder.
pub const CACHE_FILE: &str = "update-check.json";

/// The most of the file that is read.
pub const MAX_CACHE: usize = 4 * 1024;

const CACHE_VERSION: u64 = 1;

/// How long after the start the first check runs.
pub const FIRST_CHECK_DELAY: Duration = Duration::from_secs(5);

/// How long an answer is good for across restarts.
pub const FRESH: Duration = Duration::from_secs(6 * 60 * 60);

/// How long after a check the next one runs while the app stays open.
pub const RECHECK: Duration = Duration::from_secs(24 * 60 * 60);

/// What the last check found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cache {
    /// When it ran, in seconds since 1970.
    pub checked_at: u64,
    /// The newest release then.
    pub latest: Version,
}

impl Cache {
    /// Reads the file's bytes; `None` for anything that is not a cache file of this version.
    pub fn parse(bytes: &[u8]) -> Option<Cache> {
        if bytes.len() > MAX_CACHE {
            return None;
        }
        let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
        let map = value.as_object()?;
        if map.get("version")?.as_u64()? != CACHE_VERSION {
            return None;
        }
        let checked_at = map.get("checkedAt")?.as_u64()?;
        let latest = parse_version(map.get("latest")?.as_str()?).ok()?;
        Some(Cache { checked_at, latest })
    }

    /// The file's bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes =
            serde_json::json!({ "version": CACHE_VERSION, "checkedAt": self.checked_at, "latest": self.latest.to_string() }).to_string().into_bytes();
        bytes.push(b'\n');
        bytes
    }

    /// Whether the answer still stands at `now` (seconds since 1970): it is younger than
    /// [`FRESH`] and not from the future.
    pub fn is_fresh(&self, now: u64) -> bool {
        now.checked_sub(self.checked_at).is_some_and(|age| age < FRESH.as_secs())
    }
}

/// How long after a start at `now` the first check runs: [`FIRST_CHECK_DELAY`], or, while the
/// last answer is fresh, once it no longer is.
pub fn first_check_in(cache: Option<&Cache>, now: u64) -> Duration {
    let wait = cache.filter(|c| c.is_fresh(now)).and_then(|c| now.checked_sub(c.checked_at)).map(|age| FRESH.as_secs().saturating_sub(age));
    wait.map_or(FIRST_CHECK_DELAY, |secs| Duration::from_secs(secs).max(FIRST_CHECK_DELAY))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const HOUR: u64 = 60 * 60;

    fn cache(checked_at: u64) -> Cache {
        Cache { checked_at, latest: Version::new(0, 5, 0) }
    }

    #[test]
    fn the_file_comes_back_as_written() {
        let c = cache(1_800_000_000);
        assert_eq!(Cache::parse(&c.to_bytes()), Some(c.clone()));
        assert_eq!(String::from_utf8(c.to_bytes()).unwrap(), "{\"version\":1,\"checkedAt\":1800000000,\"latest\":\"0.5.0\"}\n");
    }

    #[test]
    fn anything_else_is_no_cache() {
        for text in [
            "",
            "garbage{",
            "[]",
            "null",
            r#"{"version":2,"checkedAt":1,"latest":"0.5.0"}"#,
            r#"{"checkedAt":1,"latest":"0.5.0"}"#,
            r#"{"version":1,"latest":"0.5.0"}"#,
            r#"{"version":1,"checkedAt":-1,"latest":"0.5.0"}"#,
            r#"{"version":1,"checkedAt":1.5,"latest":"0.5.0"}"#,
            r#"{"version":1,"checkedAt":"1","latest":"0.5.0"}"#,
            r#"{"version":1,"checkedAt":1}"#,
            r#"{"version":1,"checkedAt":1,"latest":5}"#,
            r#"{"version":1,"checkedAt":1,"latest":"v0.5.0"}"#,
            r#"{"version":1,"checkedAt":1,"latest":"https://example.test/"}"#,
        ] {
            assert_eq!(Cache::parse(text.as_bytes()), None, "{text}");
        }
        let big = format!(r#"{{"version":1,"checkedAt":1,"latest":"0.5.0"{}}}"#, " ".repeat(MAX_CACHE));
        assert_eq!(Cache::parse(big.as_bytes()), None);
    }

    #[test]
    fn an_answer_is_fresh_for_six_hours_and_never_from_the_future() {
        let now = 1_800_000_000;
        assert!(cache(now).is_fresh(now));
        assert!(cache(now - 6 * HOUR + 1).is_fresh(now));
        assert!(!cache(now - 6 * HOUR).is_fresh(now));
        assert!(!cache(now + 1).is_fresh(now));
        assert!(!cache(u64::MAX).is_fresh(now));
        assert!(!cache(0).is_fresh(now));
    }

    #[test]
    fn the_first_check_waits_out_a_fresh_answer() {
        let now = 1_800_000_000;
        assert_eq!(first_check_in(None, now), FIRST_CHECK_DELAY);
        assert_eq!(first_check_in(Some(&cache(now - 7 * HOUR)), now), FIRST_CHECK_DELAY);
        assert_eq!(first_check_in(Some(&cache(now + 100)), now), FIRST_CHECK_DELAY, "a time in the future is no answer");
        assert_eq!(first_check_in(Some(&cache(now - HOUR)), now), Duration::from_secs(5 * HOUR));
        assert_eq!(first_check_in(Some(&cache(now)), now), FRESH);
        assert_eq!(first_check_in(Some(&cache(now - 6 * HOUR + 1)), now), FIRST_CHECK_DELAY);
    }
}
