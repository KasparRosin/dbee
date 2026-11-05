//! The driver the day tests share (`mod common;`): the real app, `dbee --control <port>`, on a
//! throwaway home with the environment of `cargo xtask run-isolated`, driven only through the
//! control channel (`docs/control-protocol.md`), with screenshots saved under the repository's
//! `target/` to look at.
//!
//! Safety: the app gets an isolated run's environment: the given home (with the test key and the
//! sshd's host key, from `dbee_testkit::temp_home`), in-memory secrets and clipboard (no copy
//! reaches the system clipboard), no SSH agent, the test servers as its only targets, and the
//! home and `target/` as the only folders a database file may lie in.
#![allow(dead_code, reason = "each test binary uses some of these helpers")]

use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use dbee_testkit::TestDb;
use serde_json::{Value as Json, json};

/// The longest the test waits for any one thing.
pub const WAIT: Duration = Duration::from_secs(60);

/// The app's process, killed and waited for when dropped: also when [`App::start`] fails
/// before there is an [`App`] to end it.
struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The real app with its control channel, ended (`app.quit`, then killed) when dropped.
pub struct App {
    child: Running,
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    next: u64,
    /// Where [`App::shot`] saves, from the repository root (the app's working directory).
    shots: &'static str,
}

/// The targets the app may connect to, as `run-isolated` allows them: each server's port and
/// the sshd's on `127.0.0.1` and `localhost`, and `127.0.0.1:1`.
fn allowed(db: &TestDb) -> String {
    let ports = db.servers.iter().map(|s| s.port).chain([db.sshd.port]);
    let mut list: Vec<String> = ports.flat_map(|p| [format!("127.0.0.1:{p}"), format!("localhost:{p}")]).collect();
    list.push("127.0.0.1:1".into());
    list.join(",")
}

/// A port nothing listens on right now.
fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port()
}

impl App {
    /// Starts the app from `root` on `home` with the isolated environment (`isolated::command` in
    /// `xtask/src/isolated.rs`), saving screenshots in `shots`, and waits for its first frame.
    pub fn start(root: &Path, home: &Path, db: &TestDb, shots: &'static str) -> App {
        App::start_with(root, home, db, shots, &[])
    }

    /// [`App::start`] with `args` after `--control <port>` on the command line (`dbee <file>`).
    pub fn start_with(root: &Path, home: &Path, db: &TestDb, shots: &'static str, args: &[&OsStr]) -> App {
        let port = free_port();
        let child = Command::new(env!("CARGO_BIN_EXE_dbee"))
            .current_dir(root)
            .arg("--control")
            .arg(port.to_string())
            .args(args)
            .env("HOME", home)
            .env("USERPROFILE", home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_DATA_HOME", home.join(".local").join("share"))
            .env("XDG_STATE_HOME", home.join(".local").join("state"))
            .env("XDG_CACHE_HOME", home.join(".cache"))
            .env("APPDATA", home.join("AppData").join("Roaming"))
            .env("LOCALAPPDATA", home.join("AppData").join("Local"))
            .env("DBEE_SECRET_STORE", "memory")
            .env("DBEE_CLIPBOARD", "memory")
            .env("DBEE_ALLOW_TARGETS", allowed(db))
            .env("DBEE_ALLOW_DIRS", std::env::join_paths([home.to_path_buf(), root.join("target")]).unwrap())
            .env_remove("SSH_AUTH_SOCK")
            .env_remove("DBEE_CONTROL_PORT")
            .spawn()
            .unwrap();
        let child = Running(child);
        // The channel listens before the window opens.
        let start = Instant::now();
        let stream = loop {
            if let Ok(s) = TcpStream::connect(("127.0.0.1", port)) {
                break s;
            }
            assert!(start.elapsed() < WAIT, "the control channel did not open on port {port}");
            std::thread::sleep(Duration::from_millis(50));
        };
        stream.set_read_timeout(Some(Duration::from_secs(120))).unwrap();
        let writer = stream.try_clone().unwrap();
        let mut app = App { child, reader: BufReader::new(stream), writer, next: 0, shots };
        app.until("the first frame", |app| app.inspect()["frame"].as_u64().is_some_and(|f| f >= 1));
        app
    }

    /// One request and its reply.
    pub fn call(&mut self, method: &str, params: Json) -> Json {
        self.next += 1;
        let line = json!({ "id": self.next, "method": method, "params": params });
        writeln!(self.writer, "{line}").unwrap();
        let mut reply = String::new();
        self.reader.read_line(&mut reply).unwrap();
        let reply: Json = serde_json::from_str(&reply).unwrap_or_else(|e| panic!("{method}: not JSON ({e}): {reply}"));
        assert_eq!(reply["id"], self.next, "{reply}");
        reply
    }

    pub fn ok(&mut self, method: &str, params: Json) -> Json {
        let r = self.call(method, params);
        assert_eq!(r["ok"], true, "{method}: {r}");
        r["result"].clone()
    }

    pub fn execute(&mut self, command: &str, params: Json) -> Json {
        self.ok("engine.execute", json!({ "command": command, "params": params }))
    }

    pub fn inspect(&mut self) -> Json {
        self.ok("ui.inspect", json!({}))
    }

    pub fn document(&mut self) -> Json {
        self.ok("document.inspect", json!({}))
    }

    /// The `table` part of `document.inspect`.
    pub fn table(&mut self) -> Json {
        self.document()["table"].clone()
    }

    pub fn tag(&mut self, id: &str) -> Option<Json> {
        let i = self.inspect();
        i["automation"].as_array().unwrap().iter().find(|a| a["id"] == id).cloned()
    }

    pub fn value(&mut self, id: &str) -> Json {
        self.tag(id).unwrap_or_else(|| panic!("no widget `{id}` was drawn"))["value"].clone()
    }

    /// `[x, y, width, height]` of widget `id`, in window points.
    pub fn rect(&mut self, id: &str) -> [f64; 4] {
        let tag = self.tag(id).unwrap_or_else(|| panic!("no widget `{id}` was drawn"));
        let r: Vec<f64> = tag["rect"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        [r[0], r[1], r[2], r[3]]
    }

    pub fn click(&mut self, id: &str) {
        self.ok("ui.click", json!({ "id": id }));
    }

    pub fn double_click(&mut self, id: &str) {
        self.ok("ui.click", json!({ "id": id, "count": 2 }));
    }

    pub fn right_click(&mut self, id: &str) {
        self.ok("ui.click", json!({ "id": id, "button": "right" }));
    }

    /// A click at window point `(x, y)`, with Shift held when `shift`.
    pub fn click_at(&mut self, x: f64, y: f64, shift: bool) {
        self.ok("ui.click", json!({ "x": x, "y": y, "modifiers": { "shift": shift } }));
    }

    pub fn key(&mut self, key: &str) {
        self.ok("ui.key", json!({ "key": key }));
    }

    /// `key` with Ctrl (Cmd on macOS).
    pub fn ctrl(&mut self, key: &str) {
        self.ok("ui.key", json!({ "key": key, "cmd": true }));
    }

    pub fn ctrl_shift(&mut self, key: &str) {
        self.ok("ui.key", json!({ "key": key, "cmd": true, "shift": true }));
    }

    pub fn text(&mut self, text: &str) {
        self.ok("ui.text", json!({ "text": text }));
    }

    /// Clicks the text field `id`, selects all of it and types `text` over it.
    pub fn replace(&mut self, id: &str, text: &str) {
        self.click(id);
        self.ctrl("A");
        self.text(text);
    }

    /// The automation entries whose id starts with `prefix`, in drawing order.
    pub fn tags_with(&mut self, prefix: &str) -> Vec<Json> {
        let i = self.inspect();
        let all = i["automation"].as_array().unwrap();
        all.iter().filter(|a| a["id"].as_str().is_some_and(|id| id.starts_with(prefix))).cloned().collect()
    }

    /// Asks until `done` holds (each request runs a frame).
    pub fn until(&mut self, what: &str, mut done: impl FnMut(&mut App) -> bool) {
        let start = Instant::now();
        while !done(self) {
            assert!(start.elapsed() < WAIT, "gave up waiting for {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Waits for the active table tab's page and Apply to end.
    pub fn settled(&mut self, what: &str) -> Json {
        self.until(what, |app| {
            let t = app.table();
            t.is_object() && t["loading"].is_null() && t["applying"].is_null()
        });
        self.table()
    }

    /// Saves a screenshot as `<shots>/<name>.png` and checks the file is there.
    pub fn shot(&mut self, root: &Path, name: &str) -> PathBuf {
        let path = format!("{}/{name}.png", self.shots);
        let r = self.ok("ui.screenshot", json!({ "path": path }));
        let file = root.join(&path);
        assert!(file.is_file() && r["width"].as_u64().is_some_and(|w| w > 0), "{r}");
        println!("step screenshot: {}", file.display());
        file
    }

    /// The loaded grid's cells by row: `cells[row][column]` is the cell's value
    /// (`{text, null, state?}`), rows out of view included.
    pub fn grid(&mut self) -> Vec<Vec<Json>> {
        let i = self.inspect();
        let mut cells: Vec<Vec<Json>> = Vec::new();
        for a in i["automation"].as_array().unwrap() {
            let Some(rest) = a["id"].as_str().and_then(|id| id.strip_prefix("grid-cell-")) else { continue };
            let (r, c) = rest.split_once('-').unwrap();
            let (r, c): (usize, usize) = (r.parse().unwrap(), c.parse().unwrap());
            if cells.len() <= r {
                cells.resize(r + 1, Vec::new());
            }
            if cells[r].len() <= c {
                cells[r].resize(c + 1, Json::Null);
            }
            cells[r][c] = a["value"].clone();
        }
        cells
    }

    /// The text of cell `(row, column)` as the grid shows it.
    pub fn cell(&mut self, row: usize, column: usize) -> Json {
        self.value(&format!("grid-cell-{row}-{column}"))
    }
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = writeln!(self.writer, "{}", json!({ "id": 0, "method": "app.quit" }));
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.child.0.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        // `Running` kills and waits for it if it has not ended.
    }
}
