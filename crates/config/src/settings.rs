//! The settings file beside the connections file: `{"version": 1, "theme": "dark"}`.

use std::io::Read;
use std::path::Path;

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

/// Saves it, keeping the file's other fields. `None` writes nothing. A newer dbee's file is left alone, and is an error.
pub fn save_theme(dir: Option<&Path>, theme: ThemeChoice) -> std::io::Result<()> {
    let Some(dir) = dir else { return Ok(()) };
    let mut map = read_object(dir).unwrap_or_default();
    if map.get("version").and_then(|v| v.as_u64()).is_some_and(|v| v > u64::from(SETTINGS_VERSION)) {
        return Err(std::io::Error::other("A newer dbee wrote the settings file, so this choice won't be saved."));
    }
    map.insert("version".into(), SETTINGS_VERSION.into());
    map.insert("theme".into(), theme.as_str().into());
    let mut bytes = serde_json::to_vec_pretty(&map).map_err(std::io::Error::other)?;
    bytes.push(b'\n');
    private_dir(dir)?;
    replace_private_file(&dir.join(SETTINGS_FILE), &bytes)
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
        assert!(err.to_string().contains("A newer dbee wrote the settings file"));
        assert_eq!(read(&th.home), text);
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
