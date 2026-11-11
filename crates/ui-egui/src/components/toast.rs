//! Toasts (`toast`): a result (`success`, on `ink`) or a failure (`error`, on `danger`) said in
//! the window's bottom right corner, with its icon, its words, an optional action and a close.
//! Toasts float without a shadow: their ground is solid. [`show`] stands them over the status
//! bar, one above the other.

use std::sync::Arc;

use egui::{
    Area, Color32, Context, CornerRadius, Galley, Id, LayerId, Order, Painter, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2, Widget,
    WidgetInfo, WidgetType, pos2, vec2,
};

use super::icon::Icon;
use super::painter;
use crate::theme::size::{
    RADIUS_CONTROL, RADIUS_FLOAT, TOAST_ACTION_H, TOAST_ACTION_PAD_X, TOAST_CLOSE, TOAST_GAP, TOAST_H, TOAST_MARGIN, TOAST_MAX_W, TOAST_PAD_X,
    TOAST_STACK_GAP,
};
use crate::theme::{Text, tokens};

/// The action's edge: the toast's words' colour at this opacity.
const ACTION_EDGE: f32 = 0.7;

/// What a toast reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A result: `ink`, its words and icons in `on_ink`.
    Success,
    /// A failure: `danger`, its words and icons in `on_danger`.
    Error,
}

/// What a click on a toast asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Action,
    Close,
}

/// One toast: its kind, its words (drawn as written) and its action's label, if it has one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Toast<'a> {
    kind: Kind,
    text: &'a str,
    action: Option<&'a str>,
}

/// A toast's words and action laid out, the width of their boxes (whole points, as egui puts a
/// floating area on whole pixels) and the size they make the toast.
struct Laid {
    words: Arc<Galley>,
    words_w: f32,
    action: Option<(Arc<Galley>, f32)>,
    size: Vec2,
}

impl<'a> Toast<'a> {
    pub fn new(kind: Kind, text: &'a str) -> Toast<'a> {
        Toast { kind, text, action: None }
    }

    pub fn success(text: &'a str) -> Toast<'a> {
        Toast::new(Kind::Success, text)
    }

    pub fn error(text: &'a str) -> Toast<'a> {
        Toast::new(Kind::Error, text)
    }

    /// An action, `label` drawn in capitals in a box after the words.
    pub fn action(mut self, label: &'a str) -> Toast<'a> {
        self.action = Some(label);
        self
    }

    fn icon(&self) -> Icon {
        match self.kind {
            Kind::Success => Icon::Success,
            Kind::Error => Icon::Error,
        }
    }

    /// Its parts in a row, its words wrapping to keep it at most [`TOAST_MAX_W`] wide: 12, the
    /// icon, 12, the words, [12, the action], 12, the close, 12. One line of words makes it
    /// [`TOAST_H`] tall; each more line adds its height.
    fn lay(&self, painter: &Painter) -> Laid {
        let action = self.action.map(|label| {
            let g = painter.layout_job(Text::ButtonCaps.job(label, Color32::PLACEHOLDER));
            let w = (g.size().x + 2.0 * TOAST_ACTION_PAD_X).ceil();
            (g, w)
        });
        let action_w = action.as_ref().map_or(0.0, |(_, w)| TOAST_GAP + w);
        let rest = 2.0 * TOAST_PAD_X + self.icon().size().x + TOAST_GAP + action_w + TOAST_GAP + Icon::Close.size().x;
        let mut job = Text::Body.job_as_written(self.text, Color32::PLACEHOLDER);
        job.wrap.max_width = (TOAST_MAX_W - rest).max(TOAST_PAD_X);
        let words = painter.layout_job(job);
        let words_w = words.size().x.ceil();
        let size = vec2(rest + words_w, TOAST_H.max(words.size().y + TOAST_H - Text::Body.line()));
        Laid { words, words_w, action, size }
    }

    /// Draws it where `ui` puts it: the toast, its action and its close.
    fn draw(&self, ui: &mut Ui) -> Drawn {
        let Laid { words, words_w, action, size } = self.lay(ui.painter());
        let (rect, toast) = ui.allocate_exact_size(size, Sense::hover());
        toast.widget_info(|| WidgetInfo::labeled(WidgetType::Label, ui.is_enabled(), self.text));
        let y = rect.center().y;
        let icon = self.icon();
        let icon_at = pos2(rect.left() + TOAST_PAD_X + icon.size().x / 2.0, y);
        let words_at = pos2(icon_at.x + icon.size().x / 2.0 + TOAST_GAP, y - words.size().y / 2.0);
        let mut x = words_at.x + words_w + TOAST_GAP;
        let action = action.map(|(label, w)| {
            let r = Rect::from_min_size(pos2(x, y - TOAST_ACTION_H / 2.0), vec2(w, TOAST_ACTION_H));
            x = r.right() + TOAST_GAP;
            let clicked = ui.interact(r, toast.id.with("action"), Sense::CLICK);
            clicked.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), self.action.unwrap_or_default()));
            (r, label, clicked)
        });
        let close_at = pos2(x + Icon::Close.size().x / 2.0, y);
        let close = ui.interact(Rect::from_center_size(close_at, Vec2::splat(TOAST_CLOSE)), toast.id.with("close"), Sense::CLICK);
        close.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), "Close"));
        if ui.is_rect_visible(rect) {
            let t = tokens(ui.ctx());
            let (ground, fg) = match self.kind {
                Kind::Success => (t.ink, t.on_ink),
                Kind::Error => (t.danger, t.on_danger),
            };
            let p = painter(ui);
            p.rect_filled(rect, CornerRadius::same(RADIUS_FLOAT), ground);
            icon.paint(&p, icon_at, fg);
            p.galley(words_at, words, fg);
            if let Some((r, label, _)) = &action {
                p.rect_stroke(*r, CornerRadius::same(RADIUS_CONTROL), Stroke::new(1.0, fg.gamma_multiply(ACTION_EDGE)), StrokeKind::Inside);
                p.galley(r.center() - label.size() / 2.0, Arc::clone(label), fg);
            }
            Icon::Close.paint(&p, close_at, fg);
        }
        Drawn { toast, action: action.map(|(_, _, clicked)| clicked), close }
    }
}

/// A toast as drawn: the toast, its action (when it has one) and its close.
struct Drawn {
    toast: Response,
    action: Option<Response>,
    close: Response,
}

impl Widget for Toast<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        self.draw(ui).toast
    }
}

/// What [`show`] drew: each toast and its close, in the order given, and what a click on one
/// asked for (which toast, and its action or its close).
pub struct Shown {
    pub toasts: Vec<Response>,
    pub closes: Vec<Response>,
    pub act: Option<(usize, Act)>,
}

/// Shows `toasts` over the window, the first one lowest: each 12 from the window's right edge,
/// the first 12 above `lift` (the height of what they stand on), each next one 8 above the last.
pub fn show(ctx: &Context, toasts: &[Toast<'_>], lift: f32) -> Shown {
    let window = ctx.content_rect();
    let mut shown = Shown { toasts: Vec::with_capacity(toasts.len()), closes: Vec::with_capacity(toasts.len()), act: None };
    let mut bottom = window.bottom() - lift - TOAST_MARGIN;
    for (i, toast) in toasts.iter().enumerate() {
        let id = Id::new("dbee-toast").with(i);
        let size = toast.lay(&ctx.layer_painter(LayerId::new(Order::Foreground, id))).size;
        let at = pos2(window.right() - TOAST_MARGIN - size.x, bottom - size.y);
        let drawn = Area::new(id).order(Order::Foreground).fixed_pos(at).movable(false).fade_in(false).show(ctx, |ui| toast.draw(ui)).inner;
        if drawn.close.clicked() {
            shown.act = Some((i, Act::Close));
        } else if drawn.action.as_ref().is_some_and(Response::clicked) {
            shown.act = Some((i, Act::Action));
        }
        shown.toasts.push(drawn.toast);
        shown.closes.push(drawn.close);
        bottom = at.y - TOAST_STACK_GAP;
    }
    shown
}

#[cfg(test)]
mod tests {
    use egui::{Color32, CornerRadius, FontFamily, Rect, pos2, vec2};
    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::components::{ROOM, both, flat, harness, icon_in, texts};
    use crate::theme::{MONO_MEDIUM, Text, Tokens};

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= 0.5
    }

    /// `gap` is `want`, give or take what the words' box adds to reach a whole point.
    fn whole(gap: f32, want: f32) -> bool {
        (want - 0.01..want + 1.0).contains(&gap)
    }

    /// An icon drawn at `got` sits at `want`, moved by up to half a pixel each way so its lines
    /// fall on the pixel grid.
    fn near(got: egui::Pos2, want: egui::Pos2) -> bool {
        (got.x - want.x).abs() <= 0.55 && (got.y - want.y).abs() <= 0.55
    }

    /// The ground and the words' colour of `kind` in `t`.
    fn colours(kind: Kind, t: &Tokens) -> (Color32, Color32) {
        match kind {
            Kind::Success => (t.ink, t.on_ink),
            Kind::Error => (t.danger, t.on_danger),
        }
    }

    /// The font of the text `words` drawn in `shapes`.
    fn font_of(shapes: &[egui::epaint::ClippedShape], words: &str) -> Option<egui::FontId> {
        flat(shapes).into_iter().find_map(|s| match s {
            egui::Shape::Text(t) if t.galley.text() == words => t.galley.job.sections.first().map(|s| s.format.font_id.clone()),
            _ => None,
        })
    }

    #[test]
    fn each_toast_matches_the_page() {
        // Node 162749: 44 tall, `ink` (success) or `danger` (error), corners 4, no edge and no
        // shadow; 12 in, the 14 px icon, 12, the words in `body`, [12, the action: 28 tall, its
        // caps 12 in from a 1 px edge of the words' colour at 70 %, corners 2], 12, the 7 px
        // close, 12. Words, icons and the action in `on_ink` or `on_danger`.
        let words = "Export failed: disk full";
        for (theme, t) in both() {
            for kind in [Kind::Success, Kind::Error] {
                for action in [None, Some("Open folder")] {
                    let at_ = format!("{theme:?} {kind:?} action {action:?}");
                    let mut h = harness(theme, Rect::NOTHING, move |ui, at| {
                        ui.add_space(ROOM);
                        ui.horizontal(|ui| {
                            ui.add_space(ROOM);
                            let toast = Toast::new(kind, words);
                            *at = ui.add(if let Some(a) = action { toast.action(a) } else { toast }).rect;
                        });
                    });
                    h.run();
                    let at = *h.state();
                    let shapes = h.output().shapes.clone();
                    let (ground, fg) = colours(kind, &t);
                    assert!(close(at.height(), 44.0), "{at_}: 44 tall: {at:?}");
                    let grounds: Vec<_> = flat(&shapes)
                        .into_iter()
                        .filter_map(|s| match s {
                            egui::Shape::Rect(r) if r.rect == at => Some(r),
                            _ => None,
                        })
                        .collect();
                    assert_eq!(grounds.len(), 1, "{at_}: one ground: {grounds:?}");
                    assert_eq!((grounds[0].fill, grounds[0].corner_radius), (ground, CornerRadius::same(4)), "{at_}: the ground");
                    assert!(grounds[0].stroke.width == 0.0 || grounds[0].stroke.color.a() == 0, "{at_}: no edge");
                    let blurred = flat(&shapes).into_iter().any(|s| matches!(s, egui::Shape::Rect(r) if r.blur_width > 0.0));
                    assert!(!blurred, "{at_}: no shadow");
                    let icon = Rect::from_min_size(pos2(at.left() + 12.0, at.top() + 15.0), vec2(14.0, 14.0));
                    let (icon_colours, bounds) = icon_in(&shapes, icon.expand(1.0));
                    assert_eq!(icon_colours, [fg], "{at_}: the icon in the words' colour");
                    assert!(near(bounds.center(), icon.center()), "{at_}: the icon {bounds:?} in {icon:?}");
                    let drawn = texts(&shapes);
                    let find = |w: &str| drawn.iter().find(|(s, _, _)| s == w).map(|(_, r, c)| (*r, *c));
                    let (text, colour) = find(words).unwrap_or_else(|| panic!("{at_}: the words drawn: {drawn:?}"));
                    assert_eq!(colour, fg, "{at_}: the words' colour");
                    assert!(close(text.left(), at.left() + 38.0), "{at_}: the words 38 in: {text:?}");
                    assert!(close(text.height(), 18.0) && close(text.center().y, at.center().y), "{at_}: body's 18, centred: {text:?}");
                    let font = font_of(&shapes, words).unwrap_or_else(|| panic!("{at_}: the words' font"));
                    assert_eq!(font, Text::Body.font(), "{at_}: the words in body");
                    let mut right = text.right();
                    if let Some(label) = action {
                        let caps = label.to_uppercase();
                        let (cap, cap_colour) = find(&caps).unwrap_or_else(|| panic!("{at_}: {caps} drawn: {drawn:?}"));
                        assert!(find(label).is_none(), "{at_}: the action only in capitals");
                        assert_eq!(cap_colour, fg, "{at_}: the action's words");
                        let font = font_of(&shapes, &caps).unwrap_or_else(|| panic!("{at_}: the action's font"));
                        assert_eq!(font.family, FontFamily::Name(MONO_MEDIUM.into()), "{at_}: button_caps");
                        let edge = flat(&shapes).into_iter().find_map(|s| match s {
                            egui::Shape::Rect(r) if r.stroke.width > 0.0 && r.rect.contains_rect(cap) && r.rect != at => Some(r),
                            _ => None,
                        });
                        let edge = edge.unwrap_or_else(|| panic!("{at_}: the action's box"));
                        assert_eq!(
                            (edge.stroke.width, edge.stroke.color, edge.corner_radius, edge.stroke_kind),
                            (1.0, fg.gamma_multiply(0.7), CornerRadius::same(2), egui::StrokeKind::Inside),
                            "{at_}: the action's edge"
                        );
                        assert!(edge.fill.a() == 0, "{at_}: the action has no ground");
                        assert!(close(edge.rect.height(), 28.0) && close(edge.rect.center().y, at.center().y), "{at_}: 28 tall, centred");
                        assert!(whole(edge.rect.left() - text.right(), 12.0), "{at_}: 12 after the words' box");
                        let sides = [cap.left() - edge.rect.left(), edge.rect.right() - cap.right()];
                        assert!(sides.iter().all(|s| (12.0..12.6).contains(s)), "{at_}: 12 either side: {sides:?}");
                        right = edge.rect.right();
                    } else {
                        assert_eq!(drawn.len(), 1, "{at_}: only the words, no action: {drawn:?}");
                        let boxed = flat(&shapes).into_iter().any(|s| matches!(s, egui::Shape::Rect(r) if r.stroke.width > 0.0));
                        assert!(!boxed, "{at_}: no action's box");
                    }
                    let glyph = Rect::from_min_size(pos2(at.right() - 12.0 - 7.0, at.center().y - 3.5), vec2(7.0, 7.0));
                    let (close_colours, bounds) = icon_in(&shapes, glyph.expand(1.5));
                    assert_eq!(close_colours, [fg], "{at_}: the close in the words' colour");
                    assert!(near(bounds.center(), glyph.center()), "{at_}: the close {bounds:?} 12 from the right end of {at:?}");
                    assert!(whole(glyph.left() - right, 12.0), "{at_}: the close 12 after what comes before it");
                    assert!(h.query_by_label("Close").is_some(), "{at_}: the close is a button named Close");
                    assert!(h.query_by_label(words).is_some(), "{at_}: the words read out as written");
                }
            }
        }
    }

    #[test]
    fn toasts_sit_above_the_status_bar() {
        // 12 from the window's right edge and 12 above what they stand on, 8 apart; a click on
        // a toast's close or action says which.
        const LIFT: f32 = 30.0;
        struct S {
            toasts: Vec<Rect>,
            closes: Vec<Rect>,
            act: Option<(usize, Act)>,
        }
        for (theme, _) in both() {
            let mut h = harness(theme, S { toasts: Vec::new(), closes: Vec::new(), act: None }, |ui, s| {
                let shown = show(ui.ctx(), &[Toast::success("Copied 2 rows").action("Undo"), Toast::error("Export failed: disk full")], LIFT);
                s.toasts = shown.toasts.iter().map(|r| r.rect).collect();
                s.closes = shown.closes.iter().map(|r| r.rect).collect();
                if shown.act.is_some() {
                    s.act = shown.act;
                }
            });
            h.run();
            let screen = h.ctx.content_rect();
            let (toasts, closes) = (h.state().toasts.clone(), h.state().closes.clone());
            assert_eq!((toasts.len(), closes.len()), (2, 2), "{theme:?}: two toasts and their closes");
            let [first, second] = [toasts[0], toasts[1]];
            assert!(
                close(first.right(), screen.right() - 12.0) && close(second.right(), screen.right() - 12.0),
                "{theme:?}: 12 from the right: {toasts:?}"
            );
            assert!(close(first.bottom(), screen.bottom() - LIFT - 12.0), "{theme:?}: 12 above the lift: {first:?} in {screen:?}");
            assert!(close(first.top() - second.bottom(), 8.0), "{theme:?}: 8 apart: {toasts:?}");
            h.get_all_by_label("Close").nth(1).unwrap_or_else(|| panic!("{theme:?}: the second close")).click();
            h.run();
            assert_eq!(h.state().act, Some((1, Act::Close)), "{theme:?}: the second's close");
            h.state_mut().act = None;
            h.get_by_label("Undo").click();
            h.run();
            assert_eq!(h.state().act, Some((0, Act::Action)), "{theme:?}: the first's action");
        }
    }
}
