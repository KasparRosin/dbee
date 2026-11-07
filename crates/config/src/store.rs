//! The saved and recent connections file (`connections.json`).
//!
//! Several dbee processes can share it (two windows, say). So a change never writes a list from
//! memory: [`ConnectionStore::change`] reads the file again under a lock, applies the one change
//! to what it holds, and writes that, which keeps what another dbee saved meanwhile.

use crate::connection::{ConfigError, ConnectionConfig, Unknown, keep};
use serde::{Deserialize, Serialize};
use std::fs::{File, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// File name inside the config directory.
pub const FILE: &str = "connections.json";
/// The file's version this build writes. Adding a field keeps it; renaming one, or changing what
/// it means, moves it on. A newer file is read, and never written.
pub const VERSION: u32 = 1;
/// The lock file beside it, held while a change reads and writes the file.
pub const LOCK_FILE: &str = "connections.json.lock";
/// Most recent entries kept.
pub const MAX_RECENT: usize = 20;
/// How long a change waits for another dbee's change to the file.
const LOCK_WAIT: Duration = Duration::from_secs(5);
/// The warning for a file whose version is newer than [`VERSION`].
const NEWER: &str = "A newer dbee wrote the connections file, so changes here won't be saved.";

/// A connection used lately, with the Unix time of use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentEntry {
    pub config: ConnectionConfig,
    pub used_at: u64,
    /// Fields this build does not know, as [`ConnectionConfig::extra`].
    #[serde(flatten, skip_serializing)]
    pub extra: Unknown,
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
    /// The file's own fields this build does not know, written back as they were.
    extra: Unknown,
}

/// The file as entries, each decoded on its own.
#[derive(Serialize, Deserialize)]
#[serde(default)]
struct FileBody {
    version: u32,
    saved: Vec<serde_json::Value>,
    recent: Vec<serde_json::Value>,
    #[serde(flatten)]
    extra: Unknown,
}

/// `value`, an object, with the fields of `extra` it lacks.
fn with_unknown(mut value: serde_json::Value, extra: &Unknown) -> serde_json::Value {
    if let serde_json::Value::Object(map) = &mut value {
        keep(map, extra);
    }
    value
}

/// A saved connection as the file holds it: with the fields this build does not know.
fn config_value(c: &ConnectionConfig) -> Result<serde_json::Value, serde_json::Error> {
    let mut v = serde_json::to_value(c)?;
    if let (Some(ssh), Some(slot)) = (&c.ssh, v.get_mut("ssh")) {
        *slot = with_unknown(slot.take(), &ssh.extra);
    }
    Ok(with_unknown(v, &c.extra))
}

/// A recent entry as the file holds it, as [`config_value`].
fn recent_value(r: &RecentEntry) -> Result<serde_json::Value, serde_json::Error> {
    let mut v = serde_json::to_value(r)?;
    if let Some(slot) = v.get_mut("config") {
        *slot = config_value(&r.config)?;
    }
    Ok(with_unknown(v, &r.extra))
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
        FileBody { version: VERSION, saved: Vec::new(), recent: Vec::new(), extra: Unknown::new() }
    }
}

/// What reading the file gave.
enum Read {
    /// Its entries (none when there is no file yet), and a warning when some are kept unread.
    Entries(ConnectionStore, Option<String>),
    /// It was damaged and is moved aside, so there is no file now.
    MovedAside(String),
    /// A newer dbee wrote it: what this build could read of it, never to be written.
    Newer(ConnectionStore),
    /// It must not be written: it could not be read, or it is damaged and could not be moved aside.
    Unusable(String),
}

/// Reads `dir/connections.json`, moving a damaged one aside.
fn read(dir: &Path) -> Read {
    let mut store = ConnectionStore { dir: Some(dir.to_path_buf()), ..ConnectionStore::default() };
    let path = dir.join(FILE);
    let text = match std::fs::read(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Read::Entries(store, None),
        Err(e) => {
            return Read::Unusable(format!("could not read {}: {e}; the file was left untouched and changes will not be saved", path.display()));
        }
    };
    match serde_json::from_slice::<FileBody>(&text) {
        Ok(body) => {
            (store.saved, store.kept_saved) = decode_each(body.saved);
            (store.recent, store.kept_recent) = decode_each(body.recent);
            store.recent.truncate(MAX_RECENT);
            store.extra = body.extra;
            if body.version > VERSION {
                return Read::Newer(ConnectionStore { dir: None, ..store });
            }
            let kept = store.kept_saved.len() + store.kept_recent.len();
            let warning =
                (kept > 0).then(|| format!("{kept} saved or recent connection entries could not be read and are kept unchanged in the file"));
            Read::Entries(store, warning)
        }
        Err(e) => {
            let damaged = format!("{} is damaged ({e})", path.display());
            match free_bad_name(dir)
                .ok_or_else(|| "every .bad name is taken".to_string())
                .and_then(|bad| std::fs::rename(&path, &bad).map(|()| bad).map_err(|re| re.to_string()))
            {
                Ok(bad) => Read::MovedAside(format!("{damaged}; moved it to {}", bad.display())),
                Err(re) => {
                    Read::Unusable(format!("{damaged}; could not move it aside ({re}); the file was left untouched and changes will not be saved"))
                }
            }
        }
    }
}

/// The lock on `dir`'s file, once another dbee has let it go (within [`LOCK_WAIT`]); `None` where
/// the file system has no locks.
fn lock(dir: &Path) -> Result<Option<File>, ConfigError> {
    let io = |what: &str, e: std::io::Error| ConfigError::Io(format!("{what}: {e}"));
    std::fs::create_dir_all(dir).map_err(|e| io("create directory", e))?;
    let file = File::options().create(true).truncate(false).write(true).open(dir.join(LOCK_FILE)).map_err(|e| io("lock", e))?;
    let deadline = Instant::now().checked_add(LOCK_WAIT);
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(Some(file)),
            Err(TryLockError::WouldBlock) if deadline.is_some_and(|d| Instant::now() < d) => std::thread::sleep(Duration::from_millis(20)),
            Err(TryLockError::WouldBlock) => {
                return Err(ConfigError::Io(format!("another dbee kept it locked for {} s, so nothing was changed", LOCK_WAIT.as_secs())));
            }
            Err(TryLockError::Error(e)) if e.kind() == std::io::ErrorKind::Unsupported => return Ok(None),
            Err(TryLockError::Error(e)) => return Err(io("lock", e)),
        }
    }
}

impl ConnectionStore {
    /// Loads `dir/connections.json`. A missing file is an empty store. A file that cannot be
    /// parsed is renamed to `connections.json.bad` and a warning is returned with an empty store.
    /// A file that cannot be read gives a memory-only store, so no change overwrites it; so does a
    /// file a newer dbee wrote ([`VERSION`]), with what this build can read of it.
    pub fn load(dir: Option<&Path>) -> (ConnectionStore, Option<String>) {
        let Some(dir) = dir else { return (ConnectionStore::default(), None) };
        match read(dir) {
            Read::Entries(store, warning) => (store, warning),
            Read::MovedAside(warning) => (ConnectionStore { dir: Some(dir.to_path_buf()), ..ConnectionStore::default() }, Some(warning)),
            Read::Newer(store) => (store, Some(NEWER.into())),
            Read::Unusable(warning) => (ConnectionStore::default(), Some(warning)),
        }
    }

    /// Makes one change, `f`, to the file and to this copy, all or nothing. Under the lock beside
    /// the file it reads the file again, applies `f` to what the file holds, writes the result
    /// (temp file, then rename) and keeps it as this copy, so what another dbee changed meanwhile
    /// stays. The lock is held for that only, and one another dbee holds longer than 5 s is an
    /// error that changes nothing. A memory-only store applies `f` to itself. When the file can
    /// no longer be used, the store goes memory-only (unreadable; or newer, with what this build
    /// reads of it) or writes this copy in its place (damaged, and moved aside), with `f` applied,
    /// and the warning says why.
    pub fn change<R>(&mut self, f: impl FnOnce(&mut ConnectionStore) -> R) -> Result<(R, Option<String>), ConfigError> {
        let Some(dir) = self.dir.clone() else { return Ok((f(self), None)) };
        let _lock = lock(&dir)?;
        let (mut next, warning) = match read(&dir) {
            Read::Entries(fresh, warning) => (fresh, warning),
            Read::MovedAside(warning) => (self.clone(), Some(warning)),
            Read::Newer(fresh) => {
                *self = fresh;
                return Ok((f(self), Some(NEWER.into())));
            }
            Read::Unusable(warning) => {
                self.dir = None;
                return Ok((f(self), Some(warning)));
            }
        };
        let r = f(&mut next);
        next.write(&dir)?;
        *self = next;
        Ok((r, warning))
    }

    /// Reads the file again into this copy, so it shows what another dbee changed meanwhile. A
    /// damaged file is moved aside and this copy kept (the next change writes it); an unreadable
    /// or newer one makes the store memory-only. The warning says why.
    pub fn reload(&mut self) -> Option<String> {
        let dir = self.dir.clone()?;
        match read(&dir) {
            Read::Entries(fresh, warning) => {
                *self = fresh;
                warning
            }
            Read::MovedAside(warning) => Some(warning),
            Read::Newer(fresh) => {
                *self = fresh;
                Some(NEWER.into())
            }
            Read::Unusable(warning) => {
                self.dir = None;
                Some(warning)
            }
        }
    }

    /// Writes this store to `dir/connections.json` atomically (temp file, then rename).
    fn write(&self, dir: &Path) -> Result<(), ConfigError> {
        let io = |what: &str, e: std::io::Error| ConfigError::Io(format!("{what}: {e}"));
        let to_values = |items: Vec<Result<serde_json::Value, serde_json::Error>>, kept: &[serde_json::Value]| {
            items.into_iter().collect::<Result<Vec<_>, _>>().map(|mut v| {
                v.extend(kept.iter().cloned());
                v
            })
        };
        let saved = to_values(self.saved.iter().map(config_value).collect(), &self.kept_saved);
        let recent = to_values(self.recent.iter().map(recent_value).collect(), &self.kept_recent);
        let body = FileBody {
            version: VERSION,
            saved: saved.map_err(|e| ConfigError::Io(e.to_string()))?,
            recent: recent.map_err(|e| ConfigError::Io(e.to_string()))?,
            extra: self.extra.clone(),
        };
        let json = serde_json::to_vec_pretty(&body).map_err(|e| ConfigError::Io(e.to_string()))?;
        std::fs::create_dir_all(dir).map_err(|e| io("create directory", e))?;
        let tmp = dir.join(format!("{FILE}.{}.tmp", crate::connection::new_id()));
        let written = (|| {
            let mut file = File::create(&tmp).map_err(|e| io("write", e))?;
            file.write_all(&json).map_err(|e| io("write", e))?;
            file.sync_all().map_err(|e| io("sync", e))
        })();
        let result = written.and_then(|()| std::fs::rename(&tmp, dir.join(FILE)).map_err(|e| io("rename", e)));
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }

    /// Replaces the saved connection with the same id, keeping the fields this build does not
    /// know of the one it replaces, or appends.
    pub fn upsert(&mut self, mut c: ConnectionConfig) {
        match self.saved.iter_mut().find(|s| s.id == c.id) {
            Some(slot) => {
                c.keep_unknown(slot);
                *slot = c;
            }
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

    /// Records a use: dedups by id (keeping the fields this build does not know of the entry it
    /// replaces), newest first, capped at [`MAX_RECENT`].
    pub fn touch_recent(&mut self, c: &ConnectionConfig, now: u64) {
        let mut entry = RecentEntry { config: c.clone(), used_at: now, extra: Unknown::new() };
        if let Some(old) = self.recent.iter().find(|r| r.config.id == c.id) {
            entry.config.keep_unknown(&old.config);
            entry.extra = old.extra.clone();
        }
        self.recent.retain(|r| r.config.id != c.id);
        self.recent.insert(0, entry);
        self.recent.truncate(MAX_RECENT);
    }
}
