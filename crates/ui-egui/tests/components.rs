//! The showcase of the design's Components page (`components::showcase`), in both modes.

mod common;

use common::{flat, text_colours};
use dbee_ui_egui::components::showcase;
use dbee_ui_egui::theme::{self, DARK, LIGHT, Tokens};
use egui::scroll_area::ScrollBarVisibility;
use egui::{Color32, Pos2, Rect, ScrollArea, Shape, pos2};
use egui_kittest::Harness;

/// Every colour of a mode. Listed field by field, so a new token can't be left out.
fn every(t: &Tokens) -> Vec<egui::Color32> {
    let Tokens {
        bg,
        card,
        ink,
        on_ink,
        text,
        text_muted,
        text_faint,
        line,
        line_strong,
        fill,
        hover,
        selection,
        text_selection,
        raised,
        raised_hover,
        raised_line,
        scrim,
        shadow,
        danger,
        on_danger,
        success,
        staged,
        staged_strong,
        new_row,
        deleted,
        saved,
        failed,
        key,
        brand,
        syn_keyword,
        syn_string,
        syn_number,
        syn_literal,
        match_other,
        match_current,
        conn_red,
        conn_orange,
        conn_yellow,
        conn_green,
        conn_blue,
        conn_purple,
        conn_grey,
        on_colour,
        on_colour_dark,
    } = *t;
    vec![
        bg,
        card,
        ink,
        on_ink,
        text,
        text_muted,
        text_faint,
        line,
        line_strong,
        fill,
        hover,
        selection,
        text_selection,
        raised,
        raised_hover,
        raised_line,
        scrim,
        shadow,
        danger,
        on_danger,
        success,
        staged,
        staged_strong,
        new_row,
        deleted,
        saved,
        failed,
        key,
        brand,
        syn_keyword,
        syn_string,
        syn_number,
        syn_literal,
        match_other,
        match_current,
        conn_red,
        conn_orange,
        conn_yellow,
        conn_green,
        conn_blue,
        conn_purple,
        conn_grey,
        on_colour,
        on_colour_dark,
    ]
}

#[test]
fn the_showcase_draws_in_both_modes() {
    for (theme, t) in [(egui::Theme::Light, LIGHT), (egui::Theme::Dark, DARK)] {
        let mut ready = false;
        let h = Harness::builder().with_size(egui::vec2(1240.0, 13400.0)).build_ui(|ui| {
            if !ready {
                showcase::prepare(ui.ctx(), theme);
                ready = true;
                ui.ctx().request_repaint();
                return;
            }
            showcase::show(ui);
        });
        assert_eq!(h.ctx.theme(), theme);
        let shapes = flat(&h.output().shapes);
        let words: Vec<String> = shapes
            .iter()
            .filter_map(|(_, s)| match s {
                egui::Shape::Text(t) => Some(t.galley.text().to_owned()),
                _ => None,
            })
            .collect();
        let captions = [
            "ICON/, TODAY'S DRAWINGS IN THEIR USUAL COLOUR",
            "LOGO/: THE ENGINES' LOGOS FROM SIMPLE ICONS 13.21.0 IN THEIR USUAL COLOUR, AT 24 AND AT THE CONNECTION ROW'S 14",
            "BUTTON/BOX PRIMARY: DEFAULT, FOCUS, DISABLED",
            "BUTTON/BOX OUTLINE: DEFAULT, HOVER, FOCUS, DISABLED",
            "BUTTON/BOX DANGER: DEFAULT, FOCUS, DISABLED",
            "BUTTON/BAR: DEFAULT, WITH DIVIDER, HOVER, ON, DISABLED, FOCUS",
            "BUTTON/BAR ICON: DEFAULT, HOVER, ON, DISABLED, FOCUS",
            "BUTTON/TEXT: DEFAULT, HOVER, FOCUS, DISABLED",
            "BUTTON/ICON BOXED: PLUS, MINUS, CODE, HOVER, FOCUS, DISABLED, ON (CODE: RAW WHERE; NO CHANGE ON HOVER)",
            "BUTTON/ICON PLAIN: FIRST, PREVIOUS, NEXT, LAST, HOVER, FOCUS, DISABLED",
            "INPUT BODY: EMPTY, PLACEHOLDER, FILLED, FOCUS, DISABLED, ERROR",
            "INPUT DATA: EMPTY, PLACEHOLDER, FILLED, FOCUS, DISABLED, ERROR",
            "FIELD: LABEL PLUS INPUT; WITH AN ERROR LINE",
            "DROPDOWN: DEFAULT, FOCUS, DISABLED",
            "CHECKBOX: ON, OFF, ON DISABLED, OFF DISABLED, ON FOCUS, OFF FOCUS",
            "SEGMENTED CAPS, 2 SEGMENTS",
            "SEGMENTED SMALL, 3 AND 5 SEGMENTS",
            "SEGMENTED/SEGMENT: SELECTED, DEFAULT, HOVER, DISABLED, SELECTED DISABLED, SELECTED FOCUS, FOCUS",
            "SEGMENTED/SEGMENT SMALL: SELECTED, DEFAULT, HOVER, DISABLED, SELECTED DISABLED, SELECTED FOCUS, FOCUS",
            "SWATCH: NOT SELECTED; THE LAST ONE HAS FOCUS",
            "SWATCH: SELECTED; THE LAST ONE HAS FOCUS",
            "KEYCAP: DEFAULT, DISABLED",
            "CHIP: NEXT, AVAILABLE",
            "MENU: EDIT AS ON SCREENS; MENU/ITEM, MENU/SEPARATOR, KEYCAPS",
            "MENUBAR/TITLE OPEN, VIEW MENU, THEME SUBMENU: THE CHOSEN MARK SITS IN A LEFT COLUMN",
            "MENU/ITEM: DEFAULT, HOVER (THE KEYBOARD HIGHLIGHT LOOKS THE SAME), DISABLED, DESTRUCTIVE, DESTRUCTIVE HOVER; MENUBAR/TITLE: DEFAULT, OPEN",
            "TOOLTIP: ONE LINE, WRAPPED AT 320; 4 PX BELOW THE POINTER, ITS LEFT EDGE AT THE POINTER",
            "TOAST: SUCCESS, ERROR, ERROR WITHOUT AN ACTION; ICON/SUCCESS, ICON/ERROR",
            "DIALOG: SCREENS' DIALOGS FROM THE COMPONENTS (DATABASE PASSWORD, FILL FROM URL WITH AN ERROR, CLOSE ANYWAY, DELETE CONNECTION)",
            "NEW: APPLY AGAIN (AN APPLY WHOSE OUTCOME IS UNKNOWN); THE COMMIT PROMPT ON CLOSING A TAB OR SWITCHING CONNECTIONS, COMMIT IS THE MAIN BUTTON",
            "SCRIM: OVER THE WHOLE WINDOW; THE DIALOG CENTRED, NEAR THE TOP",
            "TAB: TABLE AND QUERY; INACTIVE, HOVER (SHOWS CLOSE), ACTIVE (UNSAVED DOT, CLOSE), FOCUS (INFERRED, LIKE A BAR BUTTON)",
            "TAB STRIP: TABS, THEN THE NEW TAB BUTTON (DEFAULT, HOVER, FOCUS), ON CARD WITH THE LINE UNDER THE INACTIVE TABS",
            "TAB/LIST: DEFAULT, HOVER, FOCUS, OPEN (FILL, AS AN OPEN MENU TITLE)",
            "TABS THAT DON'T FIT: THE STRIP SCROLLS, RESTING AT A TAB'S EDGE; + AND THE LIST STAY AT THE END; THE LIST SHOWS EVERY TAB, THE ACTIVE ONE MARKED",
            "NOT ON THE PAGE YET: A BUSY TAB, THE SPINNER IN ITS ICON'S PLACE; INACTIVE, ACTIVE",
            "MENU BAR, 30 TALL, FROM MENUBAR/TITLE; THEN WITH VIEW OPEN",
            "RUN ROW: IDLE; RUNNING (RUN OFF, CANCEL THE INK BUTTON); RUN CURRENT AS AN OPTION, HIDDEN BY DEFAULT: NOT DECIDED YET",
            "RESULT TAB: INACTIVE, HOVER, ACTIVE, FOCUS (INFERRED)",
            "RESULT BAR: DONE, RUNNING, FAILED, CANCELLED; NEW FACTS: 2 ROWS AFFECTED, FIRST 10,000 OF 25,000 ROWS; THE VALUE PANEL OPEN (ON); MANY RESULTS: THEY SCROLL, WITH THE LIST",
            "ROWS/COUNT: EXACT; ESTIMATE WITH COUNT; UNKNOWN WITH COUNT ROWS; COUNTING WITH ITS TIMER (ITS CANCEL IS THE FOOTER JOB)",
            "NOT ON THE PAGE YET: A RUN AS IT FIRST SHOWS, AT 00:00.3; A COUNT'S FIRST 300 MS, ITS WORDS ALONE; A RESULT'S NAME KEEPS ITS CASE",
            "TABLE BAR: DATA, STRUCTURE, LOADING; THE ROW COUNT: ESTIMATE WITH COUNT, UNKNOWN WITH COUNT ROWS, COUNTING (ITS CANCEL IS THE FOOTER JOB); AS THE TABLE TAB DRAWS IT, STRUCTURE OFF AND NO VALUE CELL",
            "FILTER BAR: FILTER ROW FIRST (RAW SQL TOGGLE, COLUMN, OPERATOR, VALUE, REMOVE, ADD, APPLY), THEN AND ROWS; ON CARD, 4 BY 12 INSET, LINE UNDER",
            "PAGING BAR: WITH STAGED CHANGES (1 CHANGE, RESET, APPLY); WITHOUT",
            "SIDEBAR HEADER: THE FILTER (PLACEHOLDER, FILLED, FOCUS) WITH REFRESH; THE COUNT UNDER IT (516 TABLES, 3 OF 516 TABLES, 0 TABLES, LOADING)",
            "NOT ON THE PAGE YET: THE REFRESH WHILE A REFRESH RUNS (ITS PLAIN ICON, A CLICK DOES NOTHING) AND OFF; THE CONNECTION LIST'S FILTER, NO REFRESH; TYPED TEXT KEEPS ITS CASE",
            "STATUS BAR: THE CONNECTION COLOUR AS A 2 PX TOP EDGE AND THE NAME BLOCK; NO COLOUR KEEPS A QUIET FOOTER",
            "NEW: ADDRESS AS TYPED, SERVER VERSION, LINK SECURITY (ONLY WHEN WEAKER THAN FULLY CHECKED), READ-ONLY, SAVE CONNECTION; RIGHT: ONE ACTIVITY AT A TIME",
            "NOT ON THE PAGE YET: A COUNT'S JOB WITH ITS TIMER, A TABLE'S NAME KEEPS ITS CASE; RECONNECTING IN ITS FIRST 300 MS, ITS WORDS ALONE; A NAME DBEE MADE, AS WRITTEN, WITH NO ADDRESS; A LONG PATH CUT IN THE MIDDLE",
            "GRID: HEADER (SORTED, TEXT, NUMBER, FOREIGN KEY HOVERED); ROWS: PLAIN, HOVER, SELECTED WITH THE FOCUS ON A KEY (ITS LINK), STAGED AND STAGED FOCUS, SAVED, FAILED, NEW, DELETED, NULL / LINE BREAK / EMPTY, TWO PLAIN ROWS WHERE THE PAGE SHOWS THE CELL EDITOR, #17 THE WHOLE ROW SELECTED",
            "ICON/FOLLOW, ICON/LINE-BREAK: THE GRID'S FOLLOW LINK AND A LINE BREAK IN A CELL",
            "TREE/ROW: SCHEMA OPEN, TABLE, TABLE OPEN WITH ITS COLUMNS (PRIMARY KEY, PLAIN, FOREIGN KEY), VIEW, MATERIALIZED VIEW, HOVER, SELECTED, FOCUS (INFERRED), SCHEMA",
            "SIDEBAR/CONNECTION: DEFAULT, SELECTED, HOVER, FOCUS (INFERRED), NO COLOUR, OTHER ENGINES (LOGO/MYSQL, LOGO/SQLITE)",
            "QUICK SEARCH: ORD; THE OPEN CONNECTION'S HEADING AND TABLES (HIGHLIGHTED, VIEW, MATERIALIZED VIEW), OTHER CONNECTIONS (HOVER, NO COLOUR)",
            "QUICK SEARCH: NOTHING TYPED, THE PLACEHOLDER; NO MATCH; THE SEARCH'S ERROR (NOT DRAWN); CUT ROWS",
            "VALUE PANEL: JSON, TEXT, NULL, EMPTY (INFERRED), NO CELL SELECTED (B4), COPIED, EDITING SEVERAL LINES (MC+), #17 A ROW AS JSON (INFERRED)",
            "VALUE PANEL: FORMATTED AS JSON (K2), A CUT VALUE (N2), 240 WIDE WITH A LONG NAME AND TYPE",
            "ERROR BAND (EC1): POSTGRES, HINT AND POSITION; POSTGRES, DETAIL AND NO POSITION; MYSQL, THE MESSAGE ONLY; SQLITE, A POSITION; STATEMENT 2 FAILED",
            "NOT IN THE ERROR BAND FRAME: ROLLBACK UNDER A FAILED TRANSACTION (R2); THE TABLE TAB'S HEADINGS AND NOTES; A SERVER'S DETAIL AND HINT BOTH, THE DETAIL FIRST",
            "STRIP: A RESULT (A TEST PASSED, A SAVE), A NOTE; NOT DRAWN, THE CONNECTION FORM'S",
            "EDITOR: THE ERROR'S LINE NUMBER IN DANGER, THE TOKEN UNDERLINED IN DANGER; THE CURRENT LINE'S NUMBER IN TEXT",
            "STRUCTURE VIEW (SA): A SECTION HEADING, THE HEADER (INK LINE UNDER IT), ROWS WITH POSITION, NAME, TYPE, NULLABLE CHECK, DEFAULT, KEYS (PRIMARY KEY, FOREIGN KEY LINK)",
            "LOADING (LC1, LC2): A TABLE'S FIRST LOAD, THE REAL HEADER OVER TEN ROWS OF FILL BARS (NONE IN THE ROW NUMBERS, A NUMBER'S BAR AT THE RIGHT); A RELOAD, THE OLD ROWS AT 50 % UNDER THE HEADER AT FULL STRENGTH",
            "LOADING (LC4): THE TREE'S COUNT READS LOADING OVER ITS ROWS AT 50 %; WITH NO ROWS TO KEEP, EIGHT ROWS OF BARS",
            "A DIMMED AREA (THE CONNECTING FORM AT 50 %): CLEAR ON AND OFF, THEN THE SAME DIMMED; THE ONE ALREADY OFF KEEPS ITS OWN LOOK, NEVER FAINTER",
            "THE ONE GREY PHRASE OF AN EMPTY AREA: AT THE TOP (NO ROWS ARE LOADED.); CENTRED (NO CELL SELECTED)",
        ];
        for caption in captions {
            assert!(words.iter().any(|w| w == caption), "{theme:?}: the section `{caption}`: {words:?}");
        }
        // A dimmed area's texts (the waiting states) are a token at the dim's strength.
        let tokens = every(&t);
        let dimmed: Vec<Color32> = tokens.iter().map(|c| c.gamma_multiply(theme::DIM)).collect();
        for c in text_colours(&shapes) {
            assert!(tokens.contains(&c) || dimmed.contains(&c), "{theme:?}: a text in {c:?}, which is no token of this mode (nor one dimmed)");
        }
    }
}

/// The showcase as `examples/components.rs` draws it: on the ground, in a scroll area that
/// scrolls to `offset` with its scroll bar hidden; `view` is where the scroll area shows it.
struct Window {
    offset: f32,
    view: Rect,
}

/// The harness's room around what it holds, either side.
const HARNESS_MARGIN: f32 = 8.0;

/// [`Window`] in a harness of `size`, the example's 1248 wide.
fn window(theme: egui::Theme, height: f32) -> Harness<'static, Window> {
    let mut ready = false;
    Harness::builder().with_size(egui::vec2(1248.0 + 2.0 * HARNESS_MARGIN, height + 2.0 * HARNESS_MARGIN)).build_ui_state(
        move |ui, w: &mut Window| {
            if !ready {
                showcase::prepare(ui.ctx(), theme);
                ready = true;
                ui.ctx().request_repaint();
                return;
            }
            let ground = theme::tokens(ui.ctx()).bg;
            let area =
                ScrollArea::vertical().auto_shrink(false).vertical_scroll_offset(w.offset).scroll_bar_visibility(ScrollBarVisibility::AlwaysHidden);
            let out = egui::CentralPanel::default().frame(egui::Frame::new().fill(ground)).show(ui, |ui| area.show(ui, showcase::show)).inner;
            w.view = out.inner_rect;
        },
        Window { offset: 0.0, view: Rect::NOTHING },
    )
}

/// `top` (premultiplied, as egui keeps it) laid over `ground`.
fn over(top: Color32, ground: Color32) -> Color32 {
    let a = f32::from(top.a()) / 255.0;
    let mix = |t: u8, g: u8| (f32::from(t) + f32::from(g) * (1.0 - a)).round().min(255.0) as u8;
    Color32::from_rgba_premultiplied(mix(top.r(), ground.r()), mix(top.g(), ground.g()), mix(top.b(), ground.b()), mix(top.a(), ground.a()))
}

/// The colour `mesh` paints at `at`, if it covers it: the corners of its triangle there, mixed
/// by how near `at` is to each.
fn mesh_at(mesh: &egui::Mesh, at: Pos2) -> Option<Color32> {
    for tri in mesh.indices.as_chunks::<3>().0 {
        let [a, b, c] = tri.map(|i| &mesh.vertices[i as usize]);
        let d = (b.pos.y - c.pos.y) * (a.pos.x - c.pos.x) + (c.pos.x - b.pos.x) * (a.pos.y - c.pos.y);
        if d == 0.0 {
            continue;
        }
        let wa = ((b.pos.y - c.pos.y) * (at.x - c.pos.x) + (c.pos.x - b.pos.x) * (at.y - c.pos.y)) / d;
        let wb = ((c.pos.y - a.pos.y) * (at.x - c.pos.x) + (a.pos.x - c.pos.x) * (at.y - c.pos.y)) / d;
        let wc = 1.0 - wa - wb;
        if wa < -1e-4 || wb < -1e-4 || wc < -1e-4 {
            continue;
        }
        let mix = |f: fn(&Color32) -> u8| (wa * f32::from(f(&a.color)) + wb * f32::from(f(&b.color)) + wc * f32::from(f(&c.color))).round() as u8;
        return Some(Color32::from_rgba_premultiplied(mix(Color32::r), mix(Color32::g), mix(Color32::b), mix(Color32::a)));
    }
    None
}

/// The colour seen at `at` on the edge `shapes[edge]` draws: its stroke, with every fill and mesh
/// painted after it over that point laid on top.
fn seen(shapes: &[(Rect, Shape)], edge: usize, at: Pos2) -> Color32 {
    let Some((_, Shape::Rect(r))) = shapes.get(edge) else { panic!("shape {edge} is no rect") };
    let mut colour = r.stroke.color;
    for (clip, s) in &shapes[edge + 1..] {
        let top = match s {
            _ if !clip.contains(at) => continue,
            Shape::Rect(r) if r.rect.contains(at) && r.fill.a() > 0 => r.fill,
            Shape::Mesh(m) => match mesh_at(m, at) {
                Some(c) => c,
                None => continue,
            },
            _ => continue,
        };
        colour = over(top, colour);
    }
    colour
}

/// The inputs' edges the last frame drew: each edge's index in `shapes`, its rect and its stroke.
fn inputs(shapes: &[(Rect, Shape)]) -> Vec<(usize, Rect, egui::Stroke)> {
    shapes
        .iter()
        .enumerate()
        .filter_map(|(i, (_, s))| match s {
            Shape::Rect(r) if r.stroke.width > 0.0 && r.rect.size() == egui::vec2(160.0, 28.0) => Some((i, r.rect, r.stroke)),
            _ => None,
        })
        .collect()
}

/// Where the INPUT BODY row lies in the showcase, from a frame that shows it all: under its
/// caption, `view` being where the showcase starts.
fn body_row(shapes: &[(Rect, Shape)], view: Rect) -> Rect {
    const CAPTION: &str = "INPUT BODY: EMPTY, PLACEHOLDER, FILLED, FOCUS, DISABLED, ERROR";
    let caption = shapes
        .iter()
        .find_map(|(_, s)| match s {
            Shape::Text(t) if t.galley.text() == CAPTION => Some(t.galley.rect.translate(t.pos.to_vec2())),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the caption `{CAPTION}` is drawn"));
    let row: Vec<Rect> =
        inputs(shapes).into_iter().map(|(_, r, _)| r).filter(|r| r.top() > caption.bottom() && r.top() < caption.bottom() + 10.0).collect();
    assert_eq!(row.len(), 6, "the row's six inputs: {row:?}");
    row.into_iter().fold(Rect::NOTHING, Rect::union).translate(-view.min.to_vec2())
}

#[test]
fn the_input_body_rows_edges_are_even_at_either_end_of_the_window() {
    // Pictures of the components (`target/components-group1`, 1x) showed this row's edges and
    // words faded at the top, where the picture's second piece starts: each input's edge must look
    // the same at its top and at its bottom, with the row at the top or the bottom of the view.
    for (mode, t) in [(egui::Theme::Light, LIGHT), (egui::Theme::Dark, DARK)] {
        let h = window(mode, 3200.0);
        let row = body_row(&flat(&h.output().shapes), h.state().view);
        // The example's window: 900 tall.
        let mut h = window(mode, 900.0);
        let height = h.state().view.height();
        assert_eq!(height, 900.0, "{mode:?}: the example's view");
        for (at, offset) in [("at the top of the view", row.top()), ("at the bottom of the view", row.bottom() - height)] {
            h.state_mut().offset = offset;
            h.run();
            let shapes = flat(&h.output().shapes);
            let top = h.state().view.top() + row.top() - offset;
            let inputs: Vec<_> = inputs(&shapes).into_iter().filter(|(_, r, _)| (r.top() - top).abs() < 0.5).collect();
            assert_eq!(inputs.len(), 6, "{mode:?} {at}: the row's six inputs at {top}");
            for (n, (i, r, stroke)) in inputs.into_iter().enumerate() {
                let own = stroke.color;
                assert!([t.line_strong, t.ink, t.danger].contains(&own), "{mode:?} {at}: input {n}'s edge {own:?}");
                let top = seen(&shapes, i, pos2(r.center().x, r.top() + stroke.width / 2.0));
                let bottom = seen(&shapes, i, pos2(r.center().x, r.bottom() - stroke.width / 2.0));
                assert_eq!((top, bottom), (own, own), "{mode:?} {at}: input {n}'s edge {r:?} as seen at its top and its bottom");
            }
        }
    }
}
