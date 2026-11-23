//! The update button in the window, in a kittest harness over [`DbeeApp::ui`], on fakes: a fetch
//! that answers from memory and an install step the test steers. Nothing here reaches the
//! network, and no page is opened: the tests read what a frame asked the desktop to open.

mod common;

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use dbee_engine::Updates;
use dbee_ui_egui::{Clipboard, DbeeApp};
use dbee_update::{Fetch, Install, InstallKind, Offer, Os, Platform, Progress, Repo, UpdateError, Version};
use serde_json::json;

const HASH: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";

/// Answers with the manifest of one release.
struct FakeFetch(Result<Vec<u8>, UpdateError>);

impl FakeFetch {
    fn release(v: &str) -> Arc<FakeFetch> {
        Arc::new(FakeFetch(Ok(format!("{HASH}  dbee-{v}-linux-x86_64.deb\n").into_bytes())))
    }
}

impl Fetch for FakeFetch {
    fn manifest(&self) -> Result<Vec<u8>, UpdateError> {
        self.0.clone()
    }
}

/// An install step that reports 42 %, then waits for the test to say how it ends.
struct FakeInstall(Mutex<Receiver<Result<(), String>>>);

impl FakeInstall {
    fn new() -> (Arc<FakeInstall>, Sender<Result<(), String>>) {
        let (tx, rx) = channel();
        (Arc::new(FakeInstall(Mutex::new(rx))), tx)
    }
}

impl Install for FakeInstall {
    fn supports(&self, kind: InstallKind) -> bool {
        kind == InstallKind::Deb
    }

    fn install(&self, _: &Offer, progress: &dyn Fn(Progress)) -> Result<(), UpdateError> {
        progress(Progress::Percent(42));
        self.0.lock().unwrap().recv().map_err(|_| UpdateError::Install("the test went away".into()))?.map_err(UpdateError::Install)
    }
}

/// A 0.3.0 copy installed from the `.deb` that checks by itself at once, and finds `latest`.
fn updates(latest: &str) -> Updates {
    Updates {
        fetch: FakeFetch::release(latest),
        installer: None,
        current: Version::new(0, 3, 0),
        kind: InstallKind::Deb,
        platform: Platform { os: Os::Linux, arch: "x86_64" },
        repo: Repo::default(),
        automatic: true,
        first_check: Duration::ZERO,
        remember: false,
    }
}

fn window(updates: Option<Updates>, size: egui::Vec2) -> Ui {
    Ui::window(Setup { size, updates, ..Setup::default() })
}

const SIZE: egui::Vec2 = egui::vec2(1100.0, 720.0);
/// The harness's room around the window, either side.
const MARGIN: f32 = 8.0;
/// Where the button's right end is in a window `width` wide: 12 from the bar's.
fn right_end(width: f32) -> f32 {
    width - MARGIN - 12.0
}
const PAGE: &str = "https://github.com/KasparRosin/dbee/releases/tag/v0.3.1";

/// Opens a SQLite file, so the window shows the workspace.
fn open_workspace(ui: &mut Ui) {
    let file = dbee_testkit::sqlite_seeded(&ui.home.home.join("files"));
    ui.execute("connection.openFile", json!({ "path": file }));
    ui.until("the workspace", |ui| ui.inspect()["screen"] == "workspace");
    ui.frame();
}

/// The button's box and what it says, when it is drawn.
fn button(ui: &mut Ui) -> Option<(egui::Rect, serde_json::Value, bool)> {
    let tag = ui.tag("update-button")?;
    let f = |i: usize| tag["rect"][i].as_f64().unwrap() as f32;
    let rect = egui::Rect::from_min_size(egui::pos2(f(0), f(1)), egui::vec2(f(2), f(3)));
    Some((rect, tag["value"].clone(), tag["enabled"] != false))
}

/// The words drawn inside `rect`.
fn words_in(ui: &Ui, rect: egui::Rect) -> Vec<String> {
    ui.texts().into_iter().filter(|(at, _)| rect.expand(1.0).contains_rect(*at)).map(|(_, galley)| galley.text().to_owned()).collect()
}

fn right_of(ui: &mut Ui, id: &str) -> f32 {
    let tag = ui.tag(id).unwrap_or_else(|| panic!("no `{id}`"));
    (tag["rect"][0].as_f64().unwrap() + tag["rect"][2].as_f64().unwrap()) as f32
}

#[test]
fn nothing_is_shown_with_no_update() {
    // A session that never checks, on both screens.
    let mut ui = window(None, SIZE);
    assert!(button(&mut ui).is_none());
    assert_eq!(ui.document()["update"], json!({ "state": "none", "checks": "off", "enabled": true }));
    open_workspace(&mut ui);
    assert!(button(&mut ui).is_none());
    // A session that checked and found this copy the newest.
    let mut ui = window(Some(updates("0.3.0")), SIZE);
    ui.until("the check", |ui| {
        !ui.harness.state().session().busy() && ui.harness.state().session().next_update_check().is_some_and(|d| d > Duration::from_secs(60))
    });
    ui.frame();
    assert!(button(&mut ui).is_none());
    assert!(ui.tag("update-retry").is_none());
    assert!(!ui.texts().iter().any(|(_, g)| g.text().contains("DOWNLOAD") || g.text().contains("UPDATE")));
    assert!(ui.toast().is_none(), "the check says nothing");
}

#[test]
fn download_appears_at_the_bars_right_end_on_both_screens() {
    let mut ui = window(Some(updates("0.3.1")), SIZE);
    ui.until("the button", |ui| button(ui).is_some());
    for screen in ["connections", "workspace"] {
        if screen == "workspace" {
            open_workspace(&mut ui);
        }
        assert_eq!(ui.inspect()["screen"], screen);
        let (rect, value, enabled) = button(&mut ui).unwrap();
        assert_eq!(value, json!({ "state": "download", "label": "Download 0.3.1" }), "{screen}");
        assert!(enabled);
        assert_eq!(words_in(&ui, rect), ["DOWNLOAD 0.3.1"], "{screen}");
        assert!((rect.right() - right_end(SIZE.x)).abs() <= 0.5, "{screen}: 12 from the bar's right end: {rect:?}");
        assert!(
            (rect.height() - 24.0).abs() <= 0.5 && rect.top() >= MARGIN && rect.bottom() <= MARGIN + 30.0,
            "{screen}: a 24 box in the 30 bar: {rect:?}"
        );
        let titles = right_of(&mut ui, "menu-Query").max(right_of(&mut ui, "menu-View"));
        assert!(rect.left() >= titles + 12.0, "{screen}: clear of the titles");
        assert!(ui.tag("update-retry").is_none());
    }
    assert_eq!(ui.document()["update"]["action"], json!({ "type": "openPage", "url": PAGE }));
    assert!(ui.toast().is_none());
}

#[test]
fn a_click_opens_the_releases_page() {
    let mut ui = window(Some(updates("0.3.1")), SIZE);
    ui.until("the button", |ui| button(ui).is_some());
    assert_eq!(ui.inspect()["openedUrl"], json!(null));
    // The command hands the page back and opens nothing.
    assert_eq!(ui.execute("app.update", json!({})), json!({ "action": "openPage", "url": PAGE }));
    ui.frame();
    assert!(ui.opened.is_empty());
    assert_eq!(ui.inspect()["openedUrl"], json!(null));
    // The button opens it.
    ui.click("update-button");
    ui.frame();
    assert_eq!(ui.opened, [PAGE]);
    assert_eq!(ui.inspect()["openedUrl"], PAGE);
    assert_eq!(button(&mut ui).unwrap().1["state"], "download", "the offer stays");
    assert!(ui.toast().is_none());
    // The keyboard presses it too.
    ui.focus("update-button");
    ui.key("Enter");
    ui.frame();
    assert_eq!(ui.opened, [PAGE, PAGE]);
}

/// With the app's clipboard in memory (an isolated run) nothing leaves the app: the page is not
/// opened, and `ui.inspect` says which it would have been.
#[test]
fn a_click_in_an_isolated_window_opens_nothing() {
    let mut ui = Ui::window(Setup {
        size: SIZE,
        updates: Some(updates("0.3.1")),
        app: Some(Box::new(|session, rx| DbeeApp::new(session, Some(rx)).with_clipboard(Clipboard::Memory))),
        ..Setup::default()
    });
    ui.until("the button", |ui| button(ui).is_some());
    ui.click("update-button");
    ui.frame();
    assert!(ui.opened.is_empty());
    assert_eq!(ui.inspect()["openedUrl"], PAGE);
}

#[test]
fn an_install_shows_update_updating_and_restart() {
    let (installer, end) = FakeInstall::new();
    let mut ui = window(Some(Updates { installer: Some(installer), ..updates("0.4.0") }), SIZE);
    ui.until("the button", |ui| button(ui).is_some());
    let (rect, value, enabled) = button(&mut ui).unwrap();
    assert_eq!(value, json!({ "state": "update", "label": "Update 0.3.0 → 0.4.0" }));
    assert!(enabled);
    assert_eq!(words_in(&ui, rect), ["UPDATE 0.3.0 → 0.4.0"]);
    assert!((rect.right() - right_end(SIZE.x)).abs() <= 0.5);

    ui.click("update-button");
    ui.until("the percent", |ui| button(ui).is_some_and(|(_, value, _)| value["label"] == "Updating 42%"));
    let (rect, value, enabled) = button(&mut ui).unwrap();
    assert_eq!(value, json!({ "state": "updating", "label": "Updating 42%" }));
    assert!(!enabled, "no button while it installs");
    assert_eq!(words_in(&ui, rect), ["UPDATING 42%"]);
    assert!((rect.right() - right_end(SIZE.x)).abs() <= 0.5, "still 12 from the end");
    assert!(ui.opened.is_empty());

    end.send(Ok(())).unwrap();
    ui.until("the restart", |ui| button(ui).is_some_and(|(_, value, _)| value["state"] == "restart"));
    let (rect, value, enabled) = button(&mut ui).unwrap();
    assert_eq!(value, json!({ "state": "restart", "label": "Restart to update" }));
    assert!(enabled);
    assert_eq!(words_in(&ui, rect), ["RESTART TO UPDATE"]);
    assert!(ui.toast().is_none());
}

#[test]
fn a_failed_install_shows_update_failed_its_reason_and_retry() {
    let (installer, end) = FakeInstall::new();
    let mut ui = window(Some(Updates { installer: Some(installer), ..updates("0.4.0") }), SIZE);
    ui.until("the button", |ui| button(ui).is_some());
    ui.click("update-button");
    ui.until("the percent", |ui| button(ui).is_some_and(|(_, value, _)| value["label"] == "Updating 42%"));
    end.send(Err("the download's checksum is wrong".into())).unwrap();
    ui.until("the failure", |ui| button(ui).is_some_and(|(_, value, _)| value["state"] == "failed"));
    let reason = "the update could not be installed: the download's checksum is wrong";
    let (rect, value, enabled) = button(&mut ui).unwrap();
    assert_eq!(value, json!({ "state": "failed", "label": "Update failed", "reason": reason }));
    assert!(!enabled, "the words are no button");
    assert_eq!(words_in(&ui, rect), ["UPDATE FAILED"]);
    let danger = if ui.inspect()["theme"]["mode"] == "dark" { dbee_ui_egui::theme::DARK.danger } else { dbee_ui_egui::theme::LIGHT.danger };
    let said: Vec<_> =
        ui.shapes().into_iter().filter(|(_, shape)| matches!(shape, egui::Shape::Text(t) if t.galley.text() == "UPDATE FAILED")).collect();
    assert_eq!(text_colours(&said), [danger], "in the danger colour");
    let retry = ui.tag("update-retry").unwrap();
    assert_eq!(retry["value"], "Retry");
    let retry_right = right_of(&mut ui, "update-retry");
    assert!((retry_right - right_end(SIZE.x)).abs() <= 0.5, "RETRY ends 12 from the bar's end");
    assert!(retry["rect"][0].as_f64().unwrap() as f32 >= rect.right(), "RETRY after the words");
    assert!(ui.toast().is_none(), "the button says it, no toast");
    // The reason on hover.
    ui.rest(rect.center());
    assert!(ui.texts().iter().any(|(_, g)| g.text() == reason), "the reason on hover");
    ui.hover_away();
    // RETRY runs the install again.
    ui.click("update-retry");
    ui.until("the percent", |ui| button(ui).is_some_and(|(_, value, _)| value["label"] == "Updating 42%"));
    assert!(ui.tag("update-retry").is_none());
    end.send(Ok(())).unwrap();
    ui.until("the restart", |ui| button(ui).is_some_and(|(_, value, _)| value["state"] == "restart"));
}

/// A window too narrow for the titles and the button draws the titles alone; with room again
/// the button is back.
#[test]
fn a_narrow_window_keeps_the_titles_and_drops_the_button() {
    let (installer, _end) = FakeInstall::new();
    for (width, shown) in [(1100.0, true), (640.0, true), (300.0, false), (220.0, false)] {
        let mut ui = window(Some(Updates { installer: Some(installer.clone()), ..updates("0.4.0") }), egui::vec2(width, 420.0));
        ui.until("the offer", |ui| ui.document()["update"]["state"] == "available");
        ui.frame();
        let titles = right_of(&mut ui, "menu-Query");
        match button(&mut ui) {
            Some((rect, ..)) => {
                assert!(shown, "{width}: drawn at {rect:?}");
                assert!(rect.left() >= titles + 12.0, "{width}: clear of the titles: {rect:?} after {titles}");
                assert!((rect.right() - right_end(width)).abs() <= 0.5, "{width}");
            }
            None => {
                assert!(!shown, "{width}: not drawn");
                assert!(!ui.texts().iter().any(|(_, g)| g.text().starts_with("UPDATE")), "{width}: none of its words either");
            }
        }
    }
}
