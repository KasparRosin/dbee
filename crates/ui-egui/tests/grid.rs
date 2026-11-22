//! The data grid in a table tab, driven through the control channel one frame at a time in a
//! kittest harness over [`DbeeApp::ui`]: a header too narrow for its words, the types' short
//! spellings in the header and the tree, selecting several whole rows (a drag over the row
//! numbers, the arrow keys) and what each copy puts on the clipboard for them.
//!
//! Safety: every session keeps its connections file in a throwaway home and its secrets in a
//! `MemoryStore`. The tests connect only to the Docker test servers (`require_testdb!`); each
//! makes its own table in `scratch` and drops it, and writes nothing else.

mod common;

use common::*;
use dbee_testkit::TestDb;
use egui::{Event, Modifiers, PointerButton};
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

impl Ui {
    fn selection(&mut self) -> Json {
        self.document()["selection"].clone()
    }

    /// A primary-button drag from the centre of `from` to the centre of `to`, in small steps.
    fn drag(&mut self, from: &str, to: &str) {
        let (a, b) = (self.centre(from), self.centre(to));
        let button = |pos, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        self.frame_with(vec![Event::PointerMoved(a)]);
        self.frame_with(vec![button(a, true)]);
        for k in 1..=6 {
            self.frame_with(vec![Event::PointerMoved(a + (b - a) * (k as f32 / 6.0))]);
        }
        self.frame_with(vec![button(b, false)]);
        self.frame();
    }

    /// What `key` with the Ctrl and Shift given copies.
    fn copy_key(&mut self, shift: bool) -> String {
        self.copied.clear();
        if shift {
            self.ctrl_shift("C");
        } else {
            self.ctrl("C");
        }
        self.frame();
        self.copied().expect("the keys copied")
    }

    /// What the engine renders for the selection in `format`.
    fn rendered(&mut self, format: &str) -> String {
        self.execute("copy.selection", json!({ "format": format }))["text"].as_str().unwrap().to_string()
    }
}

fn shift() -> Json {
    json!({ "shift": true })
}

fn rows(first: usize, last: usize) -> Json {
    json!({ "anchor": [first, 0], "focus": [last, 5], "whole": true })
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

/// Several whole rows: a drag over the row numbers selects them, Shift with an arrow extends
/// them and an arrow alone moves on as one row; the keys and every entry of the menu, opened on a
/// row's number or on a cell of a selected row, copy all of the selected rows.
#[test]
fn several_rows_are_selected_by_drag_and_arrows_and_copied_whole() {
    let db = dbee_testkit::require_testdb!();
    let t = mails(db);
    let mut ui = opened(db, &t);
    ui.drag("grid-row-0", "grid-row-2");
    assert_eq!(ui.selection(), rows(0, 2), "a drag down the numbers");
    assert_eq!((0..4).map(|r| ui.value(&format!("grid-row-{r}"))["selected"] == true).collect::<Vec<_>>(), [true, true, true, false]);
    ui.drag("grid-row-3", "grid-row-2");
    assert_eq!(ui.selection(), rows(3, 2), "and up");
    // A drag that wanders off the numbers still goes by the row the pointer is level with.
    ui.drag("grid-row-1", "grid-cell-2-3");
    assert_eq!(ui.selection(), rows(1, 2));

    // The keys: TSV, and TSV under the column names.
    const FIRST: &str = "1\t2025-01-02 03:04:05\t\tHello\tabc\t\"<p class=\"\"a\"\">\nIt's\tok | fine</p>\"";
    const SECOND: &str = "2\t\t\tSay \"hi\"\tdef\tplain";
    const THIRD: &str = "3\t\t\tThird\tghi\t";
    const NAMES: &str = "id\ta_rather_long_column_name_for_a_date\tsent\tsubject\tcode\tbody";
    ui.drag("grid-row-0", "grid-row-2");
    assert_eq!(ui.copy_key(false), format!("{FIRST}\n{SECOND}\n{THIRD}"));
    assert_eq!(ui.copy_key(true), format!("{NAMES}\n{FIRST}\n{SECOND}\n{THIRD}"));
    // One row.
    ui.click("grid-row-1");
    assert_eq!((ui.copy_key(false), ui.copy_key(true)), (SECOND.to_string(), format!("{NAMES}\n{SECOND}")));

    // The menu on a selected row's number, and on a cell of a selected row: every entry copies the
    // selected rows, which stay selected.
    for opener in ["grid-row-1", "grid-cell-2-3", "grid-cell-0-5"] {
        for (item, format) in [
            ("grid-menu-copy", "tsv"),
            ("grid-menu-copy-header", "tsvHeader"),
            ("grid-menu-copy-csv", "csv"),
            ("grid-menu-copy-json", "json"),
            ("grid-menu-copy-markdown", "markdown"),
        ] {
            ui.execute("selection.rows", json!({ "first": 0, "last": 2 }));
            ui.frame();
            let want = ui.rendered(format);
            ui.copied.clear();
            ui.right_click(opener);
            assert_eq!(ui.selection(), rows(0, 2), "{opener}: the right-click keeps the rows");
            ui.click(item);
            ui.frame();
            assert_eq!(ui.copied().as_deref(), Some(want.as_str()), "{opener} {item}");
            assert_eq!(ui.selection(), rows(0, 2), "{opener} {item}: the rows stay selected");
            assert_eq!(ui.value("notice"), "Copied 3 rows", "{opener} {item}");
        }
    }
    // What a row with a text of several lines is in each format.
    ui.execute("selection.rows", json!({ "first": 0 }));
    assert_eq!(ui.rendered("tsv"), FIRST);
    assert_eq!(
        ui.rendered("csv"),
        "id,a_rather_long_column_name_for_a_date,sent,subject,code,body\n1,2025-01-02 03:04:05,,Hello,abc,\"<p class=\"\"a\"\">\nIt's\tok | fine</p>\""
    );
    assert_eq!(
        ui.rendered("json"),
        "[\n  {\"id\": 1, \"a_rather_long_column_name_for_a_date\": \"2025-01-02 03:04:05\", \"sent\": null, \"subject\": \"Hello\", \"code\": \"abc\", \"body\": \"<p class=\\\"a\\\">\\nIt's\\tok | fine</p>\"}\n]"
    );
    assert_eq!(
        ui.rendered("markdown"),
        "| id | a_rather_long_column_name_for_a_date | sent | subject | code | body |\n|---|---|---|---|---|---|\n| 1 | 2025-01-02 03:04:05 | NULL | Hello | abc | <p class=\"a\"><br>It's\tok \\| fine</p> |"
    );
    // One cell with Ctrl+C is that cell as a TSV field (in quotes, its own doubled, when it has a
    // line break or a tab or starts with a quote; else as it is); the value panel's COPY is the
    // value as stored.
    ui.click("grid-cell-0-5");
    assert_eq!(ui.copy_key(false), "\"<p class=\"\"a\"\">\nIt's\tok | fine</p>\"");
    assert_eq!(ui.execute("value.copy", json!({}))["text"], "<p class=\"a\">\nIt's\tok | fine</p>");

    // The arrows: Shift extends the rows from the anchor's row, an arrow alone moves on as one row.
    ui.click("grid-row-1");
    ui.key_with("ArrowDown", shift());
    assert_eq!(ui.selection(), rows(1, 2));
    ui.key_with("ArrowDown", shift());
    ui.key_with("ArrowDown", shift());
    assert_eq!(ui.selection(), rows(1, 3), "it stops at the last row");
    assert_eq!(ui.copy_key(false).lines().count(), 3, "rows 2 to 4 (none of them with a line break)");
    ui.key_with("ArrowUp", shift());
    ui.key_with("ArrowUp", shift());
    ui.key_with("ArrowUp", shift());
    assert_eq!(ui.selection(), rows(1, 0), "back past the anchor's row");
    ui.key("ArrowDown");
    assert_eq!(ui.selection(), rows(1, 1), "an arrow alone: the next row, alone");
    ui.key_with("ArrowRight", shift());
    assert_eq!(ui.selection(), rows(1, 1), "sideways, whole rows stay as they are");
    // A cell: the arrows move it, Shift extends the rectangle.
    ui.click("grid-cell-1-1");
    ui.key("ArrowRight");
    ui.key("ArrowDown");
    assert_eq!(ui.selection(), json!({ "anchor": [2, 2], "focus": [2, 2], "whole": false }));
    ui.key_with("ArrowUp", shift());
    ui.key_with("ArrowRight", shift());
    assert_eq!(ui.selection(), json!({ "anchor": [2, 2], "focus": [1, 3], "whole": false }));
    assert_eq!(ui.copy_key(false), "\tSay \"hi\"\n\tThird");
    // A text field with the keyboard keeps its arrows.
    ui.click("tree-filter");
    ui.key("ArrowDown");
    assert_eq!(ui.selection(), json!({ "anchor": [2, 2], "focus": [1, 3], "whole": false }));
}
