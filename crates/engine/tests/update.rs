//! The update check and the update button's action, on fakes: a fetch that answers from memory
//! and an install step that the test steers. Nothing here reaches the network.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{T, session_adjusted, session_in_adjusted};
use dbee_engine::{EngineError, Outcome, Session, SessionOptions, UpdateState, Updates};
use dbee_testkit::temp_home;
use dbee_update::{Fetch, Install, InstallKind, Offer, Os, Platform, Progress, Repo, UpdateError, Version};
use serde_json::json;

const HASH: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";

/// A release's manifest with one file, of version `v`.
fn manifest(v: &str) -> Vec<u8> {
    format!("{HASH}  dbee-{v}-linux-x86_64.deb\n").into_bytes()
}

/// Answers from memory, and counts how often it was asked.
struct FakeFetch {
    answer: Mutex<Result<Vec<u8>, UpdateError>>,
    calls: AtomicUsize,
}

impl FakeFetch {
    fn new(answer: Result<Vec<u8>, UpdateError>) -> Arc<FakeFetch> {
        Arc::new(FakeFetch { answer: Mutex::new(answer), calls: AtomicUsize::new(0) })
    }

    fn release(v: &str) -> Arc<FakeFetch> {
        FakeFetch::new(Ok(manifest(v)))
    }

    fn answers(&self, answer: Result<Vec<u8>, UpdateError>) {
        *self.answer.lock().unwrap() = answer;
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Fetch for FakeFetch {
    fn manifest(&self) -> Result<Vec<u8>, UpdateError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.answer.lock().unwrap().clone()
    }
}

/// An install step that reports 42 %, then waits for the test to say how it ends.
struct FakeInstall {
    end: Mutex<Receiver<Result<(), String>>>,
}

impl FakeInstall {
    fn new() -> (Arc<FakeInstall>, Sender<Result<(), String>>) {
        let (tx, rx) = channel();
        (Arc::new(FakeInstall { end: Mutex::new(rx) }), tx)
    }
}

impl Install for FakeInstall {
    fn supports(&self, kind: InstallKind) -> bool {
        kind == InstallKind::Deb
    }

    fn install(&self, _: &Offer, progress: &dyn Fn(Progress)) -> Result<(), UpdateError> {
        progress(Progress::Percent(42));
        self.end.lock().unwrap().recv().map_err(|_| UpdateError::Install("the test went away".into()))?.map_err(UpdateError::Install)
    }
}

/// A 0.3.0 copy installed from the `.deb`, checking by hand only.
fn updates(fetch: &Arc<FakeFetch>) -> Updates {
    Updates {
        fetch: fetch.clone(),
        installer: None,
        current: Version::new(0, 3, 0),
        kind: InstallKind::Deb,
        platform: Platform { os: Os::Linux, arch: "x86_64" },
        repo: Repo::default(),
        automatic: false,
        first_check: Duration::ZERO,
        remember: false,
    }
}

fn with(updates: Updates) -> common::TestSession {
    session_adjusted(|o: &mut SessionOptions| o.updates = Some(updates))
}

fn check(s: &mut Session) -> Result<serde_json::Value, EngineError> {
    s.run_blocking("app.checkForUpdates", json!({}), T)
}

const PAGE: &str = "https://github.com/KasparRosin/dbee/releases/tag/v0.3.1";

#[test]
fn a_session_without_a_checker_never_checks() {
    let mut s = common::session();
    assert_eq!(s.inspect()["update"], json!({ "state": "none", "checks": "off", "enabled": true }));
    let Err(EngineError::Disabled { why, .. }) = s.execute("app.checkForUpdates", json!({})) else { panic!("it checked") };
    assert_eq!(why, "update checks are off in this run");
    let Err(EngineError::Disabled { why, .. }) = s.execute("app.update", json!({})) else { panic!("it updated") };
    assert_eq!(why, "no update is available");
    assert_eq!(s.next_update_check(), None);
    s.pump();
    assert!(!s.busy(), "no check starts by itself");
}

#[test]
fn a_newer_release_is_offered_as_a_download() {
    let fetch = FakeFetch::release("0.3.1");
    let mut s = with(updates(&fetch));
    assert_eq!(s.inspect()["update"], json!({ "state": "none", "checks": "manual", "enabled": true }));
    let answer = check(&mut s).unwrap();
    assert_eq!(answer["checked"], true);
    assert_eq!(answer["current"], "0.3.0");
    assert_eq!(answer["latest"], "0.3.1");
    assert_eq!(answer["available"], true);
    let shown = json!({
        "state": "available", "checks": "manual", "enabled": true,
        "from": "0.3.0", "to": "0.3.1", "kind": "deb", "action": { "type": "openPage", "url": PAGE },
    });
    assert_eq!(answer["update"], shown);
    assert_eq!(s.inspect()["update"], shown);
    assert!(matches!(&s.state().update, UpdateState::Available(o) if o.to == Version::new(0, 3, 1)));
    // The command hands the page back and opens nothing; the offer stays.
    assert_eq!(s.execute("app.update", json!({})).unwrap(), Outcome::Done(json!({ "action": "openPage", "url": PAGE })));
    assert_eq!(s.inspect()["update"], shown);
    assert!(s.take_ui_requests().is_empty(), "a command opens no dialog and shows no notice");
    assert!(s.journal().is_empty(), "neither command is journaled");
}

#[test]
fn every_install_kind_gets_the_download_while_there_is_no_install_step() {
    for kind in InstallKind::ALL {
        let fetch = FakeFetch::release("0.4.0");
        let mut s = with(Updates { kind, ..updates(&fetch) });
        check(&mut s).unwrap();
        let update = &s.inspect()["update"];
        assert_eq!(update["kind"], kind.id());
        assert_eq!(update["action"], json!({ "type": "openPage", "url": "https://github.com/KasparRosin/dbee/releases/tag/v0.4.0" }), "{kind:?}");
    }
}

#[test]
fn the_same_an_older_or_a_pre_release_is_no_offer() {
    for latest in ["0.3.0", "0.2.9", "0.4.0-rc.1"] {
        let fetch = FakeFetch::release(latest);
        let mut s = with(updates(&fetch));
        let answer = check(&mut s).unwrap();
        assert_eq!(answer["available"], false, "{latest}");
        assert_eq!(answer["latest"], latest);
        assert_eq!(s.state().update, UpdateState::None);
        assert!(matches!(s.execute("app.update", json!({})), Err(EngineError::Disabled { .. })));
    }
    // A pre-release copy is offered its final release.
    let fetch = FakeFetch::release("0.3.0");
    let mut s = with(Updates { current: Version::parse("0.3.0-rc.2").unwrap(), ..updates(&fetch) });
    assert_eq!(check(&mut s).unwrap()["available"], true);
}

#[test]
fn a_later_check_that_finds_nothing_newer_takes_the_offer_away() {
    let fetch = FakeFetch::release("0.3.1");
    let mut s = with(updates(&fetch));
    check(&mut s).unwrap();
    assert!(matches!(s.state().update, UpdateState::Available(_)));
    fetch.answers(Ok(manifest("0.3.0")));
    check(&mut s).unwrap();
    assert_eq!(s.state().update, UpdateState::None);
}

#[test]
fn a_manual_check_reports_its_error_and_keeps_the_state() {
    let fetch = FakeFetch::release("0.3.1");
    let mut s = with(updates(&fetch));
    check(&mut s).unwrap();
    let before = s.inspect()["update"].clone();
    fetch.answers(Err(UpdateError::Fetch("the connection failed".into())));
    let e = check(&mut s).unwrap_err();
    assert_eq!(e.code(), "update");
    assert_eq!(e.to_string(), "the update check could not reach the releases: the connection failed");
    for hostile in [&b"<html>Not Found</html>"[..], b"", &[0xff, 0xfe], &vec![b'a'; 100_000], format!("{HASH}  ../../etc/passwd\n").as_bytes()] {
        fetch.answers(Ok(hostile.to_vec()));
        let e = check(&mut s).unwrap_err();
        assert_eq!(e.code(), "update");
        assert!(e.to_string().starts_with("the release's file list could not be read: "), "{e}");
        assert!(!e.to_string().contains("passwd"), "the answer's bytes are not quoted");
    }
    assert_eq!(s.inspect()["update"], before);
}

#[test]
fn only_one_check_runs_at_a_time() {
    let fetch = FakeFetch::release("0.3.1");
    let mut s = with(updates(&fetch));
    let Outcome::Started(job) = s.execute("app.checkForUpdates", json!({})).unwrap() else { panic!("no job") };
    // The first has not been applied yet, whether or not its fetch has ended.
    assert!(matches!(s.execute("app.checkForUpdates", json!({})), Err(EngineError::Busy(_))));
    s.wait(job, T).unwrap();
    assert_eq!(fetch.calls(), 1);
    check(&mut s).unwrap();
    assert_eq!(fetch.calls(), 2);
}

/// Pumps until `done`, as a window's frames do.
fn pump_until(s: &mut Session, done: impl Fn(&Session) -> bool) -> Vec<Result<serde_json::Value, EngineError>> {
    let start = Instant::now();
    let mut results = Vec::new();
    while !done(s) {
        assert!(start.elapsed() < T, "nothing came of it");
        results.extend(s.pump().into_iter().map(|(_, r)| r));
        std::thread::sleep(Duration::from_millis(5));
    }
    results
}

#[test]
fn the_automatic_check_runs_by_itself_after_the_start() {
    let fetch = FakeFetch::release("0.3.1");
    let mut s = with(Updates { automatic: true, ..updates(&fetch) });
    assert_eq!(s.inspect()["update"]["checks"], "automatic");
    assert_eq!(s.next_update_check(), Some(Duration::ZERO));
    let results = pump_until(&mut s, |s| matches!(s.state().update, UpdateState::Available(_)));
    assert!(results.iter().all(Result::is_ok));
    assert_eq!(fetch.calls(), 1);
    // The next one is a day away.
    let next = s.next_update_check().unwrap();
    assert!(next > Duration::from_secs(23 * 60 * 60) && next <= Duration::from_secs(24 * 60 * 60), "{next:?}");
    s.pump();
    assert!(!s.busy());
    assert_eq!(fetch.calls(), 1);
}

#[test]
fn a_failed_automatic_check_is_silent() {
    for answer in [Err(UpdateError::Fetch("the connection failed".into())), Ok(b"<html>".to_vec())] {
        let fetch = FakeFetch::new(answer);
        let mut s = with(Updates { automatic: true, ..updates(&fetch) });
        let results = pump_until(&mut s, |s| fetch.calls() == 1 && !s.busy());
        assert_eq!(results, [Ok(json!({ "checked": false }))], "no error comes out of the job");
        assert_eq!(s.state().update, UpdateState::None);
        assert!(s.take_ui_requests().is_empty(), "no notice and no dialog");
    }
}

#[test]
fn the_setting_turns_the_automatic_check_off_and_is_kept() {
    let home = temp_home(None, None);
    let dir = home.home.join("config");
    let fetch = FakeFetch::release("0.3.1");
    let mut s = session_in_adjusted(home, |o| o.updates = Some(Updates { automatic: true, ..updates(&fetch) }));
    assert!(s.update_check_automatic());
    assert_eq!(s.execute("app.setUpdateCheck", json!({ "enabled": false })).unwrap(), Outcome::Done(json!({ "enabled": false })));
    assert!(!s.update_check_automatic());
    assert_eq!(s.inspect()["update"], json!({ "state": "none", "checks": "manual", "enabled": false }));
    assert_eq!(s.next_update_check(), None);
    s.pump();
    assert!(!s.busy());
    assert_eq!(fetch.calls(), 0, "nothing is asked while the check is off");
    assert!(!dbee_config::load_update_check(Some(&dir)), "settings.json keeps the choice");
    // A check by hand still runs.
    assert_eq!(check(&mut s).unwrap()["available"], true);
    assert_eq!(fetch.calls(), 1);
    // The next start reads the choice.
    let again = FakeFetch::release("0.3.1");
    let mut options = SessionOptions { config_dir: Some(dir.clone()), ..SessionOptions::default() };
    options.updates = Some(Updates { automatic: true, ..updates(&again) });
    let mut second = Session::new(options).unwrap();
    assert!(!second.update_check_automatic());
    second.pump();
    assert_eq!(again.calls(), 0);
    // Turned on again, the check runs.
    second.execute("app.setUpdateCheck", json!({ "enabled": true })).unwrap();
    assert!(dbee_config::load_update_check(Some(&dir)));
    pump_until(&mut second, |s| matches!(s.state().update, UpdateState::Available(_)));
    assert_eq!(again.calls(), 1);
    // Wrong params change nothing.
    for params in [json!({}), json!({ "enabled": null }), json!({ "enabled": "no" }), json!({ "enabled": 0 })] {
        assert!(matches!(second.execute("app.setUpdateCheck", params), Err(EngineError::BadParams { .. })));
    }
    assert!(second.update_check_automatic());
    assert_eq!(second.journal().last().map(|(id, p)| (id.as_str(), p.clone())), Some(("app.setUpdateCheck", json!({ "enabled": true }))));
}

#[test]
fn a_fresh_answer_is_kept_across_a_restart() {
    let home = temp_home(None, None);
    let dir = home.home.join("config");
    let fetch = FakeFetch::release("0.3.1");
    let remembering = |fetch: &Arc<FakeFetch>| Updates { automatic: true, remember: true, first_check: Duration::from_secs(3600), ..updates(fetch) };
    let mut s = session_in_adjusted(home, |o| o.updates = Some(remembering(&fetch)));
    assert!(!dir.join("update-check.json").exists());
    check(&mut s).unwrap();
    let kept = std::fs::read_to_string(dir.join("update-check.json")).unwrap();
    let kept: serde_json::Value = serde_json::from_str(&kept).unwrap();
    assert_eq!(kept["latest"], "0.3.1");
    assert_eq!(kept["version"], 1);
    assert!(kept["checkedAt"].as_u64().unwrap() > 1_700_000_000);

    // The next start shows the offer at once and asks nobody for almost six hours.
    let again = FakeFetch::release("0.3.1");
    let mut second =
        Session::new(SessionOptions { config_dir: Some(dir.clone()), updates: Some(remembering(&again)), ..SessionOptions::default() }).unwrap();
    assert!(matches!(&second.state().update, UpdateState::Available(o) if o.to == Version::new(0, 3, 1)));
    let next = second.next_update_check().unwrap();
    assert!(next > Duration::from_secs(5 * 60 * 60) && next <= Duration::from_secs(6 * 60 * 60), "{next:?}");
    second.pump();
    assert!(!second.busy());
    assert_eq!(again.calls(), 0);

    // A copy that is that release by now is offered nothing.
    let newer = Updates { current: Version::new(0, 3, 1), ..remembering(&again) };
    let third = Session::new(SessionOptions { config_dir: Some(dir.clone()), updates: Some(newer), ..SessionOptions::default() }).unwrap();
    assert_eq!(third.state().update, UpdateState::None);

    // An answer that is old, from the future or no answer at all counts for nothing.
    for text in [
        r#"{"version":1,"checkedAt":1000,"latest":"0.3.1"}"#.to_string(),
        r#"{"version":1,"checkedAt":99999999999,"latest":"0.3.1"}"#.to_string(),
        "garbage{".to_string(),
        " ".repeat(10_000),
    ] {
        std::fs::write(dir.join("update-check.json"), &text).unwrap();
        let s =
            Session::new(SessionOptions { config_dir: Some(dir.clone()), updates: Some(remembering(&again)), ..SessionOptions::default() }).unwrap();
        assert_eq!(s.state().update, UpdateState::None, "{text:.40}");
        assert_eq!(s.next_update_check().map(|d| d <= Duration::from_secs(3600)), Some(true));
    }

    // Without `remember` nothing is read and nothing is written.
    std::fs::remove_file(dir.join("update-check.json")).unwrap();
    let mut forgetful =
        Session::new(SessionOptions { config_dir: Some(dir.clone()), updates: Some(updates(&again)), ..SessionOptions::default() }).unwrap();
    check(&mut forgetful).unwrap();
    assert!(!dir.join("update-check.json").exists());
}

fn installing() -> (common::TestSession, Arc<FakeFetch>, Sender<Result<(), String>>) {
    let fetch = FakeFetch::release("0.4.0");
    let (installer, end) = FakeInstall::new();
    let mut s = with(Updates { installer: Some(installer), ..updates(&fetch) });
    check(&mut s).unwrap();
    assert_eq!(s.inspect()["update"]["action"], json!({ "type": "install", "asset": "dbee-0.4.0-linux-x86_64.deb" }));
    (s, fetch, end)
}

fn wait_for_percent(s: &Session) {
    let start = Instant::now();
    while s.update_percent() != Some(42) {
        assert!(start.elapsed() < T, "no progress came");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn an_install_goes_from_available_through_updating_to_ready() {
    let (mut s, fetch, end) = installing();
    assert_eq!(s.update_percent(), None);
    let Outcome::Started(job) = s.execute("app.update", json!({})).unwrap() else { panic!("no job") };
    assert!(matches!(s.state().update, UpdateState::Updating(_)));
    wait_for_percent(&s);
    assert_eq!(s.inspect()["update"]["state"], "updating");
    assert_eq!(s.inspect()["update"]["percent"], 42);
    // Meanwhile the button's command is busy, and a check's answer leaves the install alone.
    assert!(matches!(s.execute("app.update", json!({})), Err(EngineError::Busy(_))));
    fetch.answers(Ok(manifest("0.3.0")));
    let Outcome::Started(checking) = s.execute("app.checkForUpdates", json!({})).unwrap() else { panic!("no job") };
    assert_eq!(s.wait(checking, T).unwrap()["available"], false);
    assert!(matches!(s.state().update, UpdateState::Updating(_)));
    end.send(Ok(())).unwrap();
    let answer = s.wait(job, T).unwrap();
    assert_eq!(answer["action"], "install");
    assert_eq!(answer["update"]["state"], "ready");
    assert!(matches!(&s.state().update, UpdateState::Ready(o) if o.to == Version::new(0, 4, 0)));
    assert_eq!(s.update_percent(), None);
    assert_eq!(s.execute("app.update", json!({})).unwrap(), Outcome::Done(json!({ "action": "restart" })));
    check(&mut s).unwrap();
    assert!(matches!(s.state().update, UpdateState::Ready(_)), "a later check keeps what is installed");
}

#[test]
fn a_failed_install_says_why_and_a_retry_runs_it_again() {
    let (mut s, _fetch, end) = installing();
    let Outcome::Started(job) = s.execute("app.update", json!({})).unwrap() else { panic!("no job") };
    wait_for_percent(&s);
    end.send(Err("the download's checksum is wrong".into())).unwrap();
    let e = s.wait(job, T).unwrap_err();
    assert_eq!(e.code(), "update");
    assert_eq!(e.to_string(), "the update could not be installed: the download's checksum is wrong");
    let update = s.inspect()["update"].clone();
    assert_eq!(update["state"], "failed");
    assert_eq!(update["reason"], "the update could not be installed: the download's checksum is wrong");
    assert_eq!(update["to"], "0.4.0");
    assert!(update.get("percent").is_none());
    // The retry is the same command.
    let Outcome::Started(job) = s.execute("app.update", json!({})).unwrap() else { panic!("no job") };
    assert!(matches!(s.state().update, UpdateState::Updating(_)));
    end.send(Ok(())).unwrap();
    s.wait(job, T).unwrap();
    assert!(matches!(s.state().update, UpdateState::Ready(_)));
}

#[test]
fn an_install_step_that_goes_away_fails_the_update() {
    let (mut s, _fetch, end) = installing();
    let Outcome::Started(job) = s.execute("app.update", json!({})).unwrap() else { panic!("no job") };
    drop(end);
    assert_eq!(s.wait(job, T).unwrap_err().code(), "update");
    assert!(matches!(s.state().update, UpdateState::Failed { .. }));
}
