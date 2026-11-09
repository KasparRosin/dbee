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
