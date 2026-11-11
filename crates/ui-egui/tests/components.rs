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
        let h = Harness::builder().with_size(egui::vec2(1240.0, 3200.0)).build_ui(|ui| {
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
            "BUTTON/BOX PRIMARY: DEFAULT, FOCUS, DISABLED",
            "BUTTON/BOX OUTLINE: DEFAULT, HOVER, FOCUS, DISABLED",
            "BUTTON/BOX DANGER: DEFAULT, FOCUS, DISABLED",
            "BUTTON/BAR: DEFAULT, WITH DIVIDER, HOVER, ON, DISABLED, FOCUS",
            "BUTTON/BAR ICON: DEFAULT, HOVER, ON, DISABLED, FOCUS",
            "BUTTON/TEXT: DEFAULT, HOVER, FOCUS, DISABLED",
            "BUTTON/ICON BOXED: PLUS, MINUS, CODE, HOVER, FOCUS, DISABLED",
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
        ];
        for caption in captions {
            assert!(words.iter().any(|w| w == caption), "{theme:?}: the section `{caption}`: {words:?}");
        }
        let tokens = every(&t);
        for c in text_colours(&shapes) {
            assert!(tokens.contains(&c), "{theme:?}: a text in {c:?}, which is no token of this mode");
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
    // The design agent's pictures (`target/components-group1`, 1x) showed this row's edges and
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
