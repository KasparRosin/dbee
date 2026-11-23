//! The control channel's guards (`docs/control-protocol.md`) from a client's side, with the
//! driver the day tests share: the first test needs no window, and the second drives the real app
//! on a throwaway home, by hand or in the CI workflow, as the days do.
//!
//! Safety: the app gets an isolated run's environment (`common`), and nothing here connects to
//! anything but a stand-in on `127.0.0.1` and that app.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use common::App;
use dbee_testkit::temp_home;
use serde_json::{Value as Json, json};

/// Where the real app's screenshots go, from the repository root.
const SHOTS: &str = "target/control-access";

#[test]
fn a_client_stops_at_a_stand_in_that_holds_the_port_and_cannot_give_the_proof() {
    // A run was killed and left its file; whatever holds the port now never read that file.
    let home = temp_home(None, None);
    let stale = home.home.join("control-token");
    std::fs::write(&stale, "old-token\nold-proof\n").unwrap();
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let stand_in = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        let mut writer = stream.try_clone().unwrap();
        let mut reader = BufReader::new(stream);
        let mut heard = Vec::new();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        heard.push(line.trim_end().to_string());
        // It says what a real app says, with the token it heard as its best guess.
        writeln!(writer, "{}", json!({ "id": null, "ok": true, "result": { "proof": heard[0] } })).unwrap();
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(n) if n > 0 => heard.push(line.trim_end().to_string()),
                _ => return heard,
            }
        }
    });
    let e = common::connect(port, &stale).unwrap_err();
    assert!(e.contains("does not carry the file's proof"), "{e}");
    assert_eq!(stand_in.join().unwrap(), vec!["old-token".to_string()], "it heard a dead token, and nothing else");
}

/// One request on a connection that never sent the token: the replies, up to the close.
fn without_the_token(port: u16, first: &str) -> Vec<Json> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    stream.write_all(format!("{first}\n").as_bytes()).unwrap();
    BufReader::new(stream).lines().map_while(Result::ok).map(|l| serde_json::from_str(&l).unwrap()).collect()
}

#[test]
#[ignore = "opens a window on the desktop; run by hand: cargo test -p dbee --test control_access -- --ignored --nocapture"]
fn the_real_app_guards_its_channel_its_token_file_and_its_screenshots() {
    let db = dbee_testkit::require_testdb!();
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        dbee_testkit::skipped("control_access: no display (DISPLAY or WAYLAND_DISPLAY) to open the window on");
        return;
    }
    let root = dbee_testkit::repo_root();
    let shots = root.join(SHOTS);
    let _ = std::fs::remove_dir_all(&shots);
    let home = temp_home(None, None);
    let mut app = App::start(&root, &home.home, db, SHOTS);
    let (port, token_file) = (app.port, app.token_file.clone());

    // No token: refused, and the app goes on.
    for first in [
        r#"{"id":1,"method":"app.quit"}"#.to_string(),
        "not the token".to_string(),
        std::fs::read_to_string(&token_file).unwrap().lines().nth(1).unwrap().to_string(),
    ] {
        let replies = without_the_token(port, &first);
        assert_eq!(replies.len(), 1, "{replies:?}");
        assert_eq!((replies[0]["ok"].clone(), replies[0]["code"].clone()), (json!(false), json!("unauthorized")), "{replies:?}");
    }
    assert!(app.inspect()["frame"].as_u64().is_some());

    // The file is this user's alone.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&token_file), 0o600);
        assert_eq!(mode(token_file.parent().unwrap()), 0o700);
    }

    // A second start on the port stops, and leaves the running app's file alone.
    let before = std::fs::read_to_string(&token_file).unwrap();
    let second = common::command(&root, &home.home, db, SHOTS, port).output().unwrap();
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains(&format!("cannot start on 127.0.0.1:{port}")), "{second:?}");
    assert_eq!(std::fs::read_to_string(&token_file).unwrap(), before);

    // Screenshots: only a file name, only in the folder.
    let outside = home.home.join("outside.png");
    let above = root.join("target").join("above-control-access.png");
    let cases = [
        (outside.display().to_string(), "badParams"),
        ("../above-control-access.png".to_string(), "badParams"),
        ("sub/shot.png".to_string(), "badParams"),
    ];
    for (path, code) in cases {
        let r = app.call("ui.screenshot", json!({ "path": path }));
        assert_eq!((r["ok"].clone(), r["code"].clone()), (json!(false), json!(code)), "{path}: {r}");
    }
    assert!(!outside.exists() && !above.exists());
    let shot = app.shot(&root, "plain");
    assert!(std::fs::read(&shot).unwrap().starts_with(b"\x89PNG"));
    assert_eq!(app.shot(&root, "plain"), shot, "the same name again replaces it");
    #[cfg(unix)]
    {
        let victim = home.home.join("victim");
        std::fs::write(&victim, "keep me").unwrap();
        std::os::unix::fs::symlink(&victim, shots.join("link.png")).unwrap();
        let r = app.call("ui.screenshot", json!({ "path": "link.png" }));
        assert_eq!((r["ok"].clone(), r["code"].clone()), (json!(false), json!("screenshot")), "{r}");
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep me");
    }
    // Without a path the picture comes back.
    assert!(app.ok("ui.screenshot", json!({}))["base64"].is_string());

    // The file goes with the app.
    app.quit();
    assert!(!token_file.exists(), "the token file goes when the app ends");
    let _ = std::fs::remove_dir_all(&shots);
}
