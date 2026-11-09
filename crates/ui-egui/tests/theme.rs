//! The modes: in dark, no screen paints a light ground.

mod common;

use common::*;
use serde_json::json;

fn dark(ui: &mut Ui) {
    ui.harness.ctx.set_theme(egui::Theme::Dark);
    ui.frame();
    ui.frame();
}

fn assert_dark(ui: &Ui, at: &str) {
    let light = dbee_ui_egui::theme::LIGHT;
    for (_, shape) in ui.shapes() {
        if let egui::Shape::Rect(r) = shape {
            assert!(r.fill != light.bg && r.fill != light.fill, "{at}: {:?} is a light ground", r.fill);
        }
    }
}

#[test]
fn every_screen_in_dark_paints_no_light_ground() {
    let db = dbee_testkit::require_testdb!();

    let mut ui = Ui::fresh(egui::vec2(1400.0, 900.0));
    dark(&mut ui);
    assert_dark(&ui, "the connection screen");
    ui.click("conn-new");
    ui.frame();
    assert_dark(&ui, "the new-connection form");

    let mut ui = Ui::connected(db);
    dark(&mut ui);
    ui.open("shop", "orders");
    ui.until("the grid", |ui| ui.tag("grid-cell-0-0").is_some());
    ui.click("grid-cell-0-5");
    ui.frame();
    assert_dark(&ui, "a table tab with the value panel");

    ui.execute("tab.newQuery", json!({ "text": "select 1" }));
    ui.frame();
    ui.click("query-run");
    ui.until("the result", |ui| ui.tag("grid-cell-0-0").is_some());
    assert_dark(&ui, "a query tab with a result");

    ui.ctrl("P");
    ui.frame();
    assert!(ui.tag("qs-input").is_some(), "the quick search is open");
    assert_dark(&ui, "the quick search");
    ui.key("Escape");
    ui.frame();

    ui.ok("ui.key", json!({ "key": "Q", "cmd": true, "shift": true }));
    assert_eq!(ui.inspect()["dialog"], "confirm");
    assert_dark(&ui, "a confirm dialog");
}

#[test]
fn a_dialog_sits_on_card_over_the_scrim() {
    let db = dbee_testkit::require_testdb!();
    let d = dbee_ui_egui::theme::DARK;
    let mut ui = Ui::connected(db);
    dark(&mut ui);
    let has = |ui: &Ui, fill: egui::Color32, stroke: Option<egui::Color32>| {
        ui.shapes().iter().any(|(_, s)| matches!(s, egui::Shape::Rect(r) if r.fill == fill && stroke.is_none_or(|c| r.stroke.color == c)))
    };
    ui.ctrl("P");
    ui.frame();
    assert!(ui.tag("qs-input").is_some());
    assert!(has(&ui, d.scrim, None), "the quick search's scrim");
    assert!(has(&ui, d.raised, None), "the quick search on raised");
    ui.key("Escape");
    ui.frame();
    ui.execute("tab.newQuery", json!({ "text": "select 1" }));
    ui.ok("ui.key", json!({ "key": "Q", "cmd": true, "shift": true }));
    assert_eq!(ui.inspect()["dialog"], "confirm");
    assert!(has(&ui, d.scrim, None), "the dialog's scrim");
    assert!(has(&ui, d.card, Some(d.line)), "the dialog on card with a line");
}

fn drawn(ui: &Ui, text: &str) -> bool {
    ui.texts().iter().any(|(_, g)| g.text() == text)
}

#[test]
fn a_connection_with_no_name_keeps_its_case_in_the_status_bar() {
    let db = dbee_testkit::require_testdb!();
    let seed = dbee_testkit::targets_of(&[dbee_testkit::Kind::Sqlite]).pop().expect("the SQLite seed");
    let dir = std::env::temp_dir().join(dbee_testkit::unique("mixed-case"));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("MixedCase.sqlite");
    std::fs::copy(seed.file().unwrap(), &file).unwrap();
    let config = dbee_config::ConnectionConfig { kind: dbee_config::DbKind::Sqlite, file: Some(file), ..Default::default() };
    let mut ui = Ui::connected_with(db, Setup { config: Some(config), ..Setup::default() });
    let shown = ui.value("status-connection");
    let shown = shown.as_str().unwrap();
    assert_eq!(shown, "MixedCase.sqlite");
    assert!(drawn(&ui, shown), "drawn as written: {shown}");
    assert!(!drawn(&ui, &shown.to_uppercase()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_connections_own_name_is_drawn_in_capitals_and_tagged_as_written() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    let shown = ui.value("status-connection");
    let shown = shown.as_str().unwrap();
    assert_eq!(shown, "pg14");
    assert!(drawn(&ui, "PG14"), "the user's name is drawn in capitals");
    assert!(!drawn(&ui, shown));
}

#[test]
fn a_column_name_keeps_its_case_in_the_grid_header() {
    let db = dbee_testkit::require_testdb!();
    let scratch =
        Scratch::new(db, "mixed_col", |name| format!("CREATE TABLE scratch.{name} (\"MixedCase\" int); INSERT INTO scratch.{name} VALUES (1)"));
    let mut ui = Ui::connected(db);
    ui.open("scratch", &scratch.0);
    ui.until("the grid", |ui| ui.tag("grid-header-0").is_some());
    assert_eq!(ui.value("grid-header-0")["name"], "MixedCase");
    assert!(drawn(&ui, "MixedCase"), "drawn as written");
    assert!(!drawn(&ui, "MIXEDCASE"));
}

#[test]
fn the_gutters_numbers_sit_level_with_the_editors_lines() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    ui.execute("tab.newQuery", json!({ "text": "select 1\nselect 2\nselect 3\nselect 4" }));
    ui.frame();
    ui.frame();
    let texts = ui.texts();
    let (rect, editor) = texts.iter().find(|(_, g)| g.text().starts_with("select 1")).expect("the editor's text is drawn");
    assert_eq!(editor.rows.len(), 4);
    for (i, row) in editor.rows.iter().enumerate() {
        assert_eq!(row.row.size.y, 20.0, "an editor line is Code's 20");
        let line_y = rect.min.y + row.pos.y + row.row.size.y / 2.0;
        let number = (i + 1).to_string();
        let (n, _) = texts
            .iter()
            .find(|(r, g)| g.text() == number && r.max.x < rect.min.x && rect.min.x - r.max.x < 24.0)
            .unwrap_or_else(|| panic!("line number {number}"));
        assert!((n.center().y - line_y).abs() < 0.5, "number {number} at {} against its line at {line_y}", n.center().y);
    }
}
