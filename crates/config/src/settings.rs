//! The settings file beside the connections file:
//! `{"version": 1, "theme": "dark", "checkForUpdates": false}`.

use std::io::Read;
use std::path::{Path, PathBuf};

use crate::store::{private_dir, replace_private_file};

pub const SETTINGS_FILE: &str = "settings.json";
pub const SETTINGS_VERSION: u32 = 1;
const MAX_SETTINGS: u64 = 64 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeChoice {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        [Self::System, Self::Light, Self::Dark].into_iter().find(|t| t.as_str() == text)
    }
}

/// The file's JSON object; `None` when it is missing, too large or not an object.
fn read_object(dir: &Path) -> Option<serde_json::Map<String, serde_json::Value>> {
    let mut bytes = Vec::new();
    std::fs::File::open(dir.join(SETTINGS_FILE)).ok()?.take(MAX_SETTINGS + 1).read_to_end(&mut bytes).ok()?;
    if u64::try_from(bytes.len()).ok()? > MAX_SETTINGS {
        return None;
    }
    match serde_json::from_slice(&bytes).ok()? {
        serde_json::Value::Object(map) => Some(map),
        _ => None,
    }
}

/// The theme saved in `dir`. Anything missing or unreadable means System. `None` means System.
pub fn load_theme(dir: Option<&Path>) -> ThemeChoice {
    let Some(map) = dir.and_then(read_object) else { return ThemeChoice::System };
    map.get("theme").and_then(|v| v.as_str()).and_then(ThemeChoice::parse).unwrap_or_default()
}

/// Why a choice wasn't saved; each names the settings file.
#[derive(Debug)]
pub enum SaveError {
    /// A newer dbee wrote the file, which is left as it is.
    Newer(PathBuf),
    /// The file, or the folder it goes in, couldn't be written.
    Io(PathBuf, std::io::Error),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Newer(file) => write!(f, "a newer dbee wrote {}, so it isn't changed", file.display()),
            Self::Io(file, e) => write!(f, "could not write {}: {e}", file.display()),
        }
    }
}

impl std::error::Error for SaveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Newer(_) => None,
            Self::Io(_, e) => Some(e),
        }
    }
}

/// Saves it, keeping the file's other fields. `None` writes nothing. A newer dbee's file, or one
/// whose version isn't a whole number, is left alone, and is an error.
pub fn save_theme(dir: Option<&Path>, theme: ThemeChoice) -> Result<(), SaveError> {
    save_field(dir, "theme", theme.as_str().into())
}

/// The field of the automatic update check.
const CHECK_FOR_UPDATES: &str = "checkForUpdates";

/// Whether the settings in `dir` leave the automatic update check on: off only when the file says
/// `"checkForUpdates": false`. Anything missing or unreadable, and `None`, mean on.
pub fn load_update_check(dir: Option<&Path>) -> bool {
    dir.and_then(read_object).and_then(|map| map.get(CHECK_FOR_UPDATES).and_then(serde_json::Value::as_bool)).unwrap_or(true)
}

/// Saves whether the automatic update check is on, as [`save_theme`] saves the theme.
pub fn save_update_check(dir: Option<&Path>, on: bool) -> Result<(), SaveError> {
    save_field(dir, CHECK_FOR_UPDATES, on.into())
}

/// Sets one field of the file, keeping the others.
fn save_field(dir: Option<&Path>, key: &str, value: serde_json::Value) -> Result<(), SaveError> {
    let Some(dir) = dir else { return Ok(()) };
    let file = dir.join(SETTINGS_FILE);
    let mut map = read_object(dir).unwrap_or_default();
    if map.get("version").is_some_and(|v| v.as_u64().is_none_or(|n| n > u64::from(SETTINGS_VERSION))) {
        return Err(SaveError::Newer(file));
    }
    map.insert("version".into(), SETTINGS_VERSION.into());
    map.insert(key.into(), value);
    let written = serde_json::to_vec_pretty(&map).map_err(std::io::Error::other).and_then(|mut bytes| {
        bytes.push(b'\n');
        private_dir(dir)?;
        replace_private_file(&file, &bytes)
    });
    written.map_err(|e| SaveError::Io(file, e))
}

/// The first `max` bytes of the file `name` in `dir`, for a small file of dbee's own kept beside
/// the settings (the update check's last answer); `None` when it is missing, unreadable or
/// larger.
pub fn read_small_file(dir: &Path, name: &str, max: u64) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(dir.join(name)).ok()?.take(max.checked_add(1)?).read_to_end(&mut bytes).ok()?;
    (u64::try_from(bytes.len()).ok()? <= max).then_some(bytes)
}

/// Writes the file `name` in `dir` whole, private to this user, as the settings file is written.
pub fn write_small_file(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    private_dir(dir)?;
    replace_private_file(&dir.join(name), bytes)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn write(dir: &Path, text: &str) {
        std::fs::write(dir.join(SETTINGS_FILE), text).unwrap();
    }

    fn read(dir: &Path) -> String {
        std::fs::read_to_string(dir.join(SETTINGS_FILE)).unwrap()
    }

    #[test]
    fn no_file_means_system() {
        let th = dbee_testkit::temp_home(None, None);
        assert_eq!(load_theme(Some(&th.home)), ThemeChoice::System);
    }

    #[test]
    fn no_folder_keeps_nothing() {
        save_theme(None, ThemeChoice::Dark).unwrap();
        assert_eq!(load_theme(None), ThemeChoice::System);
    }

    #[test]
    fn the_choice_comes_back() {
        let th = dbee_testkit::temp_home(None, None);
        let dir = th.home.join("config");
        for t in [ThemeChoice::Dark, ThemeChoice::Light, ThemeChoice::System] {
            save_theme(Some(&dir), t).unwrap();
            assert_eq!(load_theme(Some(&dir)), t);
        }
    }

    #[test]
    fn a_broken_file_means_system() {
        let th = dbee_testkit::temp_home(None, None);
        let big = " ".repeat(65 * 1024);
        for text in ["garbage{", "[]", r#"{"theme": 3}"#, r#"{"theme": "sepia"}"#, big.as_str()] {
            write(&th.home, text);
            assert_eq!(load_theme(Some(&th.home)), ThemeChoice::System, "{text:.20}");
        }
        write(&th.home, &format!(r#"{{"theme": "dark"{}}}"#, " ".repeat(65 * 1024)));
        assert_eq!(load_theme(Some(&th.home)), ThemeChoice::System);
    }

    #[test]
    fn a_broken_file_is_replaced_by_a_save() {
        let th = dbee_testkit::temp_home(None, None);
        write(&th.home, "garbage{");
        save_theme(Some(&th.home), ThemeChoice::Light).unwrap();
        assert_eq!(load_theme(Some(&th.home)), ThemeChoice::Light);
        assert!(read(&th.home).contains(r#""version": 1"#));
    }

    #[test]
    fn other_fields_are_kept() {
        let th = dbee_testkit::temp_home(None, None);
        write(&th.home, r#"{"version":1,"later":{"a":1}}"#);
        save_theme(Some(&th.home), ThemeChoice::Dark).unwrap();
        let v: serde_json::Value = serde_json::from_str(&read(&th.home)).unwrap();
        assert_eq!(v["later"]["a"], 1);
        assert_eq!(v["theme"], "dark");
    }

    #[test]
    fn a_newer_file_is_read_and_not_written() {
        let th = dbee_testkit::temp_home(None, None);
        let text = r#"{"version":2,"theme":"dark"}"#;
        write(&th.home, text);
        assert_eq!(load_theme(Some(&th.home)), ThemeChoice::Dark);
        let err = save_theme(Some(&th.home), ThemeChoice::Light).unwrap_err();
        assert!(matches!(&err, SaveError::Newer(file) if *file == th.home.join(SETTINGS_FILE)), "{err:?}");
        assert_eq!(read(&th.home), text);
    }

    #[test]
    fn a_version_that_is_no_whole_number_counts_as_newer() {
        let th = dbee_testkit::temp_home(None, None);
        for text in [r#"{"version":"2","theme":"dark"}"#, r#"{"version":2.0}"#, r#"{"version":1.5}"#, r#"{"version":-1}"#, r#"{"version":null}"#] {
            write(&th.home, text);
            assert!(matches!(save_theme(Some(&th.home), ThemeChoice::Light), Err(SaveError::Newer(_))), "{text}");
            assert_eq!(read(&th.home), text, "left as it is");
        }
        write(&th.home, r#"{"version":1}"#);
        save_theme(Some(&th.home), ThemeChoice::Light).unwrap();
        assert_eq!(load_theme(Some(&th.home)), ThemeChoice::Light);
    }

    #[test]
    fn the_update_check_is_on_unless_the_file_says_off() {
        let th = dbee_testkit::temp_home(None, None);
        assert!(load_update_check(None));
        assert!(load_update_check(Some(&th.home)));
        for text in [
            "garbage{",
            "[]",
            r#"{"checkForUpdates": "no"}"#,
            r#"{"checkForUpdates": 0}"#,
            r#"{"checkForUpdates": null}"#,
            r#"{"checkForUpdates": true}"#,
        ] {
            write(&th.home, text);
            assert!(load_update_check(Some(&th.home)), "{text}");
        }
        write(&th.home, r#"{"version":1,"theme":"dark","checkForUpdates":false}"#);
        assert!(!load_update_check(Some(&th.home)));
        assert_eq!(load_theme(Some(&th.home)), ThemeChoice::Dark);
    }

    #[test]
    fn the_update_check_and_the_theme_keep_each_other() {
        let th = dbee_testkit::temp_home(None, None);
        save_update_check(None, false).unwrap();
        save_theme(Some(&th.home), ThemeChoice::Dark).unwrap();
        save_update_check(Some(&th.home), false).unwrap();
        assert!(!load_update_check(Some(&th.home)));
        assert_eq!(load_theme(Some(&th.home)), ThemeChoice::Dark);
        save_theme(Some(&th.home), ThemeChoice::Light).unwrap();
        assert!(!load_update_check(Some(&th.home)));
        save_update_check(Some(&th.home), true).unwrap();
        assert!(load_update_check(Some(&th.home)));
        assert_eq!(load_theme(Some(&th.home)), ThemeChoice::Light);
        write(&th.home, r#"{"version":2}"#);
        assert!(matches!(save_update_check(Some(&th.home), false), Err(SaveError::Newer(_))));
    }

    #[test]
    fn a_small_file_comes_back_and_a_larger_one_does_not() {
        let th = dbee_testkit::temp_home(None, None);
        let dir = th.home.join("config");
        assert_eq!(read_small_file(&dir, "note.json", 16), None);
        write_small_file(&dir, "note.json", b"0123456789abcdef").unwrap();
        assert_eq!(read_small_file(&dir, "note.json", 16).as_deref(), Some(&b"0123456789abcdef"[..]));
        assert_eq!(read_small_file(&dir, "note.json", 15), None);
        assert_eq!(read_small_file(&dir, "note.json", u64::MAX), None);
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let th = dbee_testkit::temp_home(None, None);
        save_theme(Some(&th.home), ThemeChoice::Dark).unwrap();
        let mode = std::fs::metadata(th.home.join(SETTINGS_FILE)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
