//! The workspace's tabs (`tab`), the new tab button (`tab/new`) and the strip they stand in
//! (`tab strip`): a band on `card` that scrolls sideways when they don't fit, with a `line` along
//! its bottom under everything but the active tab, which opens into what is below it.
//!
//! A tab holds, from its left, its kind's icon (the spinner while it is busy), its title as
//! written, the unsaved dot and the close, and a `line` at its right end. Its close shows while it
//! is the active tab or under the pointer. The close's room is kept on every tab, so no tab grows
//! under the pointer and every tab's close can be clicked by its tag.
//!
//! A strip that scrolls comes to rest with a tab whole at its left edge (the design agent's rule
//! until the overflow of tabs is decided): after the wheel or the scroll bar, it goes on to the
//! next tab's edge the way it moved; after a scroll it didn't do itself, to the nearest one. The
//! new tab button can always be scrolled to: the strip leaves room after it for that.

use std::fmt::Debug;
use std::hash::Hash;
use std::sync::Arc;

use egui::text::TextWrapping;
use egui::{
    Color32, CornerRadius, Galley, Id, Rect, Response, ScrollArea, Sense, Ui, UiBuilder, UiStackInfo, Vec2, Widget, WidgetInfo, WidgetType, pos2,
    vec2,
};

use super::icon::Icon;
use super::spinner::Spinner;
use super::{Look, focus, painter};
use crate::theme::size::{NEW_TAB_W, RADIUS_CONTROL, TAB_ACTIVE_LINE, TAB_CLOSE, TAB_DOT, TAB_GAP, TAB_H, TAB_PAD_X, TAB_TITLE_MAX};
use crate::theme::{Text, tokens};

/// The strip's lines: at a tab's right end and along the bottom.
const LINE_W: f32 = 1.0;

/// The tag a strip puts on its `Ui`: the key under which the tabs in it note their cells.
const STRIP_TAG: &str = "dbee-tab-strip";

/// What the tabs of a strip noted this frame: where each one is, and the one to bring into view.
#[derive(Clone, Debug, Default)]
struct Cells {
    tabs: Vec<(f32, f32)>,
    reveal: Option<(f32, f32)>,
}

/// Where a strip last came to rest, and whether the user has scrolled it since.
#[derive(Clone, Copy, Debug, Default)]
struct Rest {
    offset: f32,
    by_user: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Table,
    Query,
}

/// What a click on a tab asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Asked {
    /// Make it the active tab: a click on a tab that isn't, or Enter or Space on it.
    Activate,
    /// Close it: a click on its close, or a middle click on the tab.
    Close,
}

/// One tab of the strip, [`TAB_H`] tall: [`Tab::show`] draws it.
pub struct Tab<'a> {
    kind: Kind,
    title: &'a str,
    active: bool,
    unsaved: bool,
    busy: bool,
    reveal: bool,
    id: Option<Id>,
    look: Option<Look>,
}

/// A tab as drawn: the tab, its close, its unsaved dot when it has one, and what a click on it
/// asked for.
pub struct Shown {
    pub tab: Response,
    pub close: Response,
    pub unsaved: Option<Response>,
    pub asked: Option<Asked>,
}

impl<'a> Tab<'a> {
    /// A table tab: its title is the table's name, with the number of applied filters.
    pub fn table(title: &'a str) -> Tab<'a> {
        Tab::new(Kind::Table, title)
    }

    /// A query tab.
    pub fn query(title: &'a str) -> Tab<'a> {
        Tab::new(Kind::Query, title)
    }

    fn new(kind: Kind, title: &'a str) -> Tab<'a> {
        Tab { kind, title, active: false, unsaved: false, busy: false, reveal: false, id: None, look: None }
    }

    /// The active tab: `card`, the `ink` line along its top, no line under it, its close shown.
    pub fn active(mut self, active: bool) -> Tab<'a> {
        self.active = active;
        self
    }

    /// Closing it would lose something (query text, staged edits): the `ink` dot after the title.
    pub fn unsaved(mut self, unsaved: bool) -> Tab<'a> {
        self.unsaved = unsaved;
        self
    }

    /// One of its jobs runs: the spinner in its icon's place, as the design has no running mark
    /// for a tab yet.
    pub fn busy(mut self, busy: bool) -> Tab<'a> {
        self.busy = busy;
        self
    }

    /// Scroll the strip it stands in so it shows whole (a tab just made the active one).
    pub fn reveal(mut self, reveal: bool) -> Tab<'a> {
        self.reveal = reveal;
        self
    }

    /// Ids from `salt` (the tab's own id) rather than from its place in the strip, so its focus
    /// and hover stay with it when a tab before it closes.
    pub fn id_salt(mut self, salt: impl Hash + Debug) -> Tab<'a> {
        self.id = Some(Id::new(("dbee-tab", salt)));
        self
    }

    /// Draws it with `look` whatever its response says (the showcase only).
    pub fn look(mut self, look: Look) -> Tab<'a> {
        self.look = Some(look);
        self
    }

    fn icon(&self) -> Icon {
        match self.kind {
            Kind::Table => Icon::TabTable,
            Kind::Query => Icon::TabQuery,
        }
    }

    /// The title, cut with "…" at [`TAB_TITLE_MAX`], the width of its box (whole points, as the
    /// page's), and the tab's width: 12, the icon, 8, the title, [8, the dot], 8, the close, 12,
    /// the line.
    fn parts(&self, ui: &Ui) -> (Arc<Galley>, f32, f32) {
        let mut job = Text::Button.job_as_written(self.title, Color32::PLACEHOLDER);
        job.wrap = TextWrapping::truncate_at_width(TAB_TITLE_MAX);
        let title = ui.painter().layout_job(job);
        let title_w = title.size().x.ceil();
        let dot = if self.unsaved { TAB_GAP + TAB_DOT } else { 0.0 };
        let width = TAB_PAD_X + self.icon().size().x + TAB_GAP + title_w + dot + TAB_GAP + Icon::Close.size().x + TAB_PAD_X + LINE_W;
        (title, title_w, width)
    }

    /// Draws it where `ui` puts it next, and says what a click on it asked for.
    pub fn show(self, ui: &mut Ui) -> Shown {
        let (title, title_w, width) = self.parts(ui);
        let (auto, rect) = ui.allocate_space(vec2(width, TAB_H));
        note(ui, rect, self.reveal);
        let id = self.id.unwrap_or(auto);
        let tab = ui.interact(rect, id, Sense::click());
        tab.widget_info(|| WidgetInfo::selected(WidgetType::SelectableLabel, ui.is_enabled(), self.active, self.title));
        // Its parts are centred in what is above the line along its bottom.
        let under = if self.active { 0.0 } else { LINE_W };
        let mid = rect.top() + (TAB_H - under) / 2.0;
        let icon = self.icon();
        let icon_at = pos2(rect.left() + TAB_PAD_X + icon.size().x / 2.0, mid);
        let title_at = pos2(rect.left() + TAB_PAD_X + icon.size().x + TAB_GAP, mid - title.size().y / 2.0);
        let mut x = title_at.x + title_w + TAB_GAP;
        let unsaved = self.unsaved.then(|| {
            let dot = Rect::from_min_size(pos2(x, mid - TAB_DOT / 2.0), Vec2::splat(TAB_DOT));
            x = dot.right() + TAB_GAP;
            ui.interact(dot, id.with("unsaved"), Sense::hover())
        });
        let close_at = pos2(x + Icon::Close.size().x / 2.0, mid);
        let close_rect = Rect::from_center_size(close_at, Vec2::splat(TAB_CLOSE));
        let close = ui.interact(close_rect, id.with("close"), Sense::click());
        close.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), format!("Close {}", self.title)));
        let look = self.look.unwrap_or_else(|| Look::of(ui, &tab));
        let close_focused = focus::shown(ui, &close);
        let asked = if close.clicked() || close.middle_clicked() || tab.middle_clicked() {
            Some(Asked::Close)
        } else if tab.clicked() && !self.active {
            Some(Asked::Activate)
        } else {
            None
        };
        if ui.is_rect_visible(rect) {
            let t = tokens(ui.ctx());
            let p = painter(ui);
            let hovered = look.hovered || close.hovered();
            let ground = match (self.active, hovered) {
                (true, _) => t.card,
                (false, true) => t.hover,
                (false, false) => t.bg,
            };
            p.rect_filled(rect, CornerRadius::ZERO, ground);
            p.rect_filled(Rect::from_min_max(pos2(rect.right() - LINE_W, rect.top()), rect.max), CornerRadius::ZERO, t.line);
            if self.active {
                p.rect_filled(Rect::from_min_size(rect.min, vec2(rect.width(), TAB_ACTIVE_LINE)), CornerRadius::ZERO, t.ink);
            } else {
                p.rect_filled(Rect::from_min_max(pos2(rect.left(), rect.bottom() - LINE_W), rect.max), CornerRadius::ZERO, t.line);
            }
            let colour = if self.active { t.text } else { t.text_muted };
            if self.busy {
                Spinner::new().paint(ui, icon_at);
            } else {
                icon.paint(&p, icon_at, colour);
            }
            p.galley(title_at, title, colour);
            if let Some(dot) = &unsaved {
                p.circle_filled(dot.rect.center(), TAB_DOT / 2.0, t.ink);
            }
            if close.hovered() {
                p.rect_filled(close_rect, CornerRadius::same(RADIUS_CONTROL), t.hover);
            }
            if self.active || hovered || close_focused {
                Icon::Close.paint(&p, close_at, t.text_muted);
            }
            // The close's square is a cell of the tab, as a bar button's is of its bar.
            if close_focused {
                focus::box_inside(&p, close_rect);
            }
            if look.focused {
                focus::box_inside(&p, rect);
            }
        }
        Shown { tab, close, unsaved, asked }
    }
}

/// The new tab button (`tab/new`): `+` in a cell [`NEW_TAB_W`] wide, on the strip's line.
#[derive(Clone, Copy, Debug, Default)]
pub struct NewTab {
    look: Option<Look>,
}

impl NewTab {
    pub fn new() -> NewTab {
        NewTab { look: None }
    }

    /// Draws it with `look` whatever its response says (the showcase only).
    pub fn look(mut self, look: Look) -> NewTab {
        self.look = Some(look);
        self
    }
}

impl Widget for NewTab {
    fn ui(self, ui: &mut Ui) -> Response {
        let (rect, r) = ui.allocate_exact_size(vec2(NEW_TAB_W, TAB_H), Sense::click());
        r.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), "New query tab"));
        let look = self.look.unwrap_or_else(|| Look::of(ui, &r));
        if !ui.is_rect_visible(rect) {
            return r;
        }
        let t = tokens(ui.ctx());
        let p = painter(ui);
        if look.hovered {
            p.rect_filled(rect, CornerRadius::ZERO, t.hover);
        }
        p.rect_filled(Rect::from_min_max(pos2(rect.left(), rect.bottom() - LINE_W), rect.max), CornerRadius::ZERO, t.line);
        Icon::Plus.paint(&p, pos2(rect.center().x, rect.top() + (TAB_H - LINE_W) / 2.0), t.text_muted);
        if look.focused {
            focus::box_inside(&p, rect);
        }
        r
    }
}

/// The strip: a band [`TAB_H`] tall on `card`, as wide as `ui`, holding what `add` adds side by
/// side (the tabs, then [`NewTab`]) and scrolling sideways when they don't fit. Each tab but the
/// active one and the new tab button draw the line along its bottom under them; the strip draws it
/// on from the last of them to its end. Tabs added straight into the `Ui` `add` gets are the
/// strip's: it rests at their edges, and brings the one to [`Tab::reveal`] into view.
pub fn strip<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let t = tokens(ui.ctx());
    let (band, _) = ui.allocate_exact_size(vec2(ui.available_width(), TAB_H), Sense::hover());
    ui.painter().rect_filled(band, CornerRadius::ZERO, t.card);
    let key = ui.id().with(STRIP_TAG);
    ui.ctx().data_mut(|d| d.insert_temp(key, Cells::default()));
    // Read before the scroll area takes this frame's wheel.
    let scrolling = ui.input(|i| {
        let over = i.pointer.hover_pos().is_some_and(|p| band.contains(p));
        i.pointer.any_down() || (over && i.smooth_scroll_delta != Vec2::ZERO)
    });
    let width = band.width();
    let info = UiStackInfo::default().with_tag_value(STRIP_TAG, key);
    let mut inside = ui.new_child(UiBuilder::new().max_rect(band).layout(egui::Layout::left_to_right(egui::Align::Min)).ui_stack_info(info));
    // Not animated: a scroll to a tab (`scroll_to_me`) lands in the frame it is asked, and the
    // strip comes to rest in that same frame, so nothing moves under a click aimed at it.
    let out = ScrollArea::horizontal().id_salt("tab-strip").auto_shrink([false, false]).animated(false).show(&mut inside, |ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let origin = ui.cursor().left();
        let inner = add(ui);
        let end = ui.min_rect().right();
        let cells = ui.ctx().data_mut(|d| d.remove_temp::<Cells>(key)).unwrap_or_default();
        let tabs: Vec<(f32, f32)> = cells.tabs.iter().map(|(left, right)| (left - origin, right - origin)).collect();
        let rests = Rests::new(&tabs, end - origin, width);
        // Room after the new tab button, so the strip can rest at a tab's edge with it in view.
        let room = rests.last + width - (end - origin);
        if room > 0.0 {
            ui.add_space(room);
        }
        (inner, end, rests, cells.reveal.map(|(left, right)| (left - origin, right - origin)))
    });
    let (inner, end, rests, reveal) = out.inner;
    if end < band.right() {
        let rest = Rect::from_min_max(pos2(end.max(band.left()), band.bottom() - LINE_W), band.max);
        ui.painter().rect_filled(rest, CornerRadius::ZERO, t.line);
    }
    let offset = out.state.offset.x;
    let rest_id = out.id.with("rest");
    let mut rest: Rest = ui.ctx().data(|d| d.get_temp(rest_id)).unwrap_or_default();
    let target = match reveal {
        Some((left, right)) => Some(rests.showing(left, right, offset)),
        None if scrolling || out.state.velocity() != Vec2::ZERO => {
            rest.by_user |= offset != rest.offset;
            None
        }
        None => Some(rests.after(offset, rest)),
    };
    if let Some(target) = target {
        if target != offset {
            let mut state = out.state;
            state.offset.x = target;
            state.store(ui.ctx(), out.id);
            ui.ctx().request_repaint();
        }
        rest = Rest { offset: target, by_user: false };
    }
    ui.ctx().data_mut(|d| d.insert_temp(rest_id, rest));
    inner
}

/// Notes `rect` with the strip `ui` stands in, if it stands in one; `reveal` to bring it into view.
fn note(ui: &Ui, rect: Rect, reveal: bool) {
    let Some(key) = ui.stack().iter().find_map(|s| s.info.tags.get_downcast::<Id>(STRIP_TAG).copied()) else { return };
    ui.ctx().data_mut(|d| {
        let cells = d.get_temp_mut_or_default::<Cells>(key);
        cells.tabs.push((rect.left(), rect.right()));
        if reveal {
            cells.reveal = Some((rect.left(), rect.right()));
        }
    });
}

/// Where a strip can rest, from its left end: at each tab's start, up to the first one from which
/// the strip shows the new tab button whole (`last`). A strip too narrow for the last tab and the
/// button rests last with the button at its left edge.
#[derive(Clone, Debug)]
struct Rests {
    at: Vec<f32>,
    last: f32,
    width: f32,
}

impl Rests {
    /// The rests of `tabs` (each one's left and right), left to right, in content ending at `end`,
    /// seen through a strip `width` wide.
    fn new(tabs: &[(f32, f32)], end: f32, width: f32) -> Rests {
        let button = tabs.last().map_or(0.0, |(_, right)| *right);
        let mut at: Vec<f32> = std::iter::once(0.0).chain(tabs.iter().map(|(left, _)| *left)).chain(std::iter::once(button)).collect();
        at.dedup();
        let last = at.iter().copied().find(|s| s + width >= end).unwrap_or((end - width).max(0.0));
        at.retain(|s| *s < last);
        at.push(last);
        Rests { at, last, width }
    }

    /// The rest nearest `offset` that shows the tab from `left` to `right` whole; a tab wider than
    /// the strip, at its start.
    fn showing(&self, left: f32, right: f32, offset: f32) -> f32 {
        self.at
            .iter()
            .copied()
            .filter(|s| *s <= left + 0.5 && s + self.width >= right - 0.5)
            .min_by(|a, b| (a - offset).abs().total_cmp(&(b - offset).abs()))
            .unwrap_or(left.min(self.last))
    }

    /// Where the strip comes to rest from `offset`: there if it is a rest; after the user scrolled
    /// it, the next rest the way it moved since `rest`; else the nearest.
    fn after(&self, offset: f32, rest: Rest) -> f32 {
        if let Some(at) = self.at.iter().copied().find(|s| (s - offset).abs() < 0.5) {
            return at;
        }
        let next = || self.at.iter().copied().find(|s| *s > offset).unwrap_or(self.last);
        let before = || self.at.iter().copied().rev().find(|s| *s < offset).unwrap_or(0.0);
        match (rest.by_user, offset > rest.offset) {
            (true, true) => next(),
            (true, false) => before(),
            (false, _) => self.at.iter().copied().min_by(|a, b| (a - offset).abs().total_cmp(&(b - offset).abs())).unwrap_or(0.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use egui::{Event, Key, Modifiers, PointerButton, Rect, Theme};
    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::components::{Edge, ROOM, both, edges_in, fills_in, harness, icon_in, painted, texts};
    use crate::theme::Tokens;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= 0.5
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum State {
        Inactive,
        Hover,
        Active,
        Focus,
    }

    /// The fills a tab in `state` draws within `at`, in order: its ground, its right line, then the
    /// line under it or the active line along its top.
    fn tab_fills(at: Rect, state: State, t: &Tokens) -> Vec<(Rect, Color32)> {
        let ground = match state {
            State::Inactive | State::Focus => t.bg,
            State::Hover => t.hover,
            State::Active => t.card,
        };
        let right = Rect::from_min_max(pos2(at.right() - 1.0, at.top()), at.max);
        let last = match state {
            State::Active => (Rect::from_min_size(at.min, vec2(at.width(), 2.0)), t.ink),
            _ => (Rect::from_min_max(pos2(at.left(), at.bottom() - 1.0), at.max), t.line),
        };
        vec![(at, ground), (right, t.line), last]
    }

    #[test]
    fn each_tab_state_matches_the_page() {
        // Node 163793: 36 tall; 12, the icon, 8, the title in `button`, [8, the 8 px `ink` dot], 8,
        // the 7 px close, 12, a 1 px `line` at its right end. Inactive: `bg`, a `line` under it,
        // icon and title `text_muted`, centred above the line; hover: `hover`, the close shown;
        // active: `card`, no line under it, the 2 px `ink` line along its top, icon and title
        // `text`, the close shown; focus: a 2 px `ink` box 2 px inside. The close is `text_muted`.
        // The page's widths with the close shown (the close's room is kept on every tab): table
        // 122, with the dot 138; query 106, with the dot 122.
        for (theme, t) in both() {
            for (kind, title, widths) in [(Kind::Table, "orders [3]", [122.0, 138.0]), (Kind::Query, "Query 1", [106.0, 122.0])] {
                for state in [State::Inactive, State::Hover, State::Active, State::Focus] {
                    let at_ = format!("{theme:?} {kind:?} {state:?}");
                    let mut at = Rect::NOTHING;
                    let shapes = painted(theme, |ui| {
                        ui.add_space(ROOM);
                        let tab = Tab::new(kind, title).active(state == State::Active).unsaved(state == State::Active);
                        let tab = match state {
                            State::Hover => tab.look(Look { hovered: true, ..Look::default() }),
                            State::Focus => tab.look(Look { focused: true, ..Look::default() }),
                            _ => tab,
                        };
                        at = tab.show(ui).tab.rect;
                    });
                    assert_eq!(at.height(), 36.0, "{at_}: 36 tall");
                    let page = if state == State::Active { widths[1] } else { widths[0] };
                    assert!((at.width() - page).abs() <= 1.0, "{at_}: {} wide, the page's {page}", at.width());
                    assert_eq!(fills_in(&shapes, at), tab_fills(at, state, &t), "{at_}: the ground and the lines");
                    let focus = Edge { outer: at.shrink(2.0), width: 2.0, colour: t.ink, radius: CornerRadius::same(2) };
                    let want: Vec<Edge> = if state == State::Focus { vec![focus] } else { vec![] };
                    assert_eq!(edges_in(&shapes, at, 0.0), want, "{at_}: the focus box");
                    let mid = if state == State::Active { at.top() + 18.0 } else { at.top() + 17.5 };
                    let colour = if state == State::Active { t.text } else { t.text_muted };
                    let icon = Tab::new(kind, title).icon();
                    let icon_box =
                        Rect::from_min_max(pos2(at.left() + 12.0, at.top() + 2.0), pos2(at.left() + 12.0 + icon.size().x, at.bottom() - 1.0));
                    let (colours, bounds) = icon_in(&shapes, icon_box.expand(2.0));
                    assert_eq!(colours, [colour], "{at_}: the icon's colour");
                    assert!(
                        (bounds.center().x - icon_box.center().x).abs() <= 0.6 && (bounds.center().y - mid).abs() <= 0.6,
                        "{at_}: the icon {bounds:?}"
                    );
                    let drawn = texts(&shapes);
                    let (_, words, c) = drawn.iter().find(|(w, _, _)| w == title).unwrap_or_else(|| panic!("{at_}: {title} drawn: {drawn:?}"));
                    assert_eq!(*c, colour, "{at_}: the title's colour");
                    assert!(close(words.left(), at.left() + 12.0 + icon.size().x + 8.0), "{at_}: the title {words:?} 8 after the icon");
                    assert!(close(words.center().y, mid), "{at_}: the title centred above the line");
                    let mut x = words.left() + words.width().ceil() + 8.0;
                    let dots: Vec<(egui::Pos2, f32, Color32)> = crate::components::flat(&shapes)
                        .into_iter()
                        .filter_map(|s| match s {
                            // Not the round ends of the icons' lines.
                            egui::Shape::Circle(c) if at.contains(c.center) && c.radius > 1.0 => Some((c.center, c.radius, c.fill)),
                            _ => None,
                        })
                        .collect();
                    if state == State::Active {
                        assert_eq!(dots, [(pos2(x + 4.0, mid), 4.0, t.ink)], "{at_}: the unsaved dot 8 after the title");
                        x += 16.0;
                    } else {
                        assert_eq!(dots, [], "{at_}: no dot");
                    }
                    let glyph = Rect::from_min_size(pos2(x, mid - 3.5), Vec2::splat(7.0));
                    let (colours, bounds) = icon_in(&shapes, glyph.expand(2.0));
                    if matches!(state, State::Hover | State::Active) {
                        assert_eq!(colours, [t.text_muted], "{at_}: the close shows");
                        // Moved by up to half a pixel each way, so its lines fall on the pixel grid.
                        let off = bounds.center() - glyph.center();
                        assert!(off.x.abs() <= 0.55 && off.y.abs() <= 0.55, "{at_}: the close {bounds:?} at {glyph:?}");
                        assert_eq!(at.right() - 1.0 - 12.0, glyph.right(), "{at_}: 12 between the close and the line");
                    } else {
                        assert_eq!(colours, [], "{at_}: no close");
                    }
                }
            }
        }
    }

    #[test]
    fn the_new_tab_button_matches_the_page() {
        // Node 163861: 32 by 36, a `line` under it; `+` in `text_muted`, centred above the line;
        // hover `hover`; focus a 2 px `ink` box 2 px inside.
        for (theme, t) in both() {
            for state in [State::Inactive, State::Hover, State::Focus] {
                let mut at = Rect::NOTHING;
                let shapes = painted(theme, |ui| {
                    ui.add_space(ROOM);
                    let b = match state {
                        State::Hover => NewTab::new().look(Look { hovered: true, ..Look::default() }),
                        State::Focus => NewTab::new().look(Look { focused: true, ..Look::default() }),
                        _ => NewTab::new(),
                    };
                    at = ui.add(b).rect;
                });
                assert_eq!(at.size(), vec2(32.0, 36.0), "{theme:?} {state:?}");
                let line = (Rect::from_min_max(pos2(at.left(), at.bottom() - 1.0), at.max), t.line);
                let want = if state == State::Hover { vec![(at, t.hover), line] } else { vec![line] };
                assert_eq!(fills_in(&shapes, at), want, "{theme:?} {state:?}: the ground and the line");
                let focus = Edge { outer: at.shrink(2.0), width: 2.0, colour: t.ink, radius: CornerRadius::same(2) };
                let want: Vec<Edge> = if state == State::Focus { vec![focus] } else { vec![] };
                assert_eq!(edges_in(&shapes, at, 0.0), want, "{theme:?} {state:?}: the focus box");
                let (colours, bounds) = icon_in(&shapes, at);
                assert_eq!(colours, [t.text_muted], "{theme:?} {state:?}: the plus");
                assert!((bounds.center() - pos2(at.center().x, at.top() + 17.5)).length() <= 0.6, "{theme:?} {state:?}: {bounds:?}");
            }
        }
    }

    #[test]
    fn the_strip_lies_on_card_with_the_line_under_inactive_tabs() {
        // Node 163870: on `card`, 36 tall; the tabs from its left end, side by side, then the new
        // tab button; the `line` along its bottom under all but the active tab, on to its end.
        for (theme, t) in both() {
            let mut at: Vec<Rect> = Vec::new();
            let mut band = Rect::NOTHING;
            let shapes = painted(theme, |ui| {
                at.clear();
                ui.add_space(ROOM);
                ui.allocate_ui(vec2(560.0, 36.0), |ui| {
                    band = ui.max_rect();
                    strip(ui, |ui| {
                        at.push(Tab::table("orders [3]").show(ui).tab.rect);
                        at.push(Tab::table("customers [1]").show(ui).tab.rect);
                        at.push(Tab::query("Query 1").active(true).unsaved(true).show(ui).tab.rect);
                        at.push(ui.add(NewTab::new()).rect);
                    });
                });
            });
            let band = Rect::from_min_size(band.min, vec2(560.0, 36.0));
            assert_eq!(fills_in(&shapes, band).first(), Some(&(band, t.card)), "{theme:?}: the band on card");
            assert_eq!(at[0].left(), band.left(), "{theme:?}: the first tab at the strip's left end");
            for pair in at.windows(2) {
                assert_eq!(pair[0].right(), pair[1].left(), "{theme:?}: side by side: {at:?}");
            }
            for r in &at {
                assert_eq!((r.top(), r.bottom()), (band.top(), band.bottom()), "{theme:?}: {r:?} fills the band's height");
            }
            // The 1 px lines along the bottom, end to end: one gap, the active tab's width.
            let mut under: Vec<(f32, f32)> = fills_in(&shapes, band)
                .into_iter()
                .filter(|(r, c)| *c == t.line && r.height() == 1.0 && r.bottom() == band.bottom())
                .map(|(r, _)| (r.left(), r.right()))
                .collect();
            under.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut gaps = Vec::new();
            let mut x = band.left();
            for (l, r) in under {
                if l > x {
                    gaps.push((x, l));
                }
                x = x.max(r);
            }
            assert_eq!(x, band.right(), "{theme:?}: the line runs to the strip's end");
            assert_eq!(gaps, [(at[2].left(), at[2].right())], "{theme:?}: no line under the active tab only");
        }
    }

    /// What the harness's tabs asked for, the last time each did.
    struct Seen {
        asked: [Option<Asked>; 2],
        rects: [Rect; 2],
    }

    /// Two tabs side by side, the first one active.
    fn two_tabs() -> egui_kittest::Harness<'static, Seen> {
        let mut h = harness(Theme::Light, Seen { asked: [None; 2], rects: [Rect::NOTHING; 2] }, |ui, s| {
            ui.horizontal(|ui| {
                for (i, active) in [(0, true), (1, false)] {
                    let shown = Tab::table(if active { "orders" } else { "customers" }).active(active).id_salt(i).show(ui);
                    s.rects[i] = shown.tab.rect;
                    if shown.asked.is_some() {
                        s.asked[i] = shown.asked;
                    }
                }
            });
        });
        h.run();
        h
    }

    #[test]
    fn a_tab_says_what_was_asked() {
        let mut h = two_tabs();
        h.get_by_label("customers").click();
        h.run();
        assert_eq!(h.state().asked, [None, Some(Asked::Activate)], "a click on an inactive tab activates it");
        h.get_by_label("orders").click();
        h.run();
        assert_eq!(h.state().asked[0], None, "a click on the active tab asks nothing");
        h.get_by_label("Close orders").click();
        h.run();
        assert_eq!(h.state().asked[0], Some(Asked::Close), "its close closes it");
        h.state_mut().asked = [None, None];
        let pos = h.state().rects[1].center();
        for pressed in [true, false] {
            h.input_mut().events.push(Event::PointerMoved(pos));
            h.input_mut().events.push(Event::PointerButton { pos, button: PointerButton::Middle, pressed, modifiers: Modifiers::NONE });
            h.run();
        }
        assert_eq!(h.state().asked, [None, Some(Asked::Close)], "a middle click closes it");
        // By keyboard: Tab goes to the first tab, its close, the second tab; Enter activates it.
        let mut h = two_tabs();
        for _ in 0..3 {
            h.key_press(Key::Tab);
            h.run();
        }
        h.key_press(Key::Enter);
        h.run();
        assert_eq!(h.state().asked, [None, Some(Asked::Activate)], "Enter on the focused tab activates it");
    }

    #[test]
    fn a_tab_title_keeps_its_case() {
        // `Orders` is a table's name: drawn and reported as written.
        let mut h = harness(Theme::Light, (), |ui, _| {
            ui.horizontal(|ui| {
                Tab::table("Orders").active(true).show(ui);
                Tab::query("Query 1").show(ui);
            });
        });
        h.run();
        let drawn: Vec<String> = texts(&h.output().shapes).into_iter().map(|(w, _, _)| w).collect();
        assert_eq!(drawn, ["Orders", "Query 1"]);
        assert!(h.query_by_label("Orders").is_some(), "reported as written");
        assert!(h.query_by_label("ORDERS").is_none());
    }
}
