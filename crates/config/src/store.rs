//! The saved and recent connections file (`connections.json`).

use crate::connection::{ConfigError, ConnectionConfig};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

/// File name inside the config directory.
pub const FILE: &str = "connections.json";
/// Most recent entries kept.
pub const MAX_RECENT: usize = 20;

/// A connection used lately, with the Unix time of use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentEntry {
    pub config: ConnectionConfig,
    pub used_at: u64,
}

/// Saved and recent connections. `dir: None` keeps everything in memory.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConnectionStore {
    pub saved: Vec<ConnectionConfig>,
    pub recent: Vec<RecentEntry>,
    pub dir: Option<PathBuf>,
    /// Entries this dbee could not decode (say, a type from a newer dbee), written back as they were.
    kept_saved: Vec<serde_json::Value>,
    kept_recent: Vec<serde_json::Value>,
}

/// The file as entries, each decoded on its own.
#[derive(Serialize, Deserialize)]
#[serde(default)]
struct FileBody {
    version: u32,
    saved: Vec<serde_json::Value>,
    recent: Vec<serde_json::Value>,
}

/// Decoded entries, and the ones that would not decode.
fn decode_each<T: serde::de::DeserializeOwned>(values: Vec<serde_json::Value>) -> (Vec<T>, Vec<serde_json::Value>) {
    let (mut ok, mut kept) = (Vec::new(), Vec::new());
    for v in values {
        match serde_json::from_value::<T>(v.clone()) {
            Ok(t) => ok.push(t),
            Err(_) => kept.push(v),
        }
    }
    (ok, kept)
}

/// `connections.json.bad`, or the first of `.bad.1`, `.bad.2`… not taken yet.
fn free_bad_name(dir: &Path) -> Option<PathBuf> {
    let taken = |p: &Path| std::fs::symlink_metadata(p).is_ok();
    let first = dir.join(format!("{FILE}.bad"));
    if !taken(&first) {
        return Some(first);
    }
    (1..100).map(|n| dir.join(format!("{FILE}.bad.{n}"))).find(|p| !taken(p))
}

impl Default for FileBody {
    fn default() -> Self {
        FileBody { version: 1, saved: Vec::new(), recent: Vec::new() }
    }
}

impl ConnectionStore {
    /// Loads `dir/connections.json`. A missing file is an empty store. A file that cannot be
    /// parsed is renamed to `connections.json.bad` and a warning is returned with an empty store.
    pub fn load(dir: Option<&Path>) -> (ConnectionStore, Option<String>) {
        let mut store = ConnectionStore { dir: dir.map(Path::to_path_buf), ..ConnectionStore::default() };
        let Some(dir) = dir else { return (store, None) };
        let path = dir.join(FILE);
        let text = match std::fs::read(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (store, None),
            Err(e) => {
                // Memory only from here on, so a later save cannot overwrite the unreadable file.
                store.dir = None;
                return (store, Some(format!("could not read {}: {e}; the file was left untouched and changes will not be saved", path.display())));
            }
        };
        match serde_json::from_slice::<FileBody>(&text) {
            Ok(body) => {
                (store.saved, store.kept_saved) = decode_each(body.saved);
                (store.recent, store.kept_recent) = decode_each(body.recent);
                store.recent.truncate(MAX_RECENT);
                let kept = store.kept_saved.len() + store.kept_recent.len();
                let warning =
                    (kept > 0).then(|| format!("{kept} saved or recent connection entries could not be read and are kept unchanged in the file"));
                (store, warning)
            }
            Err(e) => {
                let moved = match free_bad_name(dir)
                    .ok_or_else(|| "every .bad name is taken".to_string())
                    .and_then(|bad| std::fs::rename(&path, &bad).map(|()| bad).map_err(|re| re.to_string()))
                {
                    Ok(bad) => format!("moved it to {}", bad.display()),
                    Err(re) => {
                        store.dir = None;
                        format!("could not move it aside ({re}); the file was left untouched and changes will not be saved")
                    }
                };
                (store, Some(format!("{} is damaged ({e}); {moved}", path.display())))
            }
        }
    }

    /// Writes the file atomically (temp file, then rename). Memory-only stores do nothing.
    pub fn save(&self) -> Result<(), ConfigError> {
        let Some(dir) = &self.dir else { return Ok(()) };
        let io = |what: &str, e: std::io::Error| ConfigError::Io(format!("{what}: {e}"));
        let to_values = |items: Vec<Result<serde_json::Value, serde_json::Error>>, kept: &[serde_json::Value]| {
            items.into_iter().collect::<Result<Vec<_>, _>>().map(|mut v| {
                v.extend(kept.iter().cloned());
                v
            })
        };
        let saved = to_values(self.saved.iter().map(serde_json::to_value).collect(), &self.kept_saved);
        let recent = to_values(self.recent.iter().map(serde_json::to_value).collect(), &self.kept_recent);
        let body = FileBody {
            version: 1,
            saved: saved.map_err(|e| ConfigError::Io(e.to_string()))?,
            recent: recent.map_err(|e| ConfigError::Io(e.to_string()))?,
        };
        let json = serde_json::to_vec_pretty(&body).map_err(|e| ConfigError::Io(e.to_string()))?;
        std::fs::create_dir_all(dir).map_err(|e| io("create directory", e))?;
        let tmp = dir.join(format!("{FILE}.{}.tmp", crate::connection::new_id()));
        let written = (|| {
            let mut file = std::fs::File::create(&tmp).map_err(|e| io("write", e))?;
            file.write_all(&json).map_err(|e| io("write", e))?;
            file.sync_all().map_err(|e| io("sync", e))
        })();
        let result = written.and_then(|()| std::fs::rename(&tmp, dir.join(FILE)).map_err(|e| io("rename", e)));
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }

    /// Replaces the saved connection with the same id, or appends.
    pub fn upsert(&mut self, c: ConnectionConfig) {
        match self.saved.iter_mut().find(|s| s.id == c.id) {
            Some(slot) => *slot = c,
            None => self.saved.push(c),
        }
    }

    /// Removes a saved connection; `true` if there was one.
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.saved.len();
        self.saved.retain(|c| c.id != id);
        self.saved.len() != before
    }

    /// The saved list holds the connection with this id.
    pub fn is_saved(&self, id: &str) -> bool {
        self.saved.iter().any(|c| c.id == id)
    }

    /// Saved first, then recent.
    pub fn get(&self, id: &str) -> Option<&ConnectionConfig> {
        self.saved.iter().find(|c| c.id == id).or_else(|| self.recent.iter().map(|r| &r.config).find(|c| c.id == id))
    }

    /// Records a use: dedups by id, newest first, capped at [`MAX_RECENT`].
    pub fn touch_recent(&mut self, c: &ConnectionConfig, now: u64) {
        self.recent.retain(|r| r.config.id != c.id);
        self.recent.insert(0, RecentEntry { config: c.clone(), used_at: now });
        self.recent.truncate(MAX_RECENT);
    }
}
