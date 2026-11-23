//! The workspace's tabs (`tab`), the new tab button (`tab/new`), the list button (`tab/list`)
//! and the strip they stand in (`tab strip`): a band on `card` with a `line` along its bottom
//! under everything but the active tab, which opens into what is below it.
//!
//! A tab holds, from its left, its kind's icon (the spinner while it is busy), its title as
//! written, the unsaved dot and the close, and a `line` at its right end. Its close shows while it
//! is the active tab or under the pointer. The close's room is kept on every tab, so no tab grows
//! under the pointer and every tab's close can be clicked by its tag.
//!
//! While the tabs fit, the new tab button stands after the last one. While they don't, they
//! scroll sideways in what the strip leaves before the new tab button and the list button, which
//! stand at its right end; the list button opens a menu of every tab in strip order, the active
//! one checked, and choosing one asks for it to be made the active tab. The tabs come to rest
//! with a tab's left edge at the strip's left, or at the end with the last tab against the new
//! tab button, so no room is left empty after it (`row`, which the result bar's tabs use too):
//! after the wheel (either way it turns) or the scroll bar they go on to the next rest the way
//! they moved; after a scroll they didn't do themselves, to the nearest one. The active tab is
//! brought into view whenever another one becomes active (a click, the keyboard, the list, the
//! caller) and when the strip's room or its number of tabs changes, and so is a tab the
//! keyboard's focus reaches.

use std::fmt::Debug;
use std::hash::Hash;
use std::sync::Arc;

use egui::text::TextWrapping;
use egui::{
    Align, Color32, CornerRadius, Galley, Id, Layout, Popup, Rect, RectAlign, Response, ScrollArea, Sense, Ui, UiBuilder, Vec2, Widget, WidgetInfo,
    WidgetType, pos2, vec2,
};

use super::icon::Icon;
use super::menu::{self, Item};
use super::spinner::Spinner;
use super::{Look, focus, painter};
use crate::theme::size::{
    MENU_ITEM_H, MENU_PAD, NEW_TAB_W, RADIUS_CONTROL, TAB_ACTIVE_LINE, TAB_CLOSE, TAB_DOT, TAB_GAP, TAB_H, TAB_LIST_GAP, TAB_PAD_X, TAB_TITLE_MAX,
};
use crate::theme::{Text, tokens};

/// The strip's lines: at a tab's right end and along the bottom.
const LINE_W: f32 = 1.0;

/// What the strip's list button is read out as.
const ALL_TABS: &str = "All tabs";

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
        Tab { kind, title, active: false, unsaved: false, busy: false, id: None, look: None }
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

    /// One of its jobs runs: the spinner in its icon's place; a tab has no other running
    /// mark.
    pub fn busy(mut self, busy: bool) -> Tab<'a> {
        self.busy = busy;
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

    /// The title, cut with "…" at [`TAB_TITLE_MAX`], the width of its box (whole points),
    /// and the tab's width: 12, the icon, 8, the title, [8, the dot], 8, the close, 12,
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

    /// How wide it draws in `ui`.
    pub(super) fn width(&self, ui: &Ui) -> f32 {
        self.parts(ui).2
    }

    /// Draws it where `ui` puts it next, and says what a click on it asked for.
    pub fn show(self, ui: &mut Ui) -> Shown {
        let (title, title_w, width) = self.parts(ui);
        let (auto, rect) = ui.allocate_space(vec2(width, TAB_H));
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

/// The list button (`tab/list`): ⌄ in a cell [`NEW_TAB_W`] wide and [`TAB_H`] tall, on the line
/// along the bottom of what it stands in; `fill` while its list is open, as an open menu title. A
/// strip or a result bar shows it only while their tabs don't fit (`list_button`).
#[derive(Clone, Copy, Debug)]
pub struct ListButton<'a> {
    label: &'a str,
    open: bool,
    look: Option<Look>,
}

impl<'a> ListButton<'a> {
    /// `label` is what it is read out as: what it lists.
    pub fn new(label: &'a str) -> ListButton<'a> {
        ListButton { label, open: false, look: None }
    }

    /// Its list is open.
    pub fn open(mut self, open: bool) -> ListButton<'a> {
        self.open = open;
        self
    }

    /// Draws it with `look` whatever its response says (the showcase only).
    pub fn look(mut self, look: Look) -> ListButton<'a> {
        self.look = Some(look);
        self
    }
}

impl Widget for ListButton<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let (rect, r) = ui.allocate_exact_size(vec2(NEW_TAB_W, TAB_H), Sense::click());
        r.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), self.label));
        let look = self.look.unwrap_or_else(|| Look::of(ui, &r));
        paint_list_button(ui, rect, self.open, look);
        r
    }
}

/// Draws the list button at `rect`.
fn paint_list_button(ui: &Ui, rect: Rect, open: bool, look: Look) {
    if !ui.is_rect_visible(rect) {
        return;
    }
    let t = tokens(ui.ctx());
    let p = painter(ui);
    if open {
        p.rect_filled(rect, CornerRadius::ZERO, t.fill);
    } else if look.hovered {
        p.rect_filled(rect, CornerRadius::ZERO, t.hover);
    }
    p.rect_filled(Rect::from_min_max(pos2(rect.left(), rect.bottom() - LINE_W), rect.max), CornerRadius::ZERO, t.line);
    Icon::ChevronDown.paint(&p, pos2(rect.center().x, rect.top() + (TAB_H - LINE_W) / 2.0), t.text_muted);
    if look.focused {
        focus::box_inside(&p, rect);
    }
}

/// A list button as drawn: the button, its list's items in view while the list is open (each with
/// the index of what it lists), and the index chosen from it.
pub(super) struct Listed {
    pub button: Response,
    pub items: Vec<(usize, Response)>,
    pub chosen: Option<usize>,
}

/// The list button at `rect`, read out as `label`. A click opens under it, its right edge at the
/// button's, a menu (`menu`, `menu/item`) of `titles` as written, in their order, `active`
/// checked in the mark column; past the window's bottom the menu scrolls. `open` draws the button
/// open or closed whatever its list does, and shows no list (the showcase only).
pub(super) fn list_button(ui: &mut Ui, id: Id, rect: Rect, label: &str, titles: &[&str], active: Option<usize>, open: Option<bool>) -> Listed {
    let button = ui.interact(rect, id, Sense::click());
    button.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), label));
    let look = Look::of(ui, &button);
    let mut items = Vec::new();
    let mut chosen = None;
    let open = open.unwrap_or_else(|| {
        Popup::menu(&button)
            .align(RectAlign::BOTTOM_END)
            .gap(TAB_LIST_GAP)
            .style(menu::style(ui.ctx()))
            .show(|ui| {
                // The room under the button, worked out from the button and not from where the
                // menu is (egui sizes it first somewhere else): a list that fits under it never
                // flips above it, so the arrows go down into it from the button.
                let room = ui.ctx().content_rect().bottom() - rect.bottom() - TAB_LIST_GAP - 2.0 * (f32::from(MENU_PAD) + LINE_W);
                ScrollArea::vertical().max_height(room.max(MENU_ITEM_H)).show_rows(ui, MENU_ITEM_H, titles.len(), |ui, rows| {
                    for (i, title) in titles.iter().enumerate().take(rows.end).skip(rows.start) {
                        let r = ui.add(Item::new(title).choice(Some(active == Some(i))));
                        if r.clicked() {
                            chosen = Some(i);
                            // A key chooses too, and egui closes the list only on a click.
                            ui.close();
                        }
                        items.push((i, r));
                    }
                });
            })
            .is_some()
    });
    paint_list_button(ui, rect, open, look);
    Listed { button, items, chosen }
}

/// The strip (`tab strip`): its tabs, the new tab button, and the list button while the tabs
/// don't fit.
pub struct Strip<'a> {
    tabs: Vec<Tab<'a>>,
    list_open: Option<bool>,
}

/// The strip as drawn: each tab, the new tab button, the list button while the tabs don't fit and
/// its list's items in view while it is open (each with the index of the tab it lists), and what
/// was asked of which tab, by a click on it or its close or by choosing it from the list.
pub struct StripShown {
    pub tabs: Vec<Shown>,
    pub new: Response,
    pub list: Option<Response>,
    pub listed: Vec<(usize, Response)>,
    pub asked: Option<(usize, Asked)>,
}

impl<'a> Strip<'a> {
    /// `tabs` in their order, the active one made so with [`Tab::active`].
    pub fn new(tabs: Vec<Tab<'a>>) -> Strip<'a> {
        Strip { tabs, list_open: None }
    }

    /// Draws the list button open, or closed, whatever its list does, and shows no list (the
    /// showcase only).
    pub fn list_open(mut self, open: bool) -> Strip<'a> {
        self.list_open = Some(open);
        self
    }

    /// Draws it [`TAB_H`] tall and as wide as `ui`, calling `each` with every tab's index and parts
    /// as drawn. A screen tags them there: in the strip's scrolled part, so that a tag asking to
    /// scroll a tab into view reaches it.
    pub fn show(self, ui: &mut Ui, mut each: impl FnMut(usize, &Shown)) -> StripShown {
        let t = tokens(ui.ctx());
        let (band, band_r) = ui.allocate_exact_size(vec2(ui.available_width(), TAB_H), Sense::hover());
        let id = band_r.id;
        ui.painter().rect_filled(band, CornerRadius::ZERO, t.card);
        let mut ui = ui.new_child(UiBuilder::new().id_salt(id).max_rect(band).layout(Layout::left_to_right(Align::Min)));
        let widths: Vec<f32> = self.tabs.iter().map(|tab| tab.width(&ui)).collect();
        let titles: Vec<&str> = self.tabs.iter().map(|tab| tab.title).collect();
        let active = self.tabs.iter().enumerate().find(|(_, tab)| tab.active).map(|(i, tab)| (i, tab.id.unwrap_or_else(|| id.with(i))));
        let fits = widths.iter().sum::<f32>() + NEW_TAB_W <= band.width();
        let pinned = if fits { 0.0 } else { 2.0 * NEW_TAB_W };
        let view = Rect::from_min_max(band.min, pos2((band.right() - pinned).max(band.left()), band.bottom()));
        let mut asked = None;
        let row = row(&mut ui, id, view, self.tabs, &widths, active, |ui, i, tab| {
            let shown = tab.show(ui);
            each(i, &shown);
            if let Some(a) = shown.asked {
                asked = Some((i, a));
            }
            let focused = shown.tab.gained_focus() || shown.close.gained_focus();
            Placed { item: shown, focused }
        });
        let new_at = if fits { row.end } else { view.right() };
        let new = ui.put(Rect::from_min_size(pos2(new_at, band.top()), vec2(NEW_TAB_W, TAB_H)), NewTab::new());
        let (list, listed) = if fits {
            (None, Vec::new())
        } else {
            let rect = Rect::from_min_size(pos2(new.rect.right(), band.top()), vec2(NEW_TAB_W, TAB_H));
            let listed = list_button(&mut ui, id.with("list"), rect, ALL_TABS, &titles, active.map(|(i, _)| i), self.list_open);
            if let Some(i) = listed.chosen {
                asked = Some((i, Asked::Activate));
            }
            (Some(listed.button), listed.items)
        };
        if new.rect.right() < band.right() {
            let rest = Rect::from_min_max(pos2(new.rect.right(), band.bottom() - LINE_W), band.max);
            ui.painter().rect_filled(rest, CornerRadius::ZERO, t.line);
        }
        StripShown { tabs: row.tabs, new, list, listed, asked }
    }
}

/// A tab of a [`row`] as drawn: what drawing it gave, and whether it took the keyboard's focus.
pub(super) struct Placed<T> {
    pub item: T,
    pub focused: bool,
}

/// A [`row`] as drawn: what each tab's drawing gave, and where the tabs end on screen (at most at
/// the row's right end).
pub(super) struct Row<T> {
    pub tabs: Vec<T>,
    pub end: f32,
}

/// What a row keeps between frames.
#[derive(Clone, Copy, Debug, Default)]
struct Memo {
    /// Where its scroll area left it, and whether it was still moving by itself (a fling).
    offset: f32,
    moving: bool,
    /// Where it last came to rest, and whether the user has scrolled it since.
    rest: f32,
    by_user: bool,
    /// The tab that took the keyboard's focus.
    focused: Option<usize>,
    /// The active tab, the number of tabs and the room it last showed them in.
    seen: Option<(Option<Id>, usize, f32)>,
}

/// `items`, `widths` wide, side by side from `view`'s left end, each drawn by `draw`, scrolling
/// sideways in `view` while they don't fit: where they rest, and what they bring into view, is in
/// the module docs. `active` is the active tab's index and its own id. Where to scroll is worked
/// out from `widths` before drawing, so the tabs are drawn there in the same frame.
pub(super) fn row<I, T>(
    ui: &mut Ui,
    id: Id,
    view: Rect,
    items: Vec<I>,
    widths: &[f32],
    active: Option<(usize, Id)>,
    mut draw: impl FnMut(&mut Ui, usize, I) -> Placed<T>,
) -> Row<T> {
    let mut cells = Vec::with_capacity(widths.len());
    let mut end = 0.0;
    for w in widths {
        cells.push((end, end + w));
        end += w;
    }
    let rests = Rests::new(&cells, end, view.width());
    let key = id.with("rest");
    let mut memo: Memo = ui.ctx().data(|d| d.get_temp(key)).unwrap_or_default();
    let live = memo.offset.clamp(0.0, rests.last);
    let scrolling = memo.moving
        || ui.input(|i| {
            let over = i.pointer.hover_pos().is_some_and(|p| view.contains(p));
            i.pointer.any_down() || (over && i.smooth_scroll_delta != Vec2::ZERO)
        });
    let seen = (active.map(|(_, id)| id), widths.len(), view.width());
    let reveal = memo.focused.take().or_else(|| active.filter(|_| memo.seen != Some(seen)).map(|(i, _)| i));
    memo.seen = Some(seen);
    let target = match reveal.and_then(|i| cells.get(i)) {
        Some(&(left, right)) => Some(rests.showing(left, right, live)),
        None if scrolling => {
            memo.by_user |= live != memo.rest;
            None
        }
        None => Some(rests.after(live, memo.rest, memo.by_user)),
    };
    if let Some(target) = target {
        memo.rest = target;
        memo.by_user = false;
    }
    let offset = target.unwrap_or(live);
    let mut inside = ui.new_child(UiBuilder::new().id_salt(id.with("view")).max_rect(view).layout(Layout::left_to_right(Align::Min)));
    inside.style_mut().always_scroll_the_only_direction = true;
    // Not animated, so a scroll to a tab (a tag's `scroll_to_me`) lands in the frame it is asked.
    let area = ScrollArea::horizontal().id_salt("tabs").auto_shrink([false, false]).min_scrolled_width(0.0).animated(false);
    let out = area.horizontal_scroll_offset(offset).show(&mut inside, |ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let origin = ui.cursor().left();
        let mut focused = None;
        let mut tabs = Vec::with_capacity(items.len());
        for (i, item) in items.into_iter().enumerate() {
            let placed = draw(ui, i, item);
            if placed.focused {
                focused = Some(i);
            }
            tabs.push(placed.item);
        }
        (tabs, origin, focused)
    });
    let (tabs, origin, focused) = out.inner;
    memo.offset = out.state.offset.x;
    memo.moving = out.state.velocity() != Vec2::ZERO;
    memo.focused = focused;
    if (memo.offset - offset).abs() > 0.5 || focused.is_some() {
        ui.ctx().request_repaint();
    }
    ui.ctx().data_mut(|d| d.insert_temp(key, memo));
    Row { tabs, end: (origin + end).min(view.right()) }
}

/// Where a row can rest, from its left end: at each tab's left edge, up to its end, where the last
/// tab's right edge is at the row's (`last`).
#[derive(Clone, Debug)]
struct Rests {
    at: Vec<f32>,
    last: f32,
    width: f32,
}

impl Rests {
    /// The rests of `tabs` (each one's left and right), left to right, in content `end` wide, seen
    /// through a row `width` wide.
    fn new(tabs: &[(f32, f32)], end: f32, width: f32) -> Rests {
        let last = (end - width).max(0.0);
        let mut at: Vec<f32> = std::iter::once(0.0).chain(tabs.iter().map(|(left, _)| *left)).filter(|s| *s < last).collect();
        at.dedup();
        at.push(last);
        Rests { at, last, width }
    }

    /// The rest nearest `offset` that shows the tab from `left` to `right` whole; a tab wider than
    /// the row, at its start.
    fn showing(&self, left: f32, right: f32, offset: f32) -> f32 {
        self.at
            .iter()
            .copied()
            .filter(|s| *s <= left + 0.5 && s + self.width >= right - 0.5)
            .min_by(|a, b| (a - offset).abs().total_cmp(&(b - offset).abs()))
            .unwrap_or(left.min(self.last))
    }

    /// Where the row comes to rest from `offset`: there if it is a rest; after the user scrolled
    /// it (`by_user`), the next rest the way it moved since it rested at `rest`; else the nearest.
    fn after(&self, offset: f32, rest: f32, by_user: bool) -> f32 {
        if let Some(at) = self.at.iter().copied().find(|s| (s - offset).abs() < 0.5) {
            return at;
        }
        let next = || self.at.iter().copied().find(|s| *s > offset).unwrap_or(self.last);
        let before = || self.at.iter().copied().rev().find(|s| *s < offset).unwrap_or(0.0);
        match (by_user, offset > rest) {
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
    use crate::components::{Edge, ROOM, both, edges_in, fills_in, harness, harness_in, icon_in, painted, texts};
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
    fn each_tab_state_has_its_look() {
        // 36 tall; 12, the icon, 8, the title in `button`, [8, the 8 px `ink` dot], 8,
        // the 7 px close, 12, a 1 px `line` at its right end. Inactive: `bg`, a `line` under it,
        // icon and title `text_muted`, centred above the line; hover: `hover`, the close shown;
        // active: `card`, no line under it, the 2 px `ink` line along its top, icon and title
        // `text`, the close shown; focus: a 2 px `ink` box 2 px inside. The close is `text_muted`.
        // The widths with the close shown (the close's room is kept on every tab): table
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
                    assert!((at.width() - page).abs() <= 1.0, "{at_}: {} wide, not {page}", at.width());
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
    fn the_new_tab_button_has_its_look() {
        // 32 by 36, a `line` under it; `+` in `text_muted`, centred above the line;
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
        // On `card`, 36 tall; the tabs from its left end, side by side, then the new
        // tab button; the `line` along its bottom under all but the active tab, on to its end.
        for (theme, t) in both() {
            let mut at: Vec<Rect> = Vec::new();
            let mut band = Rect::NOTHING;
            let shapes = painted(theme, |ui| {
                at.clear();
                ui.add_space(ROOM);
                ui.allocate_ui(vec2(560.0, 36.0), |ui| {
                    band = ui.max_rect();
                    let tabs = vec![Tab::table("orders [3]"), Tab::table("customers [1]"), Tab::query("Query 1").active(true).unsaved(true)];
                    let shown = Strip::new(tabs).show(ui, |_, _| {});
                    at.extend(shown.tabs.iter().map(|s| s.tab.rect));
                    at.push(shown.new.rect);
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

    /// Seven tabs that don't fit, in strip order: each title and whether it is a
    /// query tab. Query 1 has unsaved text.
    const PAGE: [(&str, bool); 7] = [
        ("orders [3]", false),
        ("customers [1]", false),
        ("Query 1", true),
        ("line_items", false),
        ("Query 2", true),
        ("Odd \"Name\"", false),
        ("dealers", false),
    ];
    /// The strip is drawn 840 wide.
    const STRIP: f32 = 840.0;

    /// The first `n` of those tabs, `active` the active one.
    fn page_tabs(n: usize, active: usize) -> Vec<Tab<'static>> {
        PAGE.iter()
            .take(n)
            .enumerate()
            .map(|(i, (title, query))| {
                let tab = if *query { Tab::query(title) } else { Tab::table(title) };
                tab.id_salt(i).active(i == active).unsaved(i == 2)
            })
            .collect()
    }

    /// What a harness's strip drew in its last frame, the tab its caller made active, and how
    /// wide the strip is and how many of those tabs it holds.
    struct Drawn {
        active: usize,
        width: f32,
        count: usize,
        band: Rect,
        tabs: Vec<Rect>,
        new: Rect,
        list: Option<Rect>,
        items: Vec<(usize, Rect)>,
        asked: Vec<(usize, Asked)>,
    }

    /// A strip `width` wide of the seven tabs in `theme`, room under it for its list; the
    /// caller makes a tab active when it is asked to.
    fn seven(theme: Theme, width: f32, active: usize) -> egui_kittest::Harness<'static, Drawn> {
        let drawn = Drawn {
            active,
            width,
            count: 7,
            band: Rect::NOTHING,
            tabs: Vec::new(),
            new: Rect::NOTHING,
            list: None,
            items: Vec::new(),
            asked: Vec::new(),
        };
        let mut h = harness_in(vec2(width + 2.0 * ROOM, 400.0), theme, drawn, move |ui, s| {
            ui.add_space(ROOM);
            ui.allocate_ui(vec2(s.width, TAB_H), |ui| {
                s.band = Rect::from_min_size(ui.cursor().min, vec2(s.width, TAB_H));
                let shown = Strip::new(page_tabs(s.count, s.active)).show(ui, |_, _| {});
                s.tabs = shown.tabs.iter().map(|t| t.tab.rect).collect();
                s.new = shown.new.rect;
                s.list = shown.list.map(|r| r.rect);
                s.items = shown.listed.iter().map(|(i, r)| (*i, r.rect)).collect();
                if let Some(asked) = shown.asked {
                    s.asked.push(asked);
                    if asked.1 == Asked::Activate {
                        s.active = asked.0;
                    }
                }
            });
        });
        h.run();
        h
    }

    /// A click of the primary button at `pos`.
    fn click_at<S>(h: &mut egui_kittest::Harness<'_, S>, pos: egui::Pos2) {
        h.input_mut().events.push(Event::PointerMoved(pos));
        for pressed in [true, false] {
            h.input_mut().events.push(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
            h.run();
        }
    }

    /// Tab `i` shows whole between the strip's left end and the new tab button.
    fn whole_in_view(s: &Drawn, i: usize) -> bool {
        s.tabs.get(i).is_some_and(|r| r.left() >= s.band.left() - 0.5 && r.right() <= s.new.left() + 0.5)
    }

    #[test]
    fn the_list_button_has_its_look() {
        // 32 by 36 like `tab/new`, a `line` under it; ⌄ (`icon/chevron-down`, 8 by
        // 4 at 12, 15.5) in `text_muted`, centred above the line; hover `hover`; focus a 2 px `ink`
        // box 2 px inside; open `fill`, as an open menu title.
        for (theme, t) in both() {
            for state in ["default", "hover", "focus", "open"] {
                let at_ = format!("{theme:?} {state}");
                let mut at = Rect::NOTHING;
                let shapes = painted(theme, |ui| {
                    ui.add_space(ROOM);
                    let b = ListButton::new("All tabs");
                    let b = match state {
                        "hover" => b.look(Look { hovered: true, ..Look::default() }),
                        "focus" => b.look(Look { focused: true, ..Look::default() }),
                        "open" => b.open(true),
                        _ => b,
                    };
                    at = ui.add(b).rect;
                });
                assert_eq!(at.size(), vec2(32.0, 36.0), "{at_}");
                let line = (Rect::from_min_max(pos2(at.left(), at.bottom() - 1.0), at.max), t.line);
                let want = match state {
                    "hover" => vec![(at, t.hover), line],
                    "open" => vec![(at, t.fill), line],
                    _ => vec![line],
                };
                assert_eq!(fills_in(&shapes, at), want, "{at_}: the ground and the line");
                let focus = Edge { outer: at.shrink(2.0), width: 2.0, colour: t.ink, radius: CornerRadius::same(2) };
                let want: Vec<Edge> = if state == "focus" { vec![focus] } else { vec![] };
                assert_eq!(edges_in(&shapes, at, 0.0), want, "{at_}: the focus box");
                let (colours, bounds) = icon_in(&shapes, at);
                assert_eq!(colours, [t.text_muted], "{at_}: the chevron");
                assert!((bounds.center() - pos2(at.center().x, at.top() + 17.5)).length() <= 0.6, "{at_}: centred above the line: {bounds:?}");
                assert!(bounds.width() > bounds.height() && (8.0..=10.0).contains(&bounds.width()), "{at_}: the chevron down, 8 by 4: {bounds:?}");
            }
        }
    }

    #[test]
    fn a_strip_that_fits_has_no_list_button() {
        // No overflow: the tabs, then the new tab button right after the last
        // one, the line on to the strip's end; no list button.
        for (theme, t) in both() {
            let h = seven(theme, 1000.0, 2);
            let s = h.state();
            assert!(s.list.is_none(), "{theme:?}: no list button while the tabs fit");
            assert_eq!(s.tabs[0].left(), s.band.left(), "{theme:?}: the first tab at the strip's left end");
            assert_eq!(s.new, Rect::from_min_size(pos2(s.tabs[6].right(), s.band.top()), vec2(32.0, 36.0)), "{theme:?}: + after the last tab");
            let rest = Rect::from_min_max(pos2(s.new.right(), s.band.bottom() - 1.0), s.band.max);
            let shapes = &h.output().shapes;
            assert!(fills_in(shapes, s.band).contains(&(rest, t.line)), "{theme:?}: the line on to the end");
            let after = Rect::from_min_max(pos2(s.new.right(), s.band.top()), s.band.max);
            assert_eq!(icon_in(shapes, after).0, [], "{theme:?}: nothing drawn after +");
        }
    }

    #[test]
    fn a_strip_that_overflows_pins_new_and_list_at_its_right_end() {
        // With overflow: the tabs scroll in what the strip leaves before + and the
        // list button, which stand at its right end, outside the scrolled part, and stay put as
        // the tabs scroll. The strip rests with a tab's left edge at its left, or at its end with
        // the last tab against +: no empty room after it.
        for (theme, t) in both() {
            let mut h = seven(theme, STRIP, 2);
            let band = h.state().band;
            let new = Rect::from_min_size(pos2(band.right() - 64.0, band.top()), vec2(32.0, 36.0));
            let list = Rect::from_min_size(pos2(band.right() - 32.0, band.top()), vec2(32.0, 36.0));
            let pinned = |s: &Drawn, at: &str| {
                assert_eq!((s.new, s.list), (new, Some(list)), "{theme:?} {at}: + and the list button at the strip's right end");
            };
            pinned(h.state(), "at the start");
            assert_eq!(h.state().tabs[0].left(), band.left(), "{theme:?}: from the first tab's edge");
            let shapes = &h.output().shapes;
            assert_eq!(icon_in(shapes, new).0, [t.text_muted], "{theme:?}: + drawn");
            assert_eq!(icon_in(shapes, list).0, [t.text_muted], "{theme:?}: ⌄ drawn");
            for s in shapes {
                if let egui::Shape::Text(text) = &s.shape
                    && PAGE.iter().any(|(title, _)| *title == text.galley.text())
                {
                    assert!(s.clip_rect.right() <= new.left() + 0.5, "{theme:?}: `{}` cut at +: {:?}", text.galley.text(), s.clip_rect);
                }
            }
            // The last tab made active: the strip goes to its end, the last tab against +.
            h.state_mut().active = 6;
            h.run();
            pinned(h.state(), "at the end");
            assert_eq!(h.state().tabs[6].right(), new.left(), "{theme:?}: no empty room after the last tab");
            // The first again: back to its edge.
            h.state_mut().active = 0;
            h.run();
            pinned(h.state(), "back at the start");
            assert_eq!(h.state().tabs[0].left(), band.left(), "{theme:?}: the first tab at the strip's left end");
            // The wheel at 840, where the tabs overflow by 90: the next rest is the end.
            let notch = |h: &mut egui_kittest::Harness<'_, Drawn>| {
                h.hover_at(h.state().band.left_center() + vec2(40.0, 0.0));
                h.event(Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: vec2(-40.0, 0.0),
                    phase: egui::TouchPhase::Move,
                    modifiers: Modifiers::NONE,
                });
                h.run_steps(30);
            };
            notch(&mut h);
            pinned(h.state(), "after the wheel");
            assert_eq!(h.state().tabs[6].right(), new.left(), "{theme:?}: a notch rests at the end: {:?}", h.state().tabs);
            // At 560, a notch at a time: each comes to rest at the next tab's edge, then the end.
            let mut h = seven(theme, 560.0, 0);
            let (new, list) = (h.state().new, h.state().list);
            for k in 1..=2 {
                notch(&mut h);
                let s = h.state();
                assert_eq!((s.new, s.list), (new, list), "{theme:?}: notch {k}: + and the list stay put");
                assert_eq!(s.tabs[k].left(), s.band.left(), "{theme:?}: notch {k} rests at tab {k}'s edge: {:?}", s.tabs);
            }
            notch(&mut h);
            assert_eq!(h.state().tabs[6].right(), new.left(), "{theme:?}: the last notch rests at the end: {:?}", h.state().tabs);
        }
    }

    #[test]
    fn a_plain_wheel_scrolls_the_strip() {
        // An ordinary mouse's wheel turns up and down; over the strip it moves
        // the tabs sideways, rest by rest, as a sideways wheel or Shift and the wheel do.
        let mut h = seven(Theme::Light, 560.0, 0);
        h.hover_at(h.state().band.left_center() + vec2(40.0, 0.0));
        h.event(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, -40.0),
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::NONE,
        });
        h.run_steps(30);
        let s = h.state();
        assert_eq!(s.tabs[1].left(), s.band.left(), "a notch down rests at the next tab's edge: {:?}", s.tabs);
    }

    #[test]
    fn the_list_shows_every_tab_in_order_with_the_active_checked() {
        // Its list is a menu (`menu`, `menu/item`) under the list button, its right edge at the
        // strip's, 2 under the strip: every tab in strip order, its title as written, the active
        // one checked in the mark column.
        for (theme, t) in both() {
            let mut h = seven(theme, STRIP, 2);
            h.get_by_label("All tabs").click();
            h.run();
            h.run();
            let s = h.state();
            let order: Vec<usize> = s.items.iter().map(|(i, _)| *i).collect();
            assert_eq!(order, (0..7).collect::<Vec<_>>(), "{theme:?}: every tab, in strip order");
            let list = s.list.unwrap_or_else(|| panic!("{theme:?}: the list button"));
            let drawn = texts(&h.output().shapes);
            for (k, (i, item)) in s.items.iter().enumerate() {
                assert_eq!(item.height(), 28.0, "{theme:?}: item {i} 28 tall");
                assert!(
                    (item.right() - (list.right() - 5.0)).abs() <= 0.5,
                    "{theme:?}: item {i} {item:?} ends 4 inside the menu's edge at the strip's end"
                );
                assert!(
                    (item.top() - (s.band.bottom() + 2.0 + 5.0 + 28.0 * k as f32)).abs() <= 0.5,
                    "{theme:?}: item {i} {item:?} in order, 2 under the strip"
                );
                let title = PAGE[*i].0;
                let words = drawn.iter().find(|(w, r, _)| w == title && item.contains_rect(*r));
                let (_, _, colour) = words.unwrap_or_else(|| panic!("{theme:?}: `{title}` in item {i}: {drawn:?}"));
                assert_eq!(*colour, t.text, "{theme:?}: `{title}` as written, in text");
                let mark = Rect::from_min_size(pos2(item.left() + 8.0, item.top() + 7.0), vec2(14.0, 14.0));
                let want: Vec<Color32> = if *i == 2 { vec![t.text] } else { vec![] };
                assert_eq!(icon_in(&h.output().shapes, mark).0, want, "{theme:?}: item {i}'s mark");
            }
            assert_eq!(fills_in(&h.output().shapes, list).first(), Some(&(list, t.fill)), "{theme:?}: the button open, on fill");
        }
    }

    #[test]
    fn choosing_from_the_list_activates_and_reveals_the_tab() {
        let mut h = seven(Theme::Light, STRIP, 0);
        assert!(!whole_in_view(h.state(), 6), "dealers is cut at +: {:?}", h.state().tabs);
        h.get_by_label("All tabs").click();
        h.run();
        h.run();
        let items = h.state().items.clone();
        let dealers = items.iter().find(|(i, _)| *i == 6).map(|(_, r)| r.center());
        click_at(&mut h, dealers.unwrap_or_else(|| panic!("dealers listed: {items:?}")));
        h.run();
        let s = h.state();
        assert_eq!(s.asked, [(6, Asked::Activate)], "choosing dealers asks to make it active");
        assert_eq!(s.active, 6);
        assert!(whole_in_view(s, 6), "and it comes into view whole: {:?}", s.tabs);
        assert!(s.items.is_empty(), "the list closes");
    }

    #[test]
    fn choosing_from_the_list_by_keyboard_closes_it() {
        // Enter on the list button opens the list; the arrows and Enter choose
        // a tab, which becomes active, and the list closes, as after a click.
        let mut h = seven(Theme::Light, STRIP, 0);
        h.get_by_label("All tabs").focus();
        h.run();
        h.key_press(Key::Enter);
        h.run();
        h.run();
        assert!(!h.state().items.is_empty(), "Enter opens the list");
        for _ in 0..2 {
            h.key_press(Key::ArrowDown);
            h.run();
        }
        h.key_press(Key::Enter);
        h.run();
        h.run();
        let s = h.state();
        assert!(matches!(s.asked.as_slice(), [(i, Asked::Activate)] if *i == s.active && *i != 0), "a tab chosen: {:?}", s.asked);
        assert!(s.items.is_empty(), "the list closes: {:?}", s.items);
    }

    #[test]
    fn the_active_tab_is_always_brought_into_view() {
        // By keyboard: Tab goes through the tabs (each tab, then its close); a tab it reaches out
        // of view comes into view, and Enter makes it active.
        let mut h = seven(Theme::Light, 560.0, 0);
        assert!(!whole_in_view(h.state(), 6), "dealers out of view: {:?}", h.state().tabs);
        for _ in 0..13 {
            h.key_press(Key::Tab);
            h.run();
        }
        assert!(h.get_by_label("dealers").is_focused(), "Tab reaches dealers");
        assert!(whole_in_view(h.state(), 6), "the focused tab comes into view: {:?}", h.state().tabs);
        h.key_press(Key::Enter);
        h.run();
        assert_eq!(h.state().active, 6, "Enter makes it active");
        assert!(whole_in_view(h.state(), 6), "and it stays in view");
        // Made active elsewhere (the control channel, Ctrl+W on another): it comes into view.
        h.state_mut().active = 0;
        h.run();
        assert!(whole_in_view(h.state(), 0) && h.state().tabs[0].left() == h.state().band.left(), "the first tab back at the edge");
        h.state_mut().active = 3;
        h.run();
        assert!(whole_in_view(h.state(), 3), "line_items in view: {:?}", h.state().tabs);
        // Wheeled away from the active tab, it comes back into view when the
        // strip's width changes, and when the number of tabs does.
        h.state_mut().active = 0;
        h.run();
        let away = |h: &mut egui_kittest::Harness<'_, Drawn>| {
            for _ in 0..6 {
                h.hover_at(h.state().band.left_center() + vec2(40.0, 0.0));
                h.event(Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: vec2(-40.0, 0.0),
                    phase: egui::TouchPhase::Move,
                    modifiers: Modifiers::NONE,
                });
                h.run_steps(30);
            }
            assert!(!whole_in_view(h.state(), 0), "wheeled away from orders: {:?}", h.state().tabs);
        };
        away(&mut h);
        h.state_mut().width = 540.0;
        h.run();
        assert!(whole_in_view(h.state(), 0), "back in view after the width changed: {:?}", h.state().tabs);
        away(&mut h);
        h.state_mut().count = 6;
        h.run();
        assert!(whole_in_view(h.state(), 0), "back in view after a tab went: {:?}", h.state().tabs);
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
