//! The update check end to end in the real app, against a stand-in for the releases on
//! `127.0.0.1` (`DBEE_UPDATE_MANIFEST_URL`, `docs/testing.md`), with the driver the day tests
//! share: by hand or in the CI workflow, on a display.
//!
//! Safety: the app runs on a throwaway home with in-memory secrets and clipboard (so the page a
//! click would open is only named, never opened), and its manifest comes from this test's own
//! listener or from nowhere: a development build, which a test binary's `dbee` is, checks nothing
//! unless that listener is named. It never reaches GitHub.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::App;
use dbee_testkit::temp_home;
use serde_json::json;

/// Where the real app's screenshots go, from the repository root.
const SHOTS: &str = "target/update-check";

const HASH: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";

/// A stand-in for the releases: answers every request with the manifest of release `version`,
/// and keeps each request's lines. Returns its port.
fn releases(version: &str) -> (u16, Arc<Mutex<Vec<Vec<String>>>>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let heard = Arc::new(Mutex::new(Vec::new()));
    let body = format!("{HASH}  dbee-{version}-linux-x86_64.deb\n{HASH}  dbee-{version}-linux-x86_64.tar.gz\n");
    let kept = Arc::clone(&heard);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(20)));
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut lines = Vec::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim_end().is_empty() {
                    break;
                }
                lines.push(line.trim_end().to_string());
            }
            kept.lock().unwrap().push(lines);
            let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        }
    });
    (port, heard)
}

fn no_display() -> bool {
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        dbee_testkit::skipped("update_check: no display (DISPLAY or WAYLAND_DISPLAY) to open the window on");
        return true;
    }
    false
}

#[test]
#[ignore = "opens a window on the desktop; run by hand: cargo test -p dbee --test update_check -- --ignored --nocapture"]
fn the_real_app_finds_a_newer_release_and_offers_its_download() {
    if no_display() {
        return;
    }
    let root = dbee_testkit::repo_root();
    let _ = std::fs::remove_dir_all(root.join(SHOTS));
    let home = temp_home(None, None);
    let (port, heard) = releases("0.3.1");
    let url = format!("http://127.0.0.1:{port}/latest");
    let mut app = App::start_with_env(&root, &home.home, SHOTS, &[("DBEE_UPDATE_MANIFEST_URL", &url), ("DBEE_UPDATE_PRETEND_VERSION", "0.3.0")]);

    // The check runs by itself a few seconds after the start; until then nothing is shown.
    let update = app.document()["update"].clone();
    assert_eq!(update["checks"], "automatic", "{update}");
    app.until("the update button", |app| app.tag("update-button").is_some());
    assert_eq!(app.value("update-button"), json!({ "state": "download", "label": "Download 0.3.1" }));
    let page = "https://github.com/KasparRosin/dbee/releases/tag/v0.3.1";
    let update = app.document()["update"].clone();
    assert_eq!(
        (update["state"].clone(), update["from"].clone(), update["to"].clone()),
        (json!("available"), json!("0.3.0"), json!("0.3.1")),
        "{update}"
    );
    assert_eq!(update["action"], json!({ "type": "openPage", "url": page }));
    app.shot(&root, "1-download-on-the-connection-screen");

    // One request, naming dbee and its real version, and nothing else of dbee's.
    let requests = heard.lock().unwrap().clone();
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert_eq!(requests[0][0], "GET /latest/SHA256SUMS.txt HTTP/1.1");
    let mut headers: Vec<String> = requests[0][1..].iter().map(|h| h.to_ascii_lowercase()).filter(|h| !h.starts_with("host: 127.0.0.1:")).collect();
    headers.sort();
    assert_eq!(headers, [format!("user-agent: dbee/{}", env!("CARGO_PKG_VERSION"))]);

    // The command hands the page back; the button names it (a memory clipboard keeps it in the app).
    assert_eq!(app.execute("app.update", json!({})), json!({ "action": "openPage", "url": page }));
    assert_eq!(app.inspect()["openedUrl"], json!(null));
    app.click("update-button");
    assert_eq!(app.inspect()["openedUrl"], page);

    // A test source's answer is not kept, and the setting is.
    let settings = home.home.join(".config").join("dbee");
    assert!(!settings.join("update-check.json").exists());
    assert_eq!(app.execute("app.setUpdateCheck", json!({ "enabled": false })), json!({ "enabled": false }));
    let kept: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(settings.join("settings.json")).unwrap()).unwrap();
    assert_eq!(kept["checkForUpdates"], false);
    assert_eq!(app.document()["update"]["checks"], "manual");
    app.quit();
    assert_eq!(heard.lock().unwrap().len(), 1, "no second request");
}

/// The switches: `DBEE_NO_UPDATE_CHECK`, a source that is not on this machine (so the
/// development build's rule holds), and no source at all. None of them checks or shows a button.
#[test]
#[ignore = "opens a window on the desktop; run by hand: cargo test -p dbee --test update_check -- --ignored --nocapture"]
fn the_real_app_checks_nothing_when_it_is_told_not_to_or_runs_from_a_build_folder() {
    if no_display() {
        return;
    }
    let root = dbee_testkit::repo_root();
    let (port, heard) = releases("0.3.1");
    let url = format!("http://127.0.0.1:{port}");
    let elsewhere = format!("http://localhost:{port}");
    let cases: [&[(&str, &str)]; 4] = [
        &[("DBEE_UPDATE_MANIFEST_URL", &url), ("DBEE_UPDATE_PRETEND_VERSION", "0.3.0"), ("DBEE_NO_UPDATE_CHECK", "1")],
        &[("DBEE_UPDATE_MANIFEST_URL", &url), ("DBEE_UPDATE_PRETEND_VERSION", "0.3.0"), ("DBEE_NO_PREFS", "1")],
        &[("DBEE_UPDATE_MANIFEST_URL", &elsewhere), ("DBEE_UPDATE_PRETEND_VERSION", "0.3.0")],
        &[],
    ];
    for env in cases {
        let home = temp_home(None, None);
        let mut app = App::start_with_env(&root, &home.home, SHOTS, env);
        let update = app.document()["update"].clone();
        assert_eq!((update["state"].clone(), update["checks"].clone()), (json!("none"), json!("off")), "{env:?}: {update}");
        let refused = app.call("engine.execute", json!({ "command": "app.checkForUpdates" }));
        assert_eq!((refused["ok"].clone(), refused["code"].clone()), (json!(false), json!("disabled")), "{env:?}: {refused}");
        assert!(app.tag("update-button").is_none(), "{env:?}");
        app.quit();
    }
    assert!(heard.lock().unwrap().is_empty(), "nothing was asked");
}
