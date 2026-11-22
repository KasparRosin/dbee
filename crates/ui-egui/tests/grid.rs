//! The data grid in a table tab, driven through the control channel one frame at a time in a
//! kittest harness over [`DbeeApp::ui`]: a header too narrow for its words, and the types' short
//! spellings in the header and the tree.
//!
//! Safety: every session keeps its connections file in a throwaway home and its secrets in a
//! `MemoryStore`. The tests connect only to the Docker test servers (`require_testdb!`); each
//! makes its own table in `scratch` and drops it, and writes nothing else.

mod common;

use common::*;
use dbee_testkit::TestDb;
use serde_json::{Value as Json, json};

/// `scratch.<t>`: a key, two timestamps with long type names (one under a name too long for its
/// column), two character types, and a text whose first value has line breaks, a tab and quotes.
fn mails(db: &TestDb) -> Scratch {
    Scratch::new(db, "t_grid", |t| {
        format!(
            "CREATE TABLE scratch.{t} (id int PRIMARY KEY, a_rather_long_column_name_for_a_date timestamp without time zone,
                 sent timestamp(3) with time zone, subject character varying(255), code character(3), body text);
             INSERT INTO scratch.{t} VALUES
                 (1, '2025-01-02 03:04:05', NULL, 'Hello', 'abc', E'<p class=\"a\">\\nIt''s\\tok | fine</p>'),
                 (2, NULL, NULL, 'Say \"hi\"', 'def', 'plain'),
                 (3, NULL, NULL, 'Third', 'ghi', NULL),
                 (4, NULL, NULL, 'Fourth', 'jkl', '');"
        )
    })
}

fn opened(db: &TestDb, t: &Scratch) -> Ui {
    let mut ui = Ui::connected_in(db, egui::vec2(1500.0, 800.0));
    ui.open("scratch", &t.0);
    ui.until("the page", |ui| ui.table()["rows"] == 4);
    ui.frame();
    ui
}

/// The header draws PostgreSQL's long type names in their short spelling, cuts a name that does
/// not fit with `…`, and says the whole name and type on hover; the tree's column rows draw the
/// same short types. The tags keep the whole type.
#[test]
fn headers_show_short_types_and_say_the_whole_on_hover() {
    let db = dbee_testkit::require_testdb!();
    let t = mails(db);
    let mut ui = opened(db, &t);
    let shown: Vec<(Json, Json, Json)> = (1..5)
        .map(|c| {
            let v = ui.value(&format!("grid-header-{c}"));
            (v["type"].clone(), v["shown"].clone(), v["cut"].clone())
        })
        .collect();
    assert_eq!(
        shown,
        [
            (json!("timestamp without time zone"), json!("timestamp"), json!(true)),
            (json!("timestamp(3) with time zone"), json!("timestamptz(3)"), json!(false)),
            (json!("character varying(255)"), json!("varchar(255)"), json!(false)),
            (json!("character(3)"), json!("char(3)"), json!(false)),
        ]
    );
    assert_eq!(ui.value("grid-header-0")["shown"], "integer", "a type with no short spelling stays");
    // The long name's header: 320 wide at most, its name cut; nothing of it is drawn past its
    // cell, at rest or hovered, and hovered both arrows are whole inside it.
    let head = ui.drawn("grid-header-1");
    assert!(head.width() <= 320.5, "{head:?}");
    let words = |ui: &Ui| -> Vec<(String, egui::Rect, bool)> {
        ui.texts().into_iter().filter(|(r, _)| head.expand(0.5).contains(r.center())).map(|(r, g)| (g.text().to_string(), r, g.elided)).collect()
    };
    let resting = words(&ui);
    assert!(resting.iter().any(|(w, _, _)| w == "a_rather_long_column_name_for_a_date"), "the name: {resting:?}");
    assert!(resting.iter().any(|(_, _, cut)| *cut), "what does not fit is cut with an ellipsis: {resting:?}");
    assert!(resting.iter().all(|(_, r, _)| r.right() <= head.right() - 8.0), "the words end before the mark's room: {resting:?}");
    assert!(!resting.iter().any(|(w, _, _)| w.contains("without time zone")), "the short type: {resting:?}");
    ui.rest(head.center());
    let hovered = words(&ui);
    assert_eq!(hovered, resting, "hovering moves and cuts nothing");
    let arrows: Vec<egui::Rect> = ui
        .shapes()
        .into_iter()
        .filter(|(_, s)| matches!(s, egui::Shape::Path(_) | egui::Shape::LineSegment { .. }))
        .map(|(clip, s)| s.visual_bounding_rect().intersect(clip))
        .filter(|r| head.contains(r.center()) && r.center().x > head.right() - 24.0 && r.height() < 12.0 && r.height() > 2.0)
        .collect();
    let mark = arrows.iter().fold(egui::Rect::NOTHING, |a, b| a.union(*b));
    let words_end = hovered.iter().map(|(_, r, _)| r.right()).fold(head.left(), f32::max);
    assert!(
        mark.width() >= 11.0 && mark.left() >= words_end && mark.right() <= head.right() - 1.0,
        "both hover arrows, whole, between the words (ending {words_end}) and the cell's edge: {mark:?} of {arrows:?} in {head:?}"
    );
    // The hover text: the whole name and type as the database writes it, then what a click does.
    let said: Vec<String> = ui.texts().into_iter().map(|(_, g)| g.text().to_string()).filter(|w| w.contains("Sort by this column")).collect();
    assert_eq!(said, ["a_rather_long_column_name_for_a_date timestamp without time zone\nSort by this column"]);
    // A header that shows everything keeps the one line.
    let id = ui.drawn("grid-header-0").center();
    ui.rest(id);
    let said: Vec<String> = ui.texts().into_iter().map(|(_, g)| g.text().to_string()).filter(|w| w.contains("Sort by this column")).collect();
    assert_eq!(said, ["Sort by this column"]);
    // The tree's column rows: the same short types drawn, the whole type in the tag.
    ui.click("tree-schema-scratch");
    ui.frame();
    ui.click(&format!("tree-expand-scratch.{}", t.0));
    ui.frame();
    let column = format!("tree-column-scratch.{}.sent", t.0);
    assert_eq!(ui.value(&column)["type"], "timestamp(3) with time zone");
    let row = ui.drawn(&column);
    let drawn: Vec<String> = ui.texts().into_iter().filter(|(r, _)| row.contains(r.center())).map(|(_, g)| g.text().to_string()).collect();
    assert!(drawn.iter().any(|w| w == "timestamptz(3)") && !drawn.iter().any(|w| w.contains("with time zone")), "{drawn:?}");
}
