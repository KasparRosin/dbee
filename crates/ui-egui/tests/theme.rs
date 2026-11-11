//! The modes: in dark, no screen paints a light ground.

mod common;

use std::path::PathBuf;

use common::*;
use dbee_ui_egui::DbeeApp;
use serde_json::json;

fn dark(ui: &mut Ui) {
    ui.execute("ui.themeDark", json!({}));
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

/// `top` (premultiplied, as egui keeps it) laid over an opaque `ground`.
fn over(top: egui::Color32, ground: egui::Color32) -> egui::Color32 {
    let a = f32::from(top.a()) / 255.0;
    let mix = |t: u8, g: u8| (f32::from(t) + f32::from(g) * (1.0 - a)).round() as u8;
    egui::Color32::from_rgb(mix(top.r(), ground.r()), mix(top.g(), ground.g()), mix(top.b(), ground.b()))
}

/// The rect of automation entry `id`, drawn in a frame run for it; that frame is the last one.
fn rect_of(ui: &mut Ui, id: &str) -> egui::Rect {
    let tag = ui.tag(id).unwrap_or_else(|| panic!("no widget `{id}`"));
    let f = |i: usize| tag["rect"][i].as_f64().unwrap() as f32;
    egui::Rect::from_min_size(egui::pos2(f(0), f(1)), egui::vec2(f(2), f(3)))
}

/// The fills of the rects the last frame drew within `within`, over its centre.
fn fills_in(ui: &Ui, within: egui::Rect) -> Vec<egui::Color32> {
    ui.shapes()
        .into_iter()
        .filter_map(|(_, s)| match s {
            egui::Shape::Rect(r) if within.expand(1.0).contains_rect(r.rect) && r.rect.contains(within.center()) => Some(r.fill),
            _ => None,
        })
        .collect()
}

/// Automation entry `id` is drawn on `raised_hover`, which shows over `raised`.
fn assert_marked(ui: &mut Ui, id: &str) {
    let d = dbee_ui_egui::theme::DARK;
    let rect = rect_of(ui, id);
    let fills = fills_in(ui, rect);
    assert!(fills.contains(&d.raised_hover), "{id}: fills {fills:?}, not raised_hover");
    assert_ne!(over(d.raised_hover, d.raised), d.raised);
}

/// A menu item's column for the chosen mark: 14 square, 8 in from its left end.
fn mark_column(item: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_size(egui::pos2(item.left() + 8.0, item.top() + 7.0), egui::vec2(14.0, 14.0))
}

/// The colours of the lines and discs (an icon) the last frame drew within `within`, over the
/// last opaque ground drawn there (a menu's), so not what lies under the menu.
fn icon_colours(ui: &Ui, within: egui::Rect) -> Vec<egui::Color32> {
    let shapes = ui.shapes();
    let ground = shapes.iter().rposition(|(_, s)| matches!(s, egui::Shape::Rect(r) if r.fill.a() == 255 && r.rect.contains_rect(within)));
    let mut out = Vec::new();
    for (_, s) in shapes.into_iter().skip(ground.map_or(0, |g| g + 1)) {
        let drawn = match &s {
            egui::Shape::Path(p) => vec![p.fill, if let egui::epaint::ColorMode::Solid(c) = p.stroke.color { c } else { egui::Color32::RED }],
            egui::Shape::Circle(c) => vec![c.fill, c.stroke.color],
            egui::Shape::LineSegment { stroke, .. } => vec![stroke.color],
            _ => continue,
        };
        if !within.contains_rect(s.visual_bounding_rect()) {
            continue;
        }
        for c in drawn.into_iter().filter(|c| *c != egui::Color32::TRANSPARENT) {
            if !out.contains(&c) {
                out.push(c);
            }
        }
    }
    out
}

/// The colour `words` are drawn in within `within` in the last frame.
fn colour_of(ui: &Ui, within: egui::Rect, words: &str) -> Option<egui::Color32> {
    ui.shapes().into_iter().find_map(|(_, s)| match s {
        egui::Shape::Text(t) if t.galley.text() == words && within.contains_rect(t.galley.rect.translate(t.pos.to_vec2())) => {
            let section = t.galley.job.sections.first()?;
            Some(t.override_text_color.unwrap_or(if section.format.color == egui::Color32::PLACEHOLDER {
                t.fallback_color
            } else {
                section.format.color
            }))
        }
        _ => None,
    })
}

#[test]
fn popups_mark_hovered_and_chosen_items_and_draw_lines_on_raised_in_dark() {
    let db = dbee_testkit::require_testdb!();
    let d = dbee_ui_egui::theme::DARK;

    // A menu: the entry under the pointer, and a submenu's button while it is open.
    let mut ui = Ui::connected(db);
    dark(&mut ui);
    ui.click("menu-View");
    ui.frame();
    let entry = rect_of(&mut ui, "menu-ui.toggleValuePanel");
    ui.hover(entry.center());
    assert_marked(&mut ui, "menu-ui.toggleValuePanel");
    let theme = rect_of(&mut ui, "menu-View-Theme");
    ui.hover(theme.center());
    ui.frame();
    let light = rect_of(&mut ui, "menu-ui.themeLight");
    ui.hover(light.center());
    assert_marked(&mut ui, "menu-ui.themeLight");
    assert_marked(&mut ui, "menu-View-Theme");
    ui.key("Escape");
    ui.frame();

    // A list: the chosen column, marked in the left column (the design's menu with choices).
    ui.open("shop", "orders");
    ui.until("the grid", |ui| ui.tag("grid-cell-0-1").is_some());
    let chosen = ui.value("filter-0-column");
    ui.click("filter-0-column");
    ui.frame();
    let at = rect_of(&mut ui, &format!("filter-0-column-{}", chosen.as_str().unwrap()));
    assert_eq!(icon_colours(&ui, mark_column(at)), [d.text], "the chosen column's mark");
    ui.key("Escape");
    ui.frame();

    // The grid's menu: its two lines.
    ui.right_click("grid-cell-0-1");
    ui.frame();
    let (top, copy, bottom) = (rect_of(&mut ui, "grid-menu-null"), rect_of(&mut ui, "grid-menu-copy"), rect_of(&mut ui, "grid-menu-delete"));
    let lines: Vec<egui::Color32> = ui
        .shapes()
        .into_iter()
        .filter_map(|(_, s)| match s {
            // Across the menu, as wide as its entries (the grid's lines under it are a cell wide).
            egui::Shape::LineSegment { points: [a, b], stroke }
                if (a.y - b.y).abs() < 0.5
                    && a.y > top.top()
                    && a.y < bottom.bottom()
                    && ((a.x - b.x).abs() - copy.width()).abs() < 2.0
                    && (a.x.min(b.x) - copy.left()).abs() < 2.0 =>
            {
                Some(stroke.color)
            }
            _ => None,
        })
        .collect();
    assert_eq!(lines, [d.raised_line, d.raised_line], "the menu's lines");
    drop(ui);

    // The connection form's database list: the row the keyboard moved to.
    let mut ui = Ui::fresh(APP_SIZE);
    dark(&mut ui);
    let id = ui.execute("connection.save", json!({ "config": pg14(db), "password": db.password }))["id"].as_str().unwrap().to_string();
    ui.frame();
    ui.click(&format!("conn-saved-{id}"));
    ui.click("conn-database-list");
    ui.until("the list", |ui| ui.tag("conn-database-item-1").is_some());
    ui.key("ArrowDown");
    ui.frame();
    assert_marked(&mut ui, "conn-database-item-1");
}

#[test]
fn a_tabs_close_button_takes_hover_under_the_pointer() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    ui.open("shop", "orders");
    let close = ids(&ui.inspect()).into_iter().find(|id| id.starts_with("tab-close-")).expect("the tab's close button");
    let at = rect_of(&mut ui, &close);
    ui.hover(at.center());
    let rect = rect_of(&mut ui, &close);
    assert!(fills_in(&ui, rect).contains(&dbee_ui_egui::theme::LIGHT.hover), "{:?}", fills_in(&ui, rect));
}

#[test]
fn a_dialog_sits_on_raised_over_the_scrim() {
    let db = dbee_testkit::require_testdb!();
    let d = dbee_ui_egui::theme::DARK;
    let l = dbee_ui_egui::theme::LIGHT;
    let mut ui = Ui::connected(db);
    dark(&mut ui);
    let has = |ui: &Ui, fill: egui::Color32, stroke: Option<egui::Color32>| {
        ui.shapes().iter().any(|(_, s)| matches!(s, egui::Shape::Rect(r) if r.fill == fill && stroke.is_none_or(|c| r.stroke.color == c)))
    };
    let shadowed =
        |ui: &Ui, color: egui::Color32| ui.shapes().iter().any(|(_, s)| matches!(s, egui::Shape::Rect(r) if r.fill == color && r.blur_width == 18.0));
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
    assert!(has(&ui, d.raised, Some(d.line)), "the dialog on raised with a line");
    assert!(shadowed(&ui, d.shadow), "the dialog's shadow");
    ui.execute("ui.themeLight", json!({}));
    ui.frame();
    assert!(has(&ui, l.scrim, None), "the light dialog's scrim");
    assert!(has(&ui, l.raised, Some(l.line)), "the light dialog on raised with a line");
    assert!(shadowed(&ui, l.shadow), "the light dialog's shadow");
}

/// Review Focus 5: an open dialog, its scrim, its buttons and its field recolour on the very next
/// frame after a theme switch.
#[test]
fn a_dialog_recolours_on_a_theme_switch() {
    let mut ui = window_with(None, None, None);
    ui.click("conn-kind-postgres");
    ui.execute("ui.themeLight", json!({}));
    ui.click("conn-fill-url");
    ui.text("postgres://db.example/shop");
    let window = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1100.0, 720.0));
    for (mode, t) in [("light", dbee_ui_egui::theme::LIGHT), ("dark", dbee_ui_egui::theme::DARK)] {
        if mode == "dark" {
            ui.execute("ui.themeDark", json!({}));
            ui.frame();
        }
        let rects: Vec<egui::epaint::RectShape> = ui
            .shapes()
            .into_iter()
            .filter_map(|(_, s)| match s {
                egui::Shape::Rect(r) => Some(r),
                _ => None,
            })
            .collect();
        assert!(rects.iter().any(|r| r.rect == window && r.fill == t.scrim), "{mode}: the scrim over the window");
        // Read from the app's tags: a control request would draw another frame first.
        let tagged = |ui: &Ui, id: &str| ui.harness.state().tags().rect(id).unwrap_or_else(|| panic!("{mode}: `{id}` drawn"));
        let ok = tagged(&ui, "dialog-ok");
        // In light the connection card under the scrim is `raised` with a `line` edge too.
        let frame = rects.iter().rev().find(|r| r.rect.contains_rect(ok) && r.fill == t.raised && r.stroke.color == t.line && r.stroke.width == 1.0);
        let frame = frame.unwrap_or_else(|| panic!("{mode}: the dialog on raised with a line edge"));
        assert!(rects.iter().any(|r| r.blur_width == 18.0 && r.fill == t.shadow && r.rect.intersects(frame.rect)), "{mode}: the dialog's shadow");
        assert_eq!(fills_in(&ui, ok), [t.ink], "{mode}: Fill on ink");
        assert_eq!(colour_of(&ui, ok, "FILL"), Some(t.on_ink), "{mode}: its words on_ink");
        let cancel = tagged(&ui, "dialog-cancel");
        assert_eq!(colour_of(&ui, cancel, "CANCEL"), Some(t.text), "{mode}: Cancel's words");
        assert!(rects.iter().any(|r| r.rect == cancel && r.stroke.color == t.line_strong), "{mode}: Cancel's outline");
        assert_eq!(colour_of(&ui, frame.rect, "Fill from URL"), Some(t.text), "{mode}: the title");
        let url = tagged(&ui, "dialog-url");
        assert!(rects.iter().any(|r| r.rect == url && r.fill == t.card), "{mode}: the field's ground");
    }
}

/// Where the last frame painted a fade: a mesh whose corners differ in colour, as egui paints over
/// the ends of a scroll area that has more to scroll to. dbee paints no gradient of its own.
fn fades(ui: &Ui) -> Vec<egui::Rect> {
    ui.shapes()
        .into_iter()
        .filter_map(|(_, s)| match s {
            egui::Shape::Mesh(m) if m.vertices.iter().any(|v| v.color.a() > 0) && m.vertices.windows(2).any(|w| w[0].color != w[1].color) => {
                Some(m.calc_bounds())
            }
            _ => None,
        })
        .collect()
}

/// The wheel over automation entry `over`, `delta` points, then frames for egui to ease the scroll.
fn wheel(ui: &mut Ui, over: &str, delta: egui::Vec2) {
    let at = rect_of(ui, over).center();
    ui.hover(at);
    ui.frame_with(vec![egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta,
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
    }]);
    for _ in 0..12 {
        ui.frame();
    }
}

/// Foundations: flat. egui fades the ends of a scroll area that has more to scroll to; the
/// connection form scrolled part way paints none.
#[test]
fn the_connection_form_paints_no_fade() {
    let mut ui = Ui::fresh(egui::vec2(1100.0, 560.0));
    ui.click("conn-kind-postgres");
    ui.frame();
    // Opening the SSH box scrolls the form to it, part way: fields hidden above and below.
    ui.click("conn-ssh-on");
    ui.frame();
    let hidden = |ui: &mut Ui, id: &str| ui.tag(id).is_some_and(|e| e["visible"] == false);
    assert!(hidden(&mut ui, "conn-name") && hidden(&mut ui, "conn-ssh-user"), "the form is scrolled part way");
    assert_eq!(fades(&ui), [], "the connection form");
}

/// The workspace's scroll areas, scrolled part way, paint no fade: the tree, the grid (its rows
/// and its columns), the query editor and the quick search's list.
#[test]
fn the_workspaces_scroll_areas_paint_no_fade() {
    let db = dbee_testkit::require_testdb!();
    let mut found = Vec::new();
    let mut ui = Ui::connected_in(db, APP_SIZE);
    ui.click("tree-schema-many");
    ui.until("the many schema's tables", |ui| ids(&ui.inspect()).iter().filter(|id| id.starts_with("tree-table-many.")).count() > 30);
    let before = rect_of(&mut ui, "tree-schema-many");
    wheel(&mut ui, "tree-schema-many", egui::vec2(0.0, -200.0));
    assert!(!visible(&ui.tag("tree-schema-many")) || rect_of(&mut ui, "tree-schema-many").top() < before.top(), "the tree scrolled");
    found.push(("the tree", fades(&ui)));

    ui.open("shop", "orders");
    ui.until("the grid", |ui| ui.tag("grid-cell-0-1").is_some());
    let before = rect_of(&mut ui, "grid-cell-0-1");
    wheel(&mut ui, "grid-cell-0-1", egui::vec2(-200.0, -200.0));
    assert!(!visible(&ui.tag("grid-cell-0-1")) || rect_of(&mut ui, "grid-cell-0-1") != before, "the grid scrolled");
    found.push(("the grid", fades(&ui)));

    let text: Vec<String> = (1..=200).map(|n| format!("select {n};")).collect();
    ui.execute("tab.newQuery", json!({ "text": text.join("\n") }));
    ui.frame();
    let first = |ui: &Ui| ui.texts().into_iter().find(|(_, g)| g.text().starts_with("select 1;")).map(|(r, _)| r.top());
    let before = first(&ui);
    wheel(&mut ui, "query-editor", egui::vec2(0.0, -200.0));
    let after = first(&ui);
    assert!(matches!((before, after), (Some(b), Some(a)) if a < b), "the editor scrolled: {before:?} to {after:?}");
    found.push(("the query editor", fades(&ui)));

    ui.ctrl("P");
    ui.frame();
    assert!(ui.tag("qs-input").is_some(), "the quick search is open");
    found.push(("the quick search", fades(&ui)));

    found.retain(|(_, f)| !f.is_empty());
    assert_eq!(found, [], "where fades were painted");
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

/// Drops pg14's type `scratch."<name>"` when the test ends.
struct DropType(String);

impl Drop for DropType {
    fn drop(&mut self) {
        if let Some(db) = dbee_testkit::testdb() {
            let _ = dbee_testkit::psql(pg14_server(db), &format!("DROP TYPE IF EXISTS scratch.\"{}\" CASCADE", self.0));
        }
    }
}

#[test]
fn a_column_type_is_drawn_as_the_database_writes_it() {
    let db = dbee_testkit::require_testdb!();
    let ty = DropType(format!("{}_Mood", dbee_testkit::unique("case")));
    let table = Scratch::new(db, "typed", |t| {
        format!(
            "CREATE TYPE scratch.\"{0}\" AS ENUM ('Happy', 'sad'); CREATE TABLE scratch.{t} (m scratch.\"{0}\"); INSERT INTO scratch.{t} VALUES ('Happy')",
            ty.0
        )
    });
    let label = format!("scratch.\"{}\"", ty.0);
    let mut ui = Ui::connected(db);
    ui.open("scratch", &table.0);
    ui.until("the grid", |ui| ui.tag("grid-cell-0-0").is_some());
    assert_eq!(ui.value("grid-header-0")["type"], json!(label), "the label the database gives");
    assert!(drawn(&ui, &label), "the grid's header draws {label}");
    ui.click("grid-cell-0-0");
    ui.frame();
    let panel = rect_of(&mut ui, "value-panel");
    assert!(ui.text_in(panel, &label).is_some(), "the value panel draws {label}");
    ui.click("tree-schema-scratch");
    ui.click(&format!("tree-expand-scratch.{}", table.0));
    let row = rect_of(&mut ui, &format!("tree-column-scratch.{}.m", table.0));
    assert!(ui.text_in(row, &label).is_some(), "the tree draws {label}");
    assert!(!drawn(&ui, &label.to_uppercase()), "nowhere in capitals");
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

fn window_with(settings: Option<PathBuf>, desktop: Option<egui::Theme>, home: Option<dbee_testkit::TempHome>) -> Ui {
    let make: MakeApp = Box::new(move |session, rx| DbeeApp::new(session, Some(rx)).with_settings_dir(settings).with_desktop_mode(desktop));
    Ui::window(Setup { size: egui::vec2(1100.0, 720.0), app: Some(make), home, ..Setup::default() })
}

fn state(ui: &mut Ui) -> serde_json::Value {
    ui.inspect()["theme"].clone()
}

#[test]
fn the_three_choices_set_the_mode() {
    let mut ui = Ui::fresh(egui::vec2(1100.0, 720.0));
    assert_eq!(state(&mut ui), json!({ "choice": "system", "mode": "light" }));
    assert_eq!(ui.execute("ui.themeDark", json!({})), json!({ "choice": "dark", "mode": "dark" }));
    ui.frame();
    assert_eq!(ui.harness.ctx.theme(), egui::Theme::Dark);
    assert_eq!(ui.harness.ctx.style_of(egui::Theme::Dark).visuals.panel_fill, dbee_ui_egui::theme::DARK.bg);
    assert_eq!(ui.execute("ui.themeLight", json!({})), json!({ "choice": "light", "mode": "light" }));
    ui.frame();
    assert_eq!(ui.harness.ctx.theme(), egui::Theme::Light);
    assert_eq!(ui.execute("ui.themeSystem", json!({})), json!({ "choice": "system", "mode": "light" }));
}

#[test]
fn system_uses_what_the_desktop_said_at_startup() {
    let mut ui = window_with(None, Some(egui::Theme::Dark), None);
    assert_eq!(state(&mut ui), json!({ "choice": "system", "mode": "dark" }));
    ui.frame();
    assert_eq!(ui.harness.ctx.theme(), egui::Theme::Dark);
    assert_eq!(ui.execute("ui.themeLight", json!({})), json!({ "choice": "light", "mode": "light" }));
    assert_eq!(ui.execute("ui.themeSystem", json!({})), json!({ "choice": "system", "mode": "dark" }));
}

#[test]
fn system_ignores_a_later_change_of_the_desktops_setting() {
    let mut ui = window_with(None, None, None);
    assert_eq!(state(&mut ui), json!({ "choice": "system", "mode": "light" }));
    ui.harness.input_mut().system_theme = Some(egui::Theme::Dark);
    for _ in 0..3 {
        ui.frame();
    }
    assert_eq!(ui.harness.ctx.system_theme(), Some(egui::Theme::Dark), "egui heard the change");
    assert_eq!(state(&mut ui), json!({ "choice": "system", "mode": "light" }));
    assert_eq!(ui.harness.ctx.theme(), egui::Theme::Light);
}

#[test]
fn the_choice_is_remembered_between_runs() {
    let dir = std::env::temp_dir().join(dbee_testkit::unique("theme-settings"));
    let mut first = window_with(Some(dir.clone()), None, None);
    first.execute("ui.themeDark", json!({}));
    drop(first);
    let mut second = window_with(Some(dir.clone()), None, None);
    assert_eq!(state(&mut second), json!({ "choice": "dark", "mode": "dark" }));
    assert!(dir.join("settings.json").exists());
    drop(second);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_run_without_a_settings_folder_writes_nothing() {
    let home = dbee_testkit::temp_home(None, None);
    let config = home.home.join("config");
    let mut ui = window_with(None, None, Some(home));
    assert_eq!(ui.execute("ui.themeDark", json!({}))["mode"], "dark");
    assert!(!config.join("settings.json").exists());
}

#[test]
fn a_choice_that_cannot_be_saved_still_applies_and_says_so() {
    let file = std::env::temp_dir().join(dbee_testkit::unique("theme-not-a-dir"));
    std::fs::write(&file, "x").unwrap();
    let mut ui = window_with(Some(file.clone()), None, None);
    assert_eq!(ui.execute("ui.themeDark", json!({}))["mode"], "dark");
    ui.frame();
    assert_eq!(ui.harness.ctx.theme(), egui::Theme::Dark);
    let settings = file.join("settings.json");
    let words = format!(
        "The theme is set for now, but it couldn't be saved in {} (File exists): check that you can write to {}.",
        settings.display(),
        file.display()
    );
    assert_eq!(ui.value("notice"), json!(words));
    let _ = std::fs::remove_file(file);
}

/// Review Focus 5: a toast and a tooltip showing recolour on the very next frame after a switch.
/// The toast is the notice that the choice couldn't be saved; the tooltip a colour swatch's.
#[test]
fn a_toast_recolours_on_a_theme_switch() {
    let file = std::env::temp_dir().join(dbee_testkit::unique("theme-not-a-dir"));
    std::fs::write(&file, "x").unwrap();
    let mut ui = window_with(Some(file.clone()), None, None);
    ui.click("conn-kind-postgres");
    ui.execute("ui.themeLight", json!({}));
    ui.until("the toast", |ui| ui.toast().is_some());
    let swatch = rect_of(&mut ui, "conn-color-red");
    ui.rest(swatch.center());
    for (mode, t) in [("light", dbee_ui_egui::theme::LIGHT), ("dark", dbee_ui_egui::theme::DARK)] {
        if mode == "dark" {
            ui.execute("ui.themeDark", json!({}));
            ui.frame();
        }
        let toast = ui.toast().unwrap_or_else(|| panic!("{mode}: the toast"));
        toast.assert_in(t.danger, t.on_danger, mode);
        let tip = ui.text_in(egui::Rect::EVERYTHING, "red").map(|(r, _)| r).unwrap_or_else(|| panic!("{mode}: the swatch's tooltip"));
        let frame = ui.shapes().into_iter().rev().find_map(|(_, s)| match s {
            egui::Shape::Rect(r) if r.blur_width == 0.0 && r.fill.a() == 255 && r.rect.contains_rect(tip) => Some(r),
            _ => None,
        });
        let frame = frame.unwrap_or_else(|| panic!("{mode}: the tooltip's frame"));
        assert_eq!((frame.fill, frame.stroke.color), (t.raised, t.raised_line), "{mode}: the tooltip's ground and edge");
        assert_eq!(colour_of(&ui, tip.expand(1.0), "red"), Some(t.text), "{mode}: the tooltip's words");
    }
    let _ = std::fs::remove_file(file);
}

/// The notice after choosing Dark with the settings kept in `dir`.
fn notice_for(dir: &std::path::Path) -> serde_json::Value {
    let mut ui = window_with(Some(dir.to_path_buf()), None, None);
    ui.execute("ui.themeDark", json!({}));
    ui.frame();
    ui.value("notice")
}

#[cfg(unix)]
#[test]
fn a_folder_that_cannot_be_written_is_named_with_what_to_do() {
    use std::os::unix::fs::PermissionsExt;
    let home = dbee_testkit::temp_home(None, None);
    let locked = home.home.join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
    // As root the folder can be written anyway: nothing to check.
    if std::fs::write(locked.join("probe"), "").is_ok() {
        return;
    }
    let dir = locked.join("dbee");
    let words = format!(
        "The theme is set for now, but it couldn't be saved in {} (Permission denied): check that you can write to {}.",
        dir.join("settings.json").display(),
        dir.display()
    );
    assert_eq!(notice_for(&dir), json!(words));
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn a_newer_dbees_file_is_named_and_left_as_it_is() {
    let home = dbee_testkit::temp_home(None, None);
    let text = r#"{"version":2,"theme":"light"}"#;
    std::fs::write(home.home.join("settings.json"), text).unwrap();
    let words = format!("The theme is set for now, but a newer dbee wrote {}, so it isn't changed.", home.home.join("settings.json").display());
    assert_eq!(notice_for(&home.home), json!(words));
    assert_eq!(std::fs::read_to_string(home.home.join("settings.json")).unwrap(), text);
}

#[test]
fn the_menu_marks_the_chosen_entry() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    ui.execute("ui.themeDark", json!({}));
    ui.frame();
    ui.click("menu-View");
    ui.frame();
    ui.click("menu-View-Theme");
    ui.frame();
    ui.frame();
    assert_eq!(ui.tag("menu-ui.themeDark").expect("Dark is listed")["value"]["checked"], true);
    assert!(ui.tag("menu-ui.themeLight").expect("Light is listed")["value"].get("checked").is_none());
    ui.click("menu-ui.themeLight");
    ui.frame();
    assert_eq!(state(&mut ui)["choice"], "light");
}

/// View > Theme draws its choices with the chosen mark in a left column (the design's menu with
/// choices), not a `✔` on the right; the entries' `checked` value is as before.
#[test]
fn the_theme_menu_marks_the_choice_on_the_left() {
    let db = dbee_testkit::require_testdb!();
    let d = dbee_ui_egui::theme::DARK;
    let mut ui = Ui::connected(db);
    ui.execute("ui.themeDark", json!({}));
    ui.frame();
    ui.click("menu-View");
    ui.frame();
    ui.click("menu-View-Theme");
    ui.frame();
    ui.frame();
    for (id, words, chosen) in [("menu-ui.themeSystem", "System", false), ("menu-ui.themeLight", "Light", false), ("menu-ui.themeDark", "Dark", true)]
    {
        let at = rect_of(&mut ui, id);
        let (drawn, _) = ui.text_in(at, words).unwrap_or_else(|| panic!("{words} drawn in {at:?}"));
        assert!((drawn.left() - (at.left() + 30.0)).abs() < 0.5, "{words} after the 14 px column: {drawn:?} in {at:?}");
        let marks = icon_colours(&ui, mark_column(at));
        assert_eq!(marks, if chosen { vec![d.text] } else { vec![] }, "{words}: the chosen mark in the left column");
        let value = ui.value(id);
        assert_eq!(value.get("checked"), chosen.then_some(&json!(true)), "{words}: `checked` as before: {value}");
    }
    assert!(!ui.texts().iter().any(|(_, g)| g.text().contains('✔')), "no ✔ drawn");
    ui.click("menu-ui.themeLight");
    ui.frame();
    assert_eq!(state(&mut ui)["choice"], "light");
}

/// Review Focus 5: an open menu, its hovered entry and its title's box recolour on the very
/// next frame after a theme switch.
#[test]
fn an_open_menu_recolours_on_a_theme_switch() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    ui.click("menu-View");
    ui.frame();
    let entry = rect_of(&mut ui, "menu-ui.toggleValuePanel");
    ui.hover(entry.center());
    let label = ui.value("menu-ui.toggleValuePanel")["label"].as_str().unwrap().to_string();
    for (mode, t) in [("light", dbee_ui_egui::theme::LIGHT), ("dark", dbee_ui_egui::theme::DARK)] {
        if mode == "dark" {
            ui.execute("ui.themeDark", json!({}));
            ui.frame();
        }
        let entry = rect_of(&mut ui, "menu-ui.toggleValuePanel");
        let title = rect_of(&mut ui, "menu-View");
        let rects: Vec<egui::epaint::RectShape> = ui
            .shapes()
            .into_iter()
            .filter_map(|(_, s)| match s {
                egui::Shape::Rect(r) => Some(r),
                _ => None,
            })
            .collect();
        let frame = rects.iter().rev().find(|r| r.blur_width == 0.0 && r.rect != entry && r.rect.contains_rect(entry) && r.fill.a() == 255);
        let frame = frame.unwrap_or_else(|| panic!("{mode}: the menu's frame"));
        assert_eq!((frame.fill, frame.stroke.color), (t.raised, t.line), "{mode}: the menu's ground and edge");
        assert!(rects.iter().any(|r| r.rect == entry && r.fill == t.raised_hover), "{mode}: the hovered entry");
        assert_eq!(colour_of(&ui, entry, &label), Some(t.text), "{mode}: the entry's words");
        assert!(rects.iter().any(|r| r.rect == title && r.fill == t.fill), "{mode}: the open title's box");
    }
}

#[test]
fn the_frame_after_a_switch_is_wholly_in_the_new_mode() {
    let (l, d) = (dbee_ui_egui::theme::LIGHT, dbee_ui_egui::theme::DARK);
    let mut ui = Ui::fresh(APP_SIZE);
    ui.frame();
    assert!(text_colours(&ui.shapes()).contains(&l.text), "light draws its text");
    ui.execute("ui.themeDark", json!({}));
    ui.frame();
    assert_eq!(ui.harness.ctx.theme(), egui::Theme::Dark);
    assert!(text_colours(&ui.shapes()).contains(&d.text), "dark draws its text");
    assert!(!text_colours(&ui.shapes()).contains(&l.text), "no text in light's colour");
    assert_dark(&ui, "the first frame after the switch to dark");
    ui.execute("ui.themeLight", json!({}));
    ui.frame();
    assert!(!text_colours(&ui.shapes()).contains(&d.text), "no text in dark's colour");
    for (_, shape) in ui.shapes() {
        if let egui::Shape::Rect(r) = shape {
            assert!(r.fill != d.fill, "the first frame after the switch to light: {:?} is a dark ground", r.fill);
        }
    }
}

#[test]
fn switching_recolours_what_is_already_open() {
    let db = dbee_testkit::require_testdb!();
    let (l, d) = (dbee_ui_egui::theme::LIGHT, dbee_ui_egui::theme::DARK);
    let colours = |ui: &Ui| -> Vec<egui::Color32> {
        ui.texts().iter().flat_map(|(_, g)| g.job.sections.iter().map(|s| s.format.color).collect::<Vec<_>>()).collect()
    };
    let none_of =
        |shown: &[egui::Color32], t: &dbee_ui_egui::theme::Tokens| ![t.syn_keyword, t.syn_string, t.syn_number].iter().any(|c| shown.contains(c));
    let mut ui = Ui::connected(db);
    // The value panel's JSON, on the table tab: its keys and strings.
    ui.open("shop", "orders");
    ui.until("the grid", |ui| ui.tag("grid-cell-0-5").is_some());
    ui.click("grid-cell-0-5");
    ui.until("the JSON's colours", |ui| ui.tag("value-text").is_some_and(|t| t["value"]["highlighted"] == true));
    let before = colours(&ui);
    assert!(before.contains(&l.syn_keyword) && before.contains(&l.syn_string), "the JSON is coloured in light");
    ui.execute("ui.themeDark", json!({}));
    ui.frame();
    let after = colours(&ui);
    assert!(after.contains(&d.syn_keyword) && after.contains(&d.syn_string), "the JSON is coloured in dark");
    assert!(none_of(&after, &l), "no light syntax colour is left in the JSON");
    // The editor, and back to light.
    ui.execute("tab.newQuery", json!({ "text": "select 'x' -- note" }));
    ui.frame();
    ui.frame();
    let before = colours(&ui);
    assert!(before.contains(&d.syn_keyword) && before.contains(&d.syn_string), "the editor is coloured in dark");
    ui.execute("ui.themeLight", json!({}));
    ui.frame();
    let after = colours(&ui);
    assert!(after.contains(&l.syn_keyword) && after.contains(&l.syn_string), "the editor is coloured in light");
    assert!(none_of(&after, &d), "no dark syntax colour is left in the editor");
}
