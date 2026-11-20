//! The driver the day tests share (`mod common;`): the real app, `dbee --control <port>`, on a
//! throwaway home with the environment of `cargo xtask run-isolated`, driven only through the
//! control channel (`docs/control-protocol.md`), with screenshots saved under the repository's
//! `target/` to look at, the run's one screenshot folder. The client sends the token from the
//! run's token file, in the home, and checks the app's proof first ([`connect`]).
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
    /// The run's screenshot folder, from the repository root, where [`App::shot`] saves.
    shots: &'static str,
    pub port: u16,
    pub token_file: PathBuf,
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

/// The file the run on `home` keeps its token in.
fn token_file(home: &Path) -> PathBuf {
    home.join("control-token")
}

/// The app as an isolated run starts it, on `port`, with a screenshot folder and its own token file.
pub fn command(root: &Path, home: &Path, db: &TestDb, shots: &str, port: u16) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_dbee"));
    command
        .current_dir(root)
        .arg("--control")
        .arg(port.to_string())
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
        .env("DBEE_SCREENSHOT_DIR", root.join(shots))
        .env("DBEE_CONTROL_TOKEN_FILE", token_file(home))
        .env_remove("SSH_AUTH_SOCK")
        .env_remove("DBEE_CONTROL_PORT");
    command
}

/// A connection to the app on `port` that has sent the token from `token_file` and checked the
/// proof in the reply; `Err` says why not, and then nothing more was sent. What a client must do
/// (`docs/control-protocol.md`).
pub fn connect(port: u16, token_file: &Path) -> Result<(BufReader<TcpStream>, TcpStream), String> {
    let text = std::fs::read_to_string(token_file).map_err(|e| format!("{}: {e}", token_file.display()))?;
    let mut lines = text.lines();
    let (Some(token), Some(proof)) = (lines.next(), lines.next()) else { return Err(format!("{} is not whole", token_file.display())) };
    let stream = TcpStream::connect(("127.0.0.1", port)).map_err(|e| format!("port {port}: {e}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(120))).map_err(|e| e.to_string())?;
    let mut writer = stream.try_clone().map_err(|e| e.to_string())?;
    writeln!(writer, "{token}").map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream);
    let mut reply = String::new();
    reader.read_line(&mut reply).map_err(|e| e.to_string())?;
    let reply: Json = serde_json::from_str(&reply).map_err(|e| format!("not JSON ({e}): {reply}"))?;
    if reply["result"]["proof"] != proof {
        return Err(format!("the reply does not carry the file's proof: {reply}"));
    }
    Ok((reader, writer))
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
        let child = command(root, home, db, shots, port).args(args).spawn().unwrap();
        let child = Running(child);
        // The channel listens before the window opens, and the file comes once it does.
        let token_file = token_file(home);
        let start = Instant::now();
        let (reader, writer) = loop {
            match connect(port, &token_file) {
                Ok(connected) => break connected,
                Err(why) => assert!(start.elapsed() < WAIT, "the control channel did not open on port {port}: {why}"),
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let mut app = App { child, reader, writer, next: 0, shots, port, token_file };
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

    /// Widget `inner` lies inside widget `outer` (to half a point).
    pub fn inside(&mut self, inner: &str, outer: &str) -> bool {
        let ([x, y, w, h], [ox, oy, ow, oh]) = (self.rect(inner), self.rect(outer));
        x >= ox - 0.5 && y >= oy - 0.5 && x + w <= ox + ow + 0.5 && y + h <= oy + oh + 0.5
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

    /// The widgets of `ids` that were drawn lie inside the window, and no two overlap (by more
    /// than half a point).
    pub fn assert_apart(&mut self, ids: &[&str], at: &str) {
        let i = self.inspect();
        let window: Vec<f64> = i["window"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        let drawn: Vec<(&str, [f64; 4])> = ids
            .iter()
            .filter_map(|id| {
                let tag = i["automation"].as_array().unwrap().iter().find(|a| a["id"] == *id && a["visible"] != false)?;
                let r: Vec<f64> = tag["rect"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
                Some((*id, [r[0], r[1], r[2], r[3]]))
            })
            .collect();
        for (id, [x, y, w, h]) in &drawn {
            assert!(
                *x >= -0.5 && *y >= -0.5 && x + w <= window[0] + 0.5 && y + h <= window[1] + 0.5,
                "{at}: `{id}` {:?} inside the window {window:?}",
                [x, y, w, h]
            );
        }
        for (n, (a, [ax, ay, aw, ah])) in drawn.iter().enumerate() {
            for (b, [bx, by, bw, bh]) in &drawn[n + 1..] {
                let (w, h) = ((ax + aw).min(bx + bw) - ax.max(*bx), (ay + ah).min(by + bh) - ay.max(*by));
                assert!(w <= 0.0 || h <= 0.0 || w * h <= 0.5, "{at}: `{a}` over `{b}`");
            }
        }
    }

    pub fn double_click(&mut self, id: &str) {
        self.ok("ui.click", json!({ "id": id, "count": 2 }));
    }

    pub fn right_click(&mut self, id: &str) {
        self.ok("ui.click", json!({ "id": id, "button": "right" }));
    }

    /// Selects the whole of a line of the value panel's text the way a user does: a click at its
    /// first row's start, then a shift-click at the right end of each row below until the
    /// selection ends where the line does (a line wider than the panel wraps onto more rows).
    /// `row` is the line's first row, `start` its byte offset and `len` its length in bytes.
    pub fn select_value_line(&mut self, row: usize, start: usize, len: usize, pad: f64, line_h: f64) {
        let [x, y, w, _] = self.rect("value-text");
        let at = |r: usize| y + line_h * (r as f64 + 0.5);
        self.click_at(x + 1.0, at(row), false);
        let mut seen = Vec::new();
        for r in row..row + 6 {
            self.click_at(x + w - pad, at(r), true);
            let selection = self.value("value-text")["selection"].clone();
            if selection == json!([start, start + len]) {
                return;
            }
            seen.push(selection);
        }
        panic!("no row's right end selected the line at {start}..{}: {seen:?}", start + len);
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
        let name = format!("{name}.png");
        let r = self.ok("ui.screenshot", json!({ "path": name }));
        let file = root.join(self.shots).join(&name);
        assert_eq!(r["path"], file.display().to_string(), "{r}");
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

impl App {
    /// Asks the app to quit and waits a little for it to end.
    pub fn quit(&mut self) {
        let _ = writeln!(self.writer, "{}", json!({ "id": 0, "method": "app.quit" }));
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.child.0.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.quit();
        // `Running` kills and waits for it if it has not ended.
    }
}
