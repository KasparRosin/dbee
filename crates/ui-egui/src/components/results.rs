//! The query tab's bars: the run row over the results (`run row`), and the result bar
//! (`result bar`) with a tab per result (`result tab`), what the shown result holds and how long
//! the run took, EXPORT and the value panel's toggle. And the row count (`rows/count`), which the
//! table bar holds.
//!
//! Each is a view: it takes what to show and says what was clicked; the screen tags what it drew
//! and acts. A result's name is data, drawn as written. The counts and how long a run took are
//! facts in `label` capitals, as the page writes them (a number has no case to keep); a running
//! timer is in `data`. All of them are read out as written.

use std::sync::Arc;
use std::time::Duration;

use egui::text::TextWrapping;
use egui::{Align, Color32, CornerRadius, Galley, Id, Layout, Rect, Response, Sense, Ui, UiBuilder, Widget, WidgetInfo, WidgetType, pos2, vec2};

use super::button::{BarButton, BoxButton, TextButton};
use super::icon::Icon;
use super::spinner::Spinner;
use super::status::{Piece, piece, timer};
use super::tab::{Placed, list_button, row};
use super::tooltip::Hint;
use super::{Look, focus, painter};
use crate::theme::size::{
    BAR_CELL_H, BAR_H, BAR_LINE, BUTTON_H, CHIP_RUN_GAP, NEW_TAB_W, RESULT_FACT_GAP, RESULT_TAB_PAD_X, RESULT_TAB_TITLE_MAX, ROW_COUNT_COUNTING_GAP,
    ROW_COUNT_GAP, RUN_CHIP_MIN_W, RUN_ROW_GAP, RUN_ROW_PAD_X, SPINNER, TAB_ACTIVE_LINE,
};
use crate::theme::{Text, Tokens, tokens};
use crate::tree::thousands;

/// What the result bar's list button is read out as.
const ALL_RESULTS: &str = "All results";

/// Why EXPORT is off, on its hover.
pub const EXPORT_LATER: &str = "Export this result: not now, export comes in a later version of dbee.";

/// A bar `ui`'s width wide, [`BAR_H`] tall: `card`, with its line along the bottom. Returns the
/// bar's response and the room above the line.
fn bar_ground(ui: &mut Ui) -> (Response, Rect) {
    let t = tokens(ui.ctx());
    let (bar, r) = ui.allocate_exact_size(vec2(ui.available_width(), BAR_H), Sense::hover());
    let p = ui.painter();
    p.rect_filled(bar, CornerRadius::ZERO, t.card);
    p.rect_filled(Rect::from_min_max(pos2(bar.left(), bar.bottom() - BAR_LINE), bar.max), CornerRadius::ZERO, t.line);
    (r, Rect::from_min_max(bar.min, pos2(bar.right(), bar.bottom() - BAR_LINE)))
}

/// The row of run buttons: what leads it (the database chip, from the screen), RUN, CANCEL while
/// asked, and RUN CURRENT when shown.
#[derive(Clone, Copy, Debug)]
pub struct RunRow {
    /// The tab's run is drawn running (the screen asks once it has gone 300 ms): RUN and RUN
    /// CURRENT are off.
    pub running: bool,
    /// CANCEL shows (the screen asks while its run is drawn running).
    pub cancel: bool,
    /// RUN CURRENT shows (as today, until it is decided).
    pub run_current: bool,
    /// A run can start (the engine's own check): RUN and RUN CURRENT are on while nothing runs.
    pub can_run: bool,
    /// How wide what leads the row is (the database chip), when something does.
    pub lead: Option<f32>,
}

/// What a click in the run row asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunAsked {
    Run,
    RunCurrent,
    Cancel,
}

/// The run row as drawn: each button, the form it took, and what a click asked for.
pub struct RunShown {
    pub run: Response,
    pub cancel: Option<Response>,
    pub run_current: Option<Response>,
    pub fit: RunFit,
    pub asked: Option<RunAsked>,
}

/// Which of the run row's parts show, short of room. They give way in this order (the first
/// first): RUN CURRENT's shortcut, RUN's shortcut (their hovers still name the keys), RUN CURRENT
/// (its keys still run the statement under the cursor), then the lead narrows to
/// [`RUN_CHIP_MIN_W`]. Past that the row is cut at its end; RUN and CANCEL never go.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RunFit {
    pub run_current_keys: bool,
    pub run_keys: bool,
    pub run_current: bool,
    /// How wide the lead is drawn (0 with none).
    pub lead_w: f32,
}

impl RunRow {
    /// Every form, from the widest to the narrowest: each gives way one step more than the one
    /// before it.
    fn forms(&self) -> [RunFit; 5] {
        let lead_w = self.lead.unwrap_or(0.0).max(0.0);
        let narrow = lead_w.min(RUN_CHIP_MIN_W);
        [
            RunFit { run_current_keys: true, run_keys: true, run_current: true, lead_w },
            RunFit { run_current_keys: false, run_keys: true, run_current: true, lead_w },
            RunFit { run_current_keys: false, run_keys: false, run_current: true, lead_w },
            RunFit { run_current_keys: false, run_keys: false, run_current: false, lead_w },
            RunFit { run_current_keys: false, run_keys: false, run_current: false, lead_w: narrow },
        ]
    }

    fn run_button(keys: bool) -> BoxButton<'static> {
        let b = BoxButton::primary("Run");
        if keys { b.shortcut("Ctrl+Enter") } else { b }
    }

    fn run_current_button(keys: bool) -> BoxButton<'static> {
        let b = BoxButton::outline("Run current");
        if keys { b.shortcut("Ctrl+Shift+Enter") } else { b }
    }

    /// How wide the row draws in form `fit`, its 12 at either end included.
    fn needs(&self, ui: &Ui, fit: RunFit) -> f32 {
        let lead = if fit.lead_w > 0.0 { fit.lead_w + CHIP_RUN_GAP } else { 0.0 };
        let cancel = if self.cancel { RUN_ROW_GAP + BoxButton::primary("Cancel").width(ui) } else { 0.0 };
        let current = if self.run_current && fit.run_current { RUN_ROW_GAP + Self::run_current_button(fit.run_current_keys).width(ui) } else { 0.0 };
        2.0 * RUN_ROW_PAD_X + lead + Self::run_button(fit.run_keys).width(ui) + cancel + current
    }

    /// The first form that fits a row `room` wide, each form taking room only from the steps
    /// before it; the narrowest when none does (the row then cut at its end).
    pub fn fit(&self, ui: &Ui, room: f32) -> RunFit {
        let forms = self.forms();
        forms.into_iter().find(|&fit| self.needs(ui, fit) <= room).unwrap_or(forms[4])
    }

    /// Draws it as wide as `ui`, [`BAR_H`] tall.
    pub fn show(self, ui: &mut Ui) -> RunShown {
        self.show_with(ui, |_, _| {})
    }

    /// Draws it as wide as `ui`, [`BAR_H`] tall, in the first form that fits ([`RunRow::fit`]),
    /// with what `lead` adds first in the row, as wide as it is given ([`RUN_ROW_GAP`] apart, as
    /// the buttons are), and RUN [`CHIP_RUN_GAP`] after it when it added something. Nothing is
    /// drawn past the row.
    pub fn show_with(self, ui: &mut Ui, lead: impl FnOnce(&mut Ui, f32)) -> RunShown {
        let (bar, cell) = bar_ground(ui);
        let fit = self.fit(ui, cell.width());
        let row = Rect::from_min_max(pos2(cell.left() + RUN_ROW_PAD_X, cell.top()), cell.max);
        let mut ui = ui.new_child(UiBuilder::new().id_salt(bar.id).max_rect(row).layout(Layout::left_to_right(Align::Center)));
        ui.set_clip_rect(ui.clip_rect().intersect(bar.rect));
        ui.spacing_mut().item_spacing.x = RUN_ROW_GAP;
        let before = ui.cursor().min.x;
        lead(&mut ui, fit.lead_w);
        if ui.cursor().min.x > before {
            ui.add_space(CHIP_RUN_GAP - RUN_ROW_GAP);
        }
        let on = self.can_run && !self.running;
        let run = ui.add_enabled(on, Self::run_button(fit.run_keys));
        let cancel = self.cancel.then(|| ui.add(BoxButton::primary("Cancel")));
        let run_current = (self.run_current && fit.run_current).then(|| ui.add_enabled(on, Self::run_current_button(fit.run_current_keys)));
        let clicked = |r: &Option<Response>| r.as_ref().is_some_and(Response::clicked);
        let asked = if run.clicked() {
            Some(RunAsked::Run)
        } else if clicked(&cancel) {
            Some(RunAsked::Cancel)
        } else if clicked(&run_current) {
            Some(RunAsked::RunCurrent)
        } else {
            None
        };
        RunShown { run, cancel, run_current, fit, asked }
    }
}

/// One result's tab in the result bar, [`BAR_CELL_H`] tall: its title as written, and a `line` at
/// its right end.
#[derive(Clone, Copy, Debug)]
pub struct ResultTab<'a> {
    title: &'a str,
    active: bool,
    look: Option<Look>,
}

impl<'a> ResultTab<'a> {
    /// A tab named `title`, as written.
    pub fn new(title: &'a str) -> ResultTab<'a> {
        ResultTab { title, active: false, look: None }
    }

    /// The result shown: `card`, the `ink` line along its top, its title in `text`.
    pub fn active(mut self, active: bool) -> ResultTab<'a> {
        self.active = active;
        self
    }

    /// Draws it with `look` whatever its response says (the showcase only).
    pub fn look(mut self, look: Look) -> ResultTab<'a> {
        self.look = Some(look);
        self
    }

    /// The title, cut with "…" at [`RESULT_TAB_TITLE_MAX`], and the tab's width: 12, the title's
    /// box (whole points, as the page's), 12, the line.
    fn parts(&self, ui: &Ui) -> (Arc<Galley>, f32) {
        let mut job = Text::Button.job_as_written(self.title, Color32::PLACEHOLDER);
        job.wrap = TextWrapping::truncate_at_width(RESULT_TAB_TITLE_MAX);
        let title = ui.painter().layout_job(job);
        let width = 2.0 * RESULT_TAB_PAD_X + title.size().x.ceil() + BAR_LINE;
        (title, width)
    }
}

impl Widget for ResultTab<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let (title, width) = self.parts(ui);
        let (rect, r) = ui.allocate_exact_size(vec2(width, BAR_CELL_H), Sense::click());
        r.widget_info(|| WidgetInfo::selected(WidgetType::SelectableLabel, ui.is_enabled(), self.active, self.title));
        let look = self.look.unwrap_or_else(|| Look::of(ui, &r));
        if !ui.is_rect_visible(rect) {
            return r;
        }
        let t = tokens(ui.ctx());
        let p = painter(ui);
        let ground = match (self.active, look.hovered) {
            (true, _) => t.card,
            (false, true) => t.hover,
            (false, false) => t.bg,
        };
        p.rect_filled(rect, CornerRadius::ZERO, ground);
        p.rect_filled(Rect::from_min_max(pos2(rect.right() - BAR_LINE, rect.top()), rect.max), CornerRadius::ZERO, t.line);
        if self.active {
            p.rect_filled(Rect::from_min_size(rect.min, vec2(rect.width(), TAB_ACTIVE_LINE)), CornerRadius::ZERO, t.ink);
        }
        let colour = if self.active { t.text } else { t.text_muted };
        p.galley(pos2(rect.left() + RESULT_TAB_PAD_X, rect.center().y - title.size().y / 2.0), title, colour);
        if look.focused {
            focus::box_inside(&p, rect);
        }
        r
    }
}

/// What became of the run, for the result bar's facts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunState {
    /// The shown result has `rows`; the run took `ms`, when known.
    Done {
        rows: u64,
        ms: Option<u64>,
    },
    /// The run goes, for `elapsed` so far (today's spinner while the app gives no timer).
    Running {
        elapsed: Option<Duration>,
    },
    /// The run failed (its error shows below the bar).
    Failed {
        ms: Option<u64>,
    },
    Cancelled {
        elapsed: Option<Duration>,
    },
    /// The shown statement changed `rows` rows and returned none; `None` when the database gave
    /// no count (the time alone is drawn).
    Affected {
        rows: Option<u64>,
        ms: Option<u64>,
    },
}

/// What a click in the result bar asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResultAsked {
    /// Show the result of this tab (0-based): a click on a tab that isn't the shown one, Enter or
    /// Space on it, or choosing it from the list.
    Show(usize),
    /// Open or close the value panel.
    Value,
}

/// The bar over the results.
#[derive(Clone, Debug)]
pub struct ResultBar<'a> {
    pub tabs: Vec<ResultTab<'a>>,
    pub state: RunState,
    /// A run's cap cut the result: the rows kept and the rows the server sent. Its words take the
    /// place of the rows' when the run is done.
    pub first_of: Option<(u64, u64)>,
    /// EXPORT shows (off until M4, its hover says why).
    pub export: bool,
    /// The value panel's toggle, on while the panel shows; `None` draws no Value cell, and the
    /// facts end 12 before EXPORT's divider, or the bar's end.
    pub value: Option<bool>,
    /// What hovering the time says (a statement whose count the database didn't give).
    pub time_hover: Option<&'a str>,
    /// The toggle drawn off: the value panel has no room beside the results.
    pub value_off: bool,
}

/// Which of the result bar's parts show, short of room. They give way in this order (the first
/// first): EXPORT's word (its icon stays), EXPORT (off until a later version, so nothing that
/// works goes), the facts' time, then the facts' words are cut with "…". The tabs scroll behind
/// the ⌄ list as soon as they don't all fit, every result still listed; the toggle never goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResultFit {
    pub export_word: bool,
    pub export: bool,
    pub time: bool,
    pub words_cut: bool,
}

impl ResultFit {
    /// Every form, from the widest to the narrowest: each gives way one step more than the one
    /// before it.
    pub const FORMS: [ResultFit; 5] = [
        ResultFit { export_word: true, export: true, time: true, words_cut: false },
        ResultFit { export_word: false, export: true, time: true, words_cut: false },
        ResultFit { export_word: false, export: false, time: true, words_cut: false },
        ResultFit { export_word: false, export: false, time: false, words_cut: false },
        ResultFit { export_word: false, export: false, time: false, words_cut: true },
    ];
}

/// The result bar as drawn: its tabs, its facts' words and their time, the facts as written
/// (`13 rows 24 ms`, before capitals), EXPORT and the toggle, the list button while its tabs don't
/// fit and its list's items in view while it is open (each with the index of the result it
/// lists), and what a click asked for.
pub struct ResultShown {
    pub tabs: Vec<Response>,
    pub facts: Response,
    pub time: Option<Response>,
    pub says: String,
    pub export: Option<Response>,
    pub value: Option<Response>,
    pub list: Option<Response>,
    pub listed: Vec<(usize, Response)>,
    pub fit: ResultFit,
    /// The facts' words were cut: hovering them should say them whole.
    pub cut: bool,
    pub asked: Option<ResultAsked>,
}

/// `n rows`, `1 row`.
fn rows_of(n: u64) -> String {
    format!("{} {}", thousands(n), if n == 1 { "row" } else { "rows" })
}

/// How long something took: `24 ms`, `3.2 s`, and past a minute as the timer reads.
fn took(d: Duration) -> String {
    match d.as_millis() {
        ms if ms < 1000 => format!("{ms} ms"),
        ms if ms < 60_000 => format!("{:.1} s", d.as_secs_f64()),
        _ => timer(d),
    }
}

/// A fact of a bar after its words: how long, or a spinner while that isn't known.
pub(super) enum Time {
    Words(Piece),
    Spinner,
}

impl Time {
    /// How long something took, in `label` capitals: `24 MS`.
    fn took(ui: &Ui, t: &Tokens, d: Duration) -> Time {
        let words = took(d);
        Time::Words(Piece::new(ui, Text::Label.job(&words, t.text_muted), words))
    }

    /// A running timer, in `data`: `00:02.4`.
    pub(super) fn timer(ui: &Ui, t: &Tokens, d: Duration) -> Time {
        let words = timer(d);
        Time::Words(Piece::new(ui, Text::Data.job_as_written(&words, t.text_muted), words))
    }

    pub(super) fn width(&self) -> f32 {
        match self {
            Time::Words(p) => p.width(),
            Time::Spinner => SPINNER,
        }
    }

    /// What it reads out as: its words, or nothing for the spinner.
    fn says(&self) -> Option<&str> {
        match self {
            Time::Words(p) => Some(p.says()),
            Time::Spinner => None,
        }
    }

    /// Draws it from `x`, centred on `mid`: its words' response.
    pub(super) fn show(self, ui: &Ui, id: Id, x: f32, mid: f32, t: &Tokens) -> Option<Response> {
        match self {
            Time::Words(p) => Some(piece(ui, id, &p, pos2(x, mid), t.text_muted)),
            Time::Spinner => {
                Spinner::new().paint(ui, pos2(x + SPINNER / 2.0, mid));
                None
            }
        }
    }
}

impl ResultBar<'_> {
    /// The facts: the words and their colour, and how long the run took or has gone.
    fn facts(&self, ui: &Ui, t: &Tokens) -> (String, Color32, Option<Time>) {
        let ms = |ms: Option<u64>| ms.map(|ms| Time::took(ui, t, Duration::from_millis(ms)));
        match self.state {
            RunState::Done { rows, ms: m } => {
                let words = match self.first_of {
                    Some((kept, seen)) => format!("First {} of {}", thousands(kept), rows_of(seen)),
                    None => rows_of(rows),
                };
                (words, t.text_muted, ms(m))
            }
            RunState::Running { elapsed } => ("Running".to_owned(), t.text_muted, Some(elapsed.map_or(Time::Spinner, |e| Time::timer(ui, t, e)))),
            RunState::Failed { ms: m } => ("Failed".to_owned(), t.danger, ms(m)),
            RunState::Cancelled { elapsed } => ("Cancelled".to_owned(), t.text_muted, elapsed.map(|e| Time::took(ui, t, e))),
            RunState::Affected { rows, ms: m } => (rows.map(|n| format!("{} affected", rows_of(n))).unwrap_or_default(), t.text_muted, ms(m)),
        }
    }

    /// EXPORT, with its word or without.
    fn export_button(word: bool) -> BarButton<'static> {
        BarButton::label(Icon::Export, "Export").over_line().words(word)
    }

    /// The first form ([`ResultFit::FORMS`]) that fits a bar `room` wide, each form taking room
    /// only from the steps before it; the narrowest when none does. The tabs need the room of
    /// all of them, or of the list button once they don't all fit.
    pub fn fit(&self, ui: &Ui, room: f32) -> ResultFit {
        let t = tokens(ui.ctx());
        let (words, colour, time) = self.facts(ui, t);
        let gap = if words.is_empty() { 0.0 } else { RESULT_FACT_GAP };
        let words_w = Piece::new(ui, Text::Label.job(&words, colour), words).width();
        let time_w = time.as_ref().map_or(0.0, |time| gap + time.width());
        let value_w = self.value.map_or(0.0, |on| BarButton::icon(Icon::Panel).on(on).over_line().width(ui));
        let tabs_w: f32 = self.tabs.iter().map(|tab| tab.parts(ui).1).sum();
        let tabs = (tabs_w + RESULT_FACT_GAP).min(NEW_TAB_W);
        let needs = |fit: ResultFit| {
            let export = if self.export && fit.export { Self::export_button(fit.export_word).width(ui) } else { 0.0 };
            tabs + words_w + if fit.time { time_w } else { 0.0 } + RESULT_FACT_GAP + export + value_w
        };
        ResultFit::FORMS[..4].iter().copied().find(|&fit| needs(fit) <= room).unwrap_or(ResultFit::FORMS[4])
    }

    /// Draws it as wide as `ui`, [`BAR_H`] tall, in the first form that fits
    /// ([`ResultBar::fit`]). While its tabs don't fit in what the facts leave them, they scroll
    /// as the workspace's tabs do (`row`), and the list button stands right after them and right
    /// before the facts, listing every result. Nothing is drawn past the bar.
    pub fn show(self, ui: &mut Ui) -> ResultShown {
        let t = tokens(ui.ctx());
        let (bar, cell) = bar_ground(ui);
        let id = bar.id;
        let mid = cell.center().y;
        let fit = self.fit(ui, cell.width());
        let mut ui = ui.new_child(UiBuilder::new().id_salt(id).max_rect(cell).layout(Layout::left_to_right(Align::Min)));
        let clip = ui.clip_rect().intersect(bar.rect);
        // Measured from the right end: the toggle, EXPORT, then the facts 12 in from them.
        let value = self.value.map(|on| BarButton::icon(Icon::Panel).on(on).over_line());
        let value_rect = value.as_ref().map(|b| Rect::from_min_max(pos2(cell.right() - b.width(&ui), cell.top()), cell.max));
        let value_left = value_rect.map_or(cell.right(), |r| r.left());
        // Everything but the toggle is cut where it starts: in a bar too narrow for the rest, the
        // toggle is what stays.
        ui.set_clip_rect(clip.intersect(Rect::from_min_max(bar.rect.min, pos2(value_left, bar.rect.bottom()))));
        let export = (self.export && fit.export).then(|| Self::export_button(fit.export_word));
        let export_rect = export.as_ref().map(|b| Rect::from_min_max(pos2(value_left - b.width(&ui), cell.top()), pos2(value_left, cell.bottom())));
        let (words_say, colour, time) = self.facts(&ui, t);
        let says = match time.as_ref().and_then(Time::says) {
            Some(time) if words_say.is_empty() => time.to_owned(),
            Some(time) => format!("{words_say} {time}"),
            None => words_say.clone(),
        };
        let mut time = time.filter(|_| fit.time);
        let mut job = Text::Label.job(&words_say, colour);
        let laid = Piece::new(&ui, job.clone(), words_say.clone());
        let facts_end = export_rect.map_or(value_left, |r| r.left()) - RESULT_FACT_GAP;
        // The time comes 12 after the words, or alone when there are none.
        let gap = if words_say.is_empty() { 0.0 } else { RESULT_FACT_GAP };
        // The tabs fit if they and the whole facts do, 12 apart; else the list button takes its
        // room before the facts.
        let widths: Vec<f32> = self.tabs.iter().map(|tab| tab.parts(&ui).1).collect();
        let whole = laid.width() + time.as_ref().map_or(0.0, |time| gap + time.width());
        let fits = widths.iter().sum::<f32>() + RESULT_FACT_GAP + whole <= facts_end - cell.left();
        let list_w = if fits { 0.0 } else { NEW_TAB_W };
        // Short of room the time goes, then the words are cut.
        let facts_room = (facts_end - cell.left() - list_w).max(0.0);
        if time.as_ref().is_some_and(|time| laid.width() + gap + time.width() > facts_room) {
            time = None;
        }
        let cut = laid.width() > facts_room;
        let words = if cut {
            job.wrap = TextWrapping::truncate_at_width(facts_room.max(1.0));
            Piece::new(&ui, job, words_say)
        } else {
            laid
        };
        let facts_w = words.width() + time.as_ref().map_or(0.0, |time| gap + time.width());
        // Never under the list button: what doesn't fit even cut is cut at the facts' end.
        let facts_left = (facts_end - facts_w).max(cell.left() + list_w);

        // Left to right, so Tab reaches them as they read: the tabs, kept left of the facts.
        let tabs_end = if fits { facts_left - RESULT_FACT_GAP } else { facts_left - NEW_TAB_W };
        let view = Rect::from_min_max(cell.min, pos2(tabs_end.max(cell.left()), cell.bottom()));
        let titles: Vec<&str> = self.tabs.iter().map(|tab| tab.title).collect();
        let shown = self.tabs.iter().position(|tab| tab.active);
        let tabs_id = id.with("tabs");
        let mut asked = None;
        let tabs = row(&mut ui, tabs_id, view, self.tabs, &widths, shown.map(|i| (i, tabs_id.with(i))), |ui, i, tab| {
            let active = tab.active;
            let r = ui.add(tab);
            if r.clicked() && !active {
                asked = Some(ResultAsked::Show(i));
            }
            Placed { focused: r.gained_focus(), item: r }
        })
        .tabs;
        let (list, listed) = if fits {
            (None, Vec::new())
        } else {
            let rect = Rect::from_min_size(pos2(view.right(), bar.rect.top()), vec2(NEW_TAB_W, BAR_H));
            let listed = list_button(&mut ui, id.with("list"), rect, ALL_RESULTS, &titles, shown, None);
            if let Some(i) = listed.chosen {
                asked = Some(ResultAsked::Show(i));
            }
            (Some(listed.button), listed.items)
        };
        let left_clip = ui.clip_rect();
        ui.set_clip_rect(left_clip.intersect(Rect::from_x_y_ranges(facts_left..=facts_end.max(facts_left), bar.rect.y_range())));
        let facts = piece(&ui, id.with("facts"), &words, pos2(facts_left, mid), colour);
        let time = time.and_then(|time| time.show(&ui, id.with("time"), facts_left + words.width() + gap, mid, t));
        ui.set_clip_rect(left_clip);
        let time = match (time, self.time_hover) {
            (Some(r), Some(hover)) => Some(r.hint(hover)),
            (time, _) => time,
        };
        let export = export.zip(export_rect).map(|(b, rect)| ui.add_enabled_ui(false, |ui| ui.put(rect, b)).inner.hint_disabled(EXPORT_LATER));
        ui.set_clip_rect(clip);
        let value = value.zip(value_rect).map(|(b, rect)| ui.add_enabled_ui(!self.value_off, |ui| ui.put(rect, b)).inner);
        if value.as_ref().is_some_and(Response::clicked) {
            asked = Some(ResultAsked::Value);
        }
        ResultShown { tabs, facts, time, says, export, value, list, listed, fit, cut, asked }
    }
}

/// How many rows there are (`rows/count`): known exactly, the planner's estimate with COUNT,
/// unknown with COUNT ROWS, or being counted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowCount {
    Exact(u64),
    /// The planner's estimate; one of 0 counts as unknown, so it never reads `~0`.
    Estimate(u64),
    Unknown,
    /// Counting, for `elapsed` so far, with the estimate shown meanwhile, if any. Before its
    /// timer shows (`None`, the first 300 ms) the words show alone. Its cancel is the status
    /// bar's job.
    Counting {
        estimate: Option<u64>,
        elapsed: Option<Duration>,
    },
}

/// The row count as drawn: its words (none with no rows shown and no count), its COUNT or COUNT
/// ROWS, COUNTING (with its timer, while counting), and whether COUNT was clicked.
pub struct CountShown {
    pub words: Option<Response>,
    pub count: Option<Response>,
    pub counting: Option<Response>,
    pub asked: bool,
}

/// A row count's parts, measured.
struct CountParts {
    words: Option<Piece>,
    button: Option<TextButton<'static>>,
    counting: Option<(Piece, Time)>,
    width: f32,
}

impl RowCount {
    /// The words: the rows shown (`1–100`) and how many there are (`of 25,000 rows`, `of ~100,000
    /// rows`), either alone when the other isn't there.
    pub fn words(self, shown: Option<(u64, u64)>) -> String {
        let total = match self {
            RowCount::Exact(n) => Some(thousands(n)),
            RowCount::Estimate(n) | RowCount::Counting { estimate: Some(n), .. } if n > 0 => Some(format!("~{}", thousands(n))),
            _ => None,
        };
        let n = match self {
            RowCount::Exact(n) | RowCount::Estimate(n) | RowCount::Counting { estimate: Some(n), .. } => n,
            _ => 0,
        };
        let rows = if n == 1 { "row" } else { "rows" };
        let range = shown.map(|(first, last)| format!("{}–{}", thousands(first), thousands(last)));
        match (range, total) {
            (Some(range), Some(total)) => format!("{range} of {total} {rows}"),
            (Some(range), None) => range,
            (None, Some(total)) => format!("{total} {rows}"),
            (None, None) => String::new(),
        }
    }

    /// Its parts: the words (unless `bare`), COUNT or COUNT ROWS, COUNTING with its time; and its
    /// width.
    fn parts(self, ui: &Ui, shown: Option<(u64, u64)>, bare: bool) -> CountParts {
        let t = tokens(ui.ctx());
        let says = if bare { String::new() } else { self.words(shown) };
        let words = (!says.is_empty()).then(|| Piece::new(ui, Text::Label.job(&says, t.text_muted), says));
        let button = match self {
            RowCount::Estimate(n) if n > 0 => Some(TextButton::new("Count")),
            RowCount::Estimate(_) | RowCount::Unknown => Some(TextButton::new("Count rows")),
            RowCount::Exact(_) | RowCount::Counting { .. } => None,
        };
        let counting = match self {
            RowCount::Counting { elapsed: Some(e), .. } => {
                Some((Piece::new(ui, Text::Label.job("Counting", t.text_muted), "Counting"), Time::timer(ui, t, e)))
            }
            _ => None,
        };
        // Each part after the words with the gap before it; none before the first.
        let words_w = words.as_ref().map_or(0.0, Piece::width);
        let any = words.is_some();
        let gap = |w: f32| if any { w } else { 0.0 };
        let button_w = button.as_ref().map_or(0.0, |b| gap(ROW_COUNT_GAP) + b.width(ui));
        let counting_w = counting.as_ref().map_or(0.0, |(c, time)| gap(ROW_COUNT_COUNTING_GAP) + c.width() + ROW_COUNT_COUNTING_GAP + time.width());
        CountParts { words, button, counting, width: (words_w + button_w + counting_w).ceil() }
    }

    /// How wide it draws in `ui` with the rows shown.
    pub fn width(self, ui: &Ui, shown: Option<(u64, u64)>) -> f32 {
        self.parts(ui, shown, false).width
    }

    /// How wide it draws in `ui` without its words: COUNT, COUNT ROWS or COUNTING alone, as a
    /// narrow bar keeps it; 0 when that leaves nothing.
    pub fn bare_width(self, ui: &Ui) -> f32 {
        self.parts(ui, None, true).width
    }

    /// Draws it where `ui` puts it next, as wide as what it holds and as tall as its button, with
    /// the rows shown (`1–100`), if any.
    pub fn show(self, ui: &mut Ui, shown: Option<(u64, u64)>) -> CountShown {
        self.show_parts(ui, shown, false)
    }

    /// [`RowCount::show`] without its words ([`RowCount::bare_width`]).
    pub fn show_bare(self, ui: &mut Ui) -> CountShown {
        self.show_parts(ui, None, true)
    }

    fn show_parts(self, ui: &mut Ui, shown: Option<(u64, u64)>, bare: bool) -> CountShown {
        let t = tokens(ui.ctx());
        let CountParts { words, button, counting, width } = self.parts(ui, shown, bare);
        let words_w = words.as_ref().map_or(0.0, Piece::width);
        let any = words.is_some();
        let gap = |w: f32| if any { w } else { 0.0 };
        let (rect, r) = ui.allocate_exact_size(vec2(width, BUTTON_H), Sense::hover());
        let id = r.id;
        let mid = rect.center().y;
        let words = words.map(|w| piece(ui, id.with("words"), &w, pos2(rect.left(), mid), t.text_muted));
        let mut x = rect.left() + words_w;
        let count = button.map(|b| {
            x += gap(ROW_COUNT_GAP);
            let w = b.width(ui);
            ui.put(Rect::from_min_size(pos2(x, rect.top()), vec2(w, BUTTON_H)), b)
        });
        let counting = counting.map(|(c, time)| {
            x += gap(ROW_COUNT_COUNTING_GAP);
            let r = piece(ui, id.with("counting"), &c, pos2(x, mid), t.text_muted);
            time.show(ui, id.with("timer"), x + c.width() + ROW_COUNT_COUNTING_GAP, mid, t);
            r
        });
        let asked = count.as_ref().is_some_and(Response::clicked);
        CountShown { words, count, counting, asked }
    }
}

#[cfg(test)]
mod tests {
    use egui::epaint::ClippedShape;
    use egui::{Color32, CornerRadius, Key, Theme, pos2, vec2};
    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::components::{Edge, Look, ROOM, both, edges_in, fills_in, harness, harness_in, icon_in, painted, painted_in, sections, texts};
    use crate::theme::Text;
    use crate::theme::size::{BAR_H, SPINNER};

    /// The page's bars are 840 wide; the harness holds this much, and [`harness`] this.
    const W: f32 = 760.0;
    const NARROW: f32 = 560.0;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= 0.6
    }

    /// One frame of `add` drawing a bar `W` wide: its shapes and where the bar lay.
    fn drawn(theme: Theme, mut add: impl FnMut(&mut Ui)) -> (Vec<ClippedShape>, Rect) {
        let mut at = Rect::NOTHING;
        let shapes = painted(theme, |ui| {
            ui.add_space(ROOM);
            ui.allocate_ui(vec2(W, BAR_H), |ui| {
                at = Rect::from_min_size(ui.cursor().min, vec2(W, BAR_H));
                add(ui);
            });
        });
        (shapes, at)
    }

    /// The text `words` drawn (a whole galley's text): where, and its colour.
    fn word(shapes: &[ClippedShape], words: &str) -> (Rect, Color32) {
        whole(shapes, words).unwrap_or_else(|| panic!("`{words}` drawn: {:?}", texts(shapes)))
    }

    fn whole(shapes: &[ClippedShape], words: &str) -> Option<(Rect, Color32)> {
        crate::components::flat(shapes).into_iter().find_map(|s| match s {
            egui::Shape::Text(t) if t.galley.text() == words => {
                let colour = texts(&[ClippedShape { clip_rect: Rect::EVERYTHING, shape: egui::Shape::Text(t.clone()) }])[0].2;
                Some((t.galley.rect.translate(t.pos.to_vec2()), colour))
            }
            _ => None,
        })
    }

    /// The bar's ground and its line: `card` over the whole bar, a 1 px `line` along its bottom.
    fn assert_ground(shapes: &[ClippedShape], at: Rect, t: &crate::theme::Tokens, at_: &str) {
        let fills = fills_in(shapes, at);
        assert_eq!(fills.first(), Some(&(at, t.card)), "{at_}: the bar on card");
        let line = Rect::from_min_max(pos2(at.left(), at.bottom() - 1.0), at.max);
        assert!(fills.contains(&(line, t.line)), "{at_}: the 1 px line along its bottom: {fills:?}");
    }

    #[test]
    fn the_run_row_matches_the_page() {
        // Node 164094: 36 tall on `card` with a 1 px `line` along its bottom; its buttons 28 tall,
        // 12 in from its left, 4 apart, centred in the 35 above the line. Idle: RUN, the main
        // button, with its shortcut. Running: RUN off, then CANCEL, the main button. RUN CURRENT,
        // an outline button with its shortcut, after them when shown, off while running. The
        // page's widths: RUN 122, CANCEL 67, RUN CURRENT 223.
        for (theme, t) in both() {
            for (running, run_current) in [(false, false), (true, false), (false, true), (true, true)] {
                let at_ = format!("{theme:?} running {running} run current {run_current}");
                let mut rects = (Rect::NOTHING, None, None);
                let (shapes, at) = drawn(theme, |ui| {
                    let s = RunRow { running, cancel: running, run_current, can_run: true, lead: None }.show(ui);
                    rects = (s.run.rect, s.cancel.map(|r| r.rect), s.run_current.map(|r| r.rect));
                });
                assert_ground(&shapes, at, &t, &at_);
                let (run, cancel, current) = rects;
                assert!(close(run.left(), at.left() + 12.0) && close(run.top(), at.top() + 3.5), "{at_}: RUN 12 in, centred: {run:?} in {at:?}");
                assert_eq!(run.height(), 28.0, "{at_}: 28 tall");
                assert!((run.width() - 122.0).abs() <= 1.0, "{at_}: RUN as wide as the page's: {run:?}");
                let (ground, words, keys) =
                    if running { (t.fill, t.text_faint, t.text_faint) } else { (t.ink, t.on_ink, t.on_ink.gamma_multiply(0.7)) };
                assert_eq!(fills_in(&shapes, run), [(run, ground)], "{at_}: RUN's ground");
                assert_eq!(word(&shapes, "RUN").1, words, "{at_}: RUN's words");
                assert_eq!(word(&shapes, "CTRL+ENTER").1, keys, "{at_}: RUN's shortcut");
                assert!(run.contains_rect(word(&shapes, "CTRL+ENTER").0), "{at_}: the shortcut in RUN");
                let mut next = run.right() + 4.0;
                match cancel {
                    Some(cancel) => {
                        assert!(running, "{at_}: CANCEL only while running");
                        assert!(
                            close(cancel.left(), next) && cancel.top() == run.top() && cancel.height() == 28.0,
                            "{at_}: CANCEL 4 after RUN: {cancel:?}"
                        );
                        assert!((cancel.width() - 67.0).abs() <= 1.0, "{at_}: CANCEL as wide as the page's: {cancel:?}");
                        assert_eq!(fills_in(&shapes, cancel), [(cancel, t.ink)], "{at_}: CANCEL the main button");
                        assert_eq!(word(&shapes, "CANCEL").1, t.on_ink, "{at_}: CANCEL's words");
                        next = cancel.right() + 4.0;
                    }
                    None => {
                        assert!(!running, "{at_}: CANCEL while running");
                        assert!(whole(&shapes, "CANCEL").is_none(), "{at_}: no CANCEL");
                    }
                }
                match current {
                    Some(current) => {
                        assert!(run_current, "{at_}: RUN CURRENT only when shown");
                        assert!(close(current.left(), next) && current.top() == run.top(), "{at_}: RUN CURRENT 4 after: {current:?}");
                        assert!((current.width() - 223.0).abs() <= 1.0, "{at_}: RUN CURRENT as wide as the page's: {current:?}");
                        let edge = Edge { outer: current, width: 1.0, colour: t.line_strong, radius: CornerRadius::same(2) };
                        assert_eq!(edges_in(&shapes, current, 0.0), [edge], "{at_}: RUN CURRENT's outline");
                        let words = if running { t.text_faint } else { t.text };
                        assert_eq!(word(&shapes, "RUN CURRENT").1, words, "{at_}: RUN CURRENT's words");
                        assert!(current.contains_rect(word(&shapes, "CTRL+SHIFT+ENTER").0), "{at_}: its shortcut");
                    }
                    None => {
                        assert!(!run_current, "{at_}: RUN CURRENT when shown");
                        assert!(whole(&shapes, "RUN CURRENT").is_none(), "{at_}: no RUN CURRENT");
                    }
                }
            }
        }
    }

    #[test]
    fn cancel_shows_only_when_asked() {
        // The screen asks for CANCEL once its run has gone 300 ms; before that RUN looks as idle.
        for (theme, t) in both() {
            let mut rects = (Rect::NOTHING, None);
            let (shapes, _) = drawn(theme, |ui| {
                let s = RunRow { running: false, cancel: false, run_current: true, can_run: true, lead: None }.show(ui);
                rects = (s.run.rect, s.cancel.map(|r| r.rect));
            });
            assert!(rects.1.is_none() && whole(&shapes, "CANCEL").is_none(), "{theme:?}: no CANCEL unless asked");
            assert_eq!(fills_in(&shapes, rects.0), [(rects.0, t.ink)], "{theme:?}: RUN on");
            let (shapes, _) = drawn(theme, |ui| {
                let s = RunRow { running: true, cancel: true, run_current: true, can_run: true, lead: None }.show(ui);
                rects = (s.run.rect, s.cancel.map(|r| r.rect));
            });
            assert_eq!(fills_in(&shapes, rects.0), [(rects.0, t.fill)], "{theme:?}: RUN drawn off while running");
            assert_eq!(word(&shapes, "RUN").1, t.text_faint, "{theme:?}: RUN's words off");
            let cancel = rects.1.unwrap_or_else(|| panic!("{theme:?}: CANCEL when asked"));
            assert_eq!(fills_in(&shapes, cancel), [(cancel, t.ink)], "{theme:?}: CANCEL in ink");
        }
    }

    #[test]
    fn a_lead_comes_first_and_run_8_after_it() {
        for (theme, _) in both() {
            let mut rects = (Rect::NOTHING, Rect::NOTHING);
            let (_, at) = drawn(theme, |ui| {
                let s = RunRow { running: false, cancel: false, run_current: true, can_run: true, lead: Some(150.0) }.show_with(ui, |ui, _| {
                    rects.0 = ui.allocate_exact_size(vec2(150.0, 28.0), Sense::hover()).0;
                });
                rects.1 = s.run.rect;
            });
            let (lead, run) = rects;
            assert!(close(lead.left(), at.left() + 12.0), "{theme:?}: the lead 12 in: {lead:?}");
            assert!(close(run.left(), lead.right() + 8.0), "{theme:?}: RUN 8 after the lead: {run:?}");
        }
    }

    #[test]
    fn each_result_tab_state_matches_the_page() {
        // Node 164099: 35 tall (the bar's line under it), its title in `button` as written 12 in
        // from either end, then a 1 px `line` at its right end; 119 wide for `select ... series`.
        // Inactive on `bg`, its title `text_muted`; hover on `hover`; active on `card`, the title
        // in `text`, a 2 px `ink` line along its top over the whole width; focus on `bg` with a
        // 2 px `ink` box 2 px inside.
        let hovered = Look { hovered: true, ..Look::default() };
        let focused = Look { focused: true, ..Look::default() };
        for (theme, t) in both() {
            for state in ["inactive", "hover", "active", "focus"] {
                let at_ = format!("{theme:?} {state}");
                let mut at = Rect::NOTHING;
                let shapes = painted(theme, |ui| {
                    ui.add_space(ROOM);
                    let tab = ResultTab::new("select ... series");
                    let tab = match state {
                        "hover" => tab.look(hovered),
                        "active" => tab.active(true),
                        "focus" => tab.look(focused),
                        _ => tab,
                    };
                    at = ui.add(tab).rect;
                });
                assert_eq!(at.height(), 35.0, "{at_}: 35 tall");
                assert!((at.width() - 119.0).abs() <= 1.5, "{at_}: as wide as the page's: {at:?}");
                let (title, colour) = word(&shapes, "select ... series");
                assert_eq!(colour, if state == "active" { t.text } else { t.text_muted }, "{at_}: the title's colour");
                assert!(close(title.left(), at.left() + 12.0) && close(title.center().y, at.top() + 17.5), "{at_}: the title {title:?} in {at:?}");
                assert_eq!(at.width(), 12.0 + title.width().ceil() + 12.0 + 1.0, "{at_}: 12, the title's box, 12, the line");
                let ground = match state {
                    "hover" => t.hover,
                    "active" => t.card,
                    _ => t.bg,
                };
                let mut fills = vec![(at, ground), (Rect::from_min_max(pos2(at.right() - 1.0, at.top()), at.max), t.line)];
                if state == "active" {
                    fills.push((Rect::from_min_size(at.min, vec2(at.width(), 2.0)), t.ink));
                }
                assert_eq!(fills_in(&shapes, at), fills, "{at_}: the ground, the line, the active line");
                let focus = Edge { outer: at.shrink(2.0), width: 2.0, colour: t.ink, radius: CornerRadius::same(2) };
                let edges = if state == "focus" { vec![focus] } else { vec![] };
                assert_eq!(edges_in(&shapes, at, 0.0), edges, "{at_}: the focus box");
            }
        }
    }

    /// A result bar of `state` with the page's two tabs (`select ... orders` shown), or none.
    fn bar(tabs: bool, state: RunState, first_of: Option<(u64, u64)>, export: bool, value_on: bool) -> ResultBar<'static> {
        let tabs = if tabs { vec![ResultTab::new("select ... orders").active(true), ResultTab::new("select ... series")] } else { Vec::new() };
        ResultBar { tabs, state, first_of, export, value: Some(value_on), time_hover: None, value_off: false }
    }

    #[test]
    fn each_result_bar_state_matches_the_page() {
        // Node 164136 and the page's rows of it: 36 tall on `card` with a 1 px `line` along its
        // bottom. From the left the result tabs, side by side; at the right end the value panel's
        // toggle (a bar button, 36 wide), EXPORT before it (off), each with its divider and 35
        // tall; before them, 12 in, the facts 12 apart, centred above the line: what the result
        // holds or what became of the run in `label` capitals, numbers and all, and how long it
        // took in `label` too (a running timer in `data`), all `text_muted` but FAILED in `danger`.
        use Text::{Data, Label};
        let s = Duration::from_millis;
        for (theme, t) in both() {
            let m = t.text_muted;
            let rows: Vec<(&str, ResultBar<'static>, (&str, Color32), Option<(&str, Text)>)> = vec![
                ("done", bar(true, RunState::Done { rows: 10, ms: Some(24) }, None, true, false), ("10 ROWS", m), Some(("24 MS", Label))),
                ("running", bar(true, RunState::Running { elapsed: Some(s(2400)) }, None, true, false), ("RUNNING", m), Some(("00:02.4", Data))),
                ("failed", bar(false, RunState::Failed { ms: Some(12) }, None, false, false), ("FAILED", t.danger), Some(("12 MS", Label))),
                (
                    "cancelled",
                    bar(false, RunState::Cancelled { elapsed: Some(s(3200)) }, None, false, false),
                    ("CANCELLED", m),
                    Some(("3.2 S", Label)),
                ),
                (
                    "affected",
                    bar(true, RunState::Affected { rows: Some(2), ms: Some(5) }, None, false, false),
                    ("2 ROWS AFFECTED", m),
                    Some(("5 MS", Label)),
                ),
                (
                    "first of",
                    bar(true, RunState::Done { rows: 10_000, ms: Some(80) }, Some((10_000, 25_000)), true, false),
                    ("FIRST 10,000 OF 25,000 ROWS", m),
                    Some(("80 MS", Label)),
                ),
                ("value on", bar(true, RunState::Done { rows: 10, ms: Some(24) }, None, true, true), ("10 ROWS", m), Some(("24 MS", Label))),
            ];
            for (name, result, (want, colour), time) in rows {
                let at_ = format!("{theme:?} {name}");
                let (tabs, export, value_on) = (result.tabs.len(), result.export, result.value == Some(true));
                let mut seen = (Vec::new(), None, Rect::NOTHING);
                let (shapes, at) = drawn(theme, |ui| {
                    let s = result.clone().show(ui);
                    seen = (s.tabs.iter().map(|r| r.rect).collect(), s.export.map(|r| r.rect), s.value.map_or(Rect::NOTHING, |r| r.rect));
                });
                assert_ground(&shapes, at, &t, &at_);
                let (tab_rects, export_rect, value) = seen;
                let cell = Rect::from_min_max(at.min, pos2(at.right(), at.bottom() - 1.0));
                // The toggle at the right end, then EXPORT.
                assert_eq!(value, Rect::from_min_max(pos2(cell.right() - 36.0, cell.top()), cell.max), "{at_}: the toggle");
                let fills = fills_in(&shapes, value);
                assert!(fills.contains(&(Rect::from_min_size(value.min, vec2(1.0, 35.0)), t.line)), "{at_}: the toggle's divider: {fills:?}");
                assert_eq!(fills.contains(&(value, t.fill)), value_on, "{at_}: the toggle on `fill` when the panel is open");
                assert_eq!(icon_in(&shapes, value).0, [t.text], "{at_}: the toggle's icon");
                let end = match export_rect {
                    Some(e) => {
                        assert!(export, "{at_}: EXPORT when asked");
                        assert!(e.right() == value.left() && e.top() == cell.top() && e.height() == 35.0, "{at_}: EXPORT before the toggle: {e:?}");
                        assert!((e.width() - 83.0).abs() <= 1.0, "{at_}: EXPORT as wide as the page's: {e:?}");
                        assert_eq!(word(&shapes, "EXPORT").1, t.text_faint, "{at_}: EXPORT off");
                        assert!(fills_in(&shapes, e).contains(&(Rect::from_min_size(e.min, vec2(1.0, 35.0)), t.line)), "{at_}: EXPORT's divider");
                        e.left()
                    }
                    None => {
                        assert!(!export, "{at_}: EXPORT when asked");
                        assert!(whole(&shapes, "EXPORT").is_none(), "{at_}: no EXPORT");
                        value.left()
                    }
                };
                // The facts: the words in `label` capitals, numbers and all, as the page writes them.
                let (words, _) = word(&shapes, want);
                assert_eq!(sections(&shapes, want), [(want.to_owned(), Label.font(), colour)], "{at_}: the words in label");
                let mid = cell.center().y;
                assert!(close(words.center().y, mid), "{at_}: the words centred above the line: {words:?}");
                // egui puts the letter spacing after a caps text's last letter too.
                let trail = |style: Text| if style == Label { Label.spacing() } else { 0.0 };
                let last = match time {
                    Some((time, style)) => {
                        assert_eq!(sections(&shapes, time), [(time.to_owned(), style.font(), m)], "{at_}: the time in {style:?}");
                        let (at_time, _) = word(&shapes, time);
                        assert!(close(at_time.left() - (words.right() - trail(Label)), 12.0), "{at_}: 12 between the facts: {words:?} {at_time:?}");
                        assert!(close(at_time.center().y, mid), "{at_}: the time centred: {at_time:?}");
                        at_time.right() - trail(style)
                    }
                    None => words.right() - trail(Label),
                };
                assert!(close(end - last, 12.0), "{at_}: the facts end 12 before {end}: {last}");
                // The tabs, from the left end.
                assert_eq!(tab_rects.len(), tabs, "{at_}: its tabs");
                let mut x = cell.left();
                for (i, r) in tab_rects.iter().enumerate() {
                    assert!(r.left() == x && r.top() == cell.top() && r.height() == 35.0, "{at_}: tab {i} side by side from the left: {r:?}");
                    x = r.right();
                }
                if tabs > 0 {
                    let ink = (Rect::from_min_size(tab_rects[0].min, vec2(tab_rects[0].width(), 2.0)), t.ink);
                    assert!(fills_in(&shapes, tab_rects[0]).contains(&ink), "{at_}: the shown result's tab is the active one");
                    assert_eq!(word(&shapes, "select ... orders").1, t.text, "{at_}: its title");
                    assert_eq!(word(&shapes, "select ... series").1, m, "{at_}: the other's");
                } else {
                    assert!(whole(&shapes, "select ... orders").is_none(), "{at_}: no tabs");
                }
            }
        }
    }

    #[test]
    fn facts_read_as_drawn() {
        // The facts in `label` capitals, `text_muted` but FAILED in `danger`; a statement whose
        // count the database didn't give draws its time alone (its hover says why); a cut result's
        // words are grey like any fact. With no Value cell EXPORT ends at the bar's end. The facts
        // read out as written, before capitals.
        let t = crate::theme::LIGHT;
        let m = t.text_muted;
        let rows: Vec<(RunState, Option<(u64, u64)>, Vec<(&str, Color32)>, &str)> = vec![
            (RunState::Done { rows: 13, ms: Some(24) }, None, vec![("13 ROWS", m), ("24 MS", m)], "13 rows 24 ms"),
            (RunState::Failed { ms: Some(12) }, None, vec![("FAILED", t.danger), ("12 MS", m)], "Failed 12 ms"),
            (RunState::Affected { rows: None, ms: Some(5) }, None, vec![("5 MS", m)], "5 ms"),
            (RunState::Affected { rows: Some(2), ms: Some(5) }, None, vec![("2 ROWS AFFECTED", m), ("5 MS", m)], "2 rows affected 5 ms"),
            (
                RunState::Done { rows: 10_000, ms: Some(80) },
                Some((10_000, 25_000)),
                vec![("FIRST 10,000 OF 25,000 ROWS", m), ("80 MS", m)],
                "First 10,000 of 25,000 rows 80 ms",
            ),
        ];
        for (state, first_of, want, says) in rows {
            let mut seen = None;
            let (shapes, at) = drawn(Theme::Light, |ui| {
                let s =
                    ResultBar { tabs: Vec::new(), state, first_of, export: true, value: None, time_hover: Some("why"), value_off: false }.show(ui);
                seen = Some((s.says.clone(), s.export.map(|r| r.rect), s.value.is_some(), s.time.map(|r| r.rect)));
            });
            let (said, export, value, time) = seen.expect("drawn");
            let drawn: Vec<String> = texts(&shapes).into_iter().map(|(w, _, _)| w).filter(|w| w != "EXPORT").collect();
            let wanted: Vec<String> = want.iter().map(|(w, _)| (*w).to_owned()).collect();
            assert_eq!(drawn, wanted, "{state:?}: the facts drawn");
            for (w, colour) in &want {
                assert_eq!(word(&shapes, w).1, *colour, "{state:?}: `{w}`'s colour");
            }
            assert_eq!(said, says, "{state:?}: read out as written");
            assert!(!value, "{state:?}: no Value cell");
            assert!(!icon_in(&shapes, at).0.contains(&t.text), "{state:?}: no panel icon painted");
            let export = export.unwrap_or_else(|| panic!("{state:?}: EXPORT"));
            assert!(close(export.right(), at.right()), "{state:?}: EXPORT ends at the bar's end: {export:?} {at:?}");
            assert!(time.is_some(), "{state:?}: the time's response");
        }
    }

    #[test]
    fn each_row_count_matches_the_page() {
        // Node 164223: 28 tall; the rows shown and how many there are in `label` capitals,
        // `text_muted`, numbers and all; an estimate with COUNT and no count with
        // COUNT ROWS (`button/text`), 8 after the words; counting: 12, COUNTING in `label`, 12,
        // the timer in `data`. The page's COUNT is 56 wide, COUNT ROWS 92.
        use Text::{Data, Label};
        let s = Duration::from_millis;
        for (theme, t) in both() {
            let m = t.text_muted;
            let rows: Vec<(RowCount, &str, Option<(&str, f32)>, Option<&str>)> = vec![
                (RowCount::Exact(25_000), "1–100 OF 25,000 ROWS", None, None),
                (RowCount::Estimate(100_000), "1–100 OF ~100,000 ROWS", Some(("COUNT", 56.0)), None),
                (RowCount::Unknown, "1–100", Some(("COUNT ROWS", 92.0)), None),
                (RowCount::Counting { estimate: Some(100_000), elapsed: Some(s(1200)) }, "1–100 OF ~100,000 ROWS", None, Some("00:01.2")),
            ];
            for (count, want, button, timer) in rows {
                let at_ = format!("{theme:?} {count:?}");
                let mut seen = (Rect::NOTHING, None, None);
                let shapes = painted(theme, |ui| {
                    ui.add_space(ROOM);
                    ui.horizontal(|ui| {
                        let start = ui.cursor().min;
                        let s = count.show(ui, Some((1, 100)));
                        seen = (Rect::from_min_max(start, ui.min_rect().max), s.words.map(|r| r.rect), s.count.map(|r| r.rect));
                    });
                });
                let (at, words_r, count_r) = seen;
                assert_eq!(at.height(), 28.0, "{at_}: 28 tall");
                let (words, _) = word(&shapes, want);
                assert_eq!(sections(&shapes, want), [(want.to_owned(), Label.font(), m)], "{at_}: the words in label");
                assert!(close(words.left(), at.left()) && close(words.center().y, at.center().y), "{at_}: the words first, centred: {words:?}");
                assert!(words_r.is_some_and(|r| r.contains_rect(words.shrink(0.5))), "{at_}: the words' response: {words_r:?}");
                let trail = Label.spacing();
                let mut end = words.right() - trail;
                match button {
                    Some((label, width)) => {
                        let b = count_r.unwrap_or_else(|| panic!("{at_}: {label}"));
                        assert!(close(b.left() - end, 8.0) && b.height() == 28.0 && b.top() == at.top(), "{at_}: {label} 8 after the words: {b:?}");
                        assert!((b.width() - width).abs() <= 1.0, "{at_}: {label} as wide as the page's: {b:?}");
                        let (w, c) = word(&shapes, label);
                        assert!(c == t.text && close(w.left(), b.left() + 10.0), "{at_}: {label}'s words: {w:?} {c:?}");
                        end = b.right();
                    }
                    None => assert!(count_r.is_none(), "{at_}: no button: {count_r:?}"),
                }
                if let Some(timer) = timer {
                    let (counting, c) = word(&shapes, "COUNTING");
                    assert!(c == m && close(counting.left() - end, 12.0), "{at_}: COUNTING 12 after the words: {counting:?}");
                    assert_eq!(sections(&shapes, timer), [(timer.to_owned(), Data.font(), m)], "{at_}: the timer in data");
                    let (at_timer, _) = word(&shapes, timer);
                    assert!(close(at_timer.left() - (counting.right() - Label.spacing()), 12.0), "{at_}: the timer 12 after: {at_timer:?}");
                    end = at_timer.right();
                }
                assert!(close(at.right(), end.ceil()) || close(at.right(), end), "{at_}: as wide as what it holds: {at:?} {end}");
            }
        }
    }

    #[test]
    fn numbers_and_names_keep_their_case() {
        // A result's name is drawn and read out as written. The facts are drawn in `label`
        // capitals as the page writes them (a number has no case to keep) and read out as written.
        let mut h = harness(Theme::Light, (), |ui, _| {
            ui.allocate_ui(vec2(NARROW, BAR_H), |ui| {
                let tabs = vec![ResultTab::new("select … Orders").active(true), ResultTab::new("select … Line_Items")];
                ResultBar {
                    tabs,
                    state: RunState::Done { rows: 10_000, ms: Some(80) },
                    first_of: Some((10_000, 25_000)),
                    export: true,
                    value: Some(false),
                    time_hover: None,
                    value_off: false,
                }
                .show(ui);
            });
            ui.horizontal(|ui| RowCount::Estimate(25_000).show(ui, Some((1, 100))));
        });
        h.run();
        let drawn: Vec<String> = texts(&h.output().shapes).into_iter().map(|(w, _, _)| w).collect();
        for name in ["select … Orders", "select … Line_Items"] {
            assert!(drawn.iter().any(|w| w == name), "`{name}` drawn as written: {drawn:?}");
            assert!(!drawn.iter().any(|w| *w == name.to_uppercase()), "never in capitals: {drawn:?}");
            assert!(h.query_by_label(name).is_some(), "`{name}` read out as written");
        }
        for (says, caps) in
            [("First 10,000 of 25,000 rows", "FIRST 10,000 OF 25,000 ROWS"), ("1–100 of ~25,000 rows", "1–100 OF ~25,000 ROWS"), ("80 ms", "80 MS")]
        {
            assert!(drawn.iter().any(|w| w == caps), "`{caps}` drawn: {drawn:?}");
            assert!(h.query_by_label(says).is_some(), "`{says}` read out as written");
            assert!(h.query_by_label(caps).is_none(), "the capitals are only drawn");
        }
    }

    #[test]
    fn timings_read_as_the_page_writes_them() {
        // How long a run took: read out as written (`24 ms`), drawn in `label` capitals
        // (`24 MS`), past a minute as the timer reads; a running timer in `data`.
        let ms = Duration::from_millis;
        assert_eq!(took(ms(24)), "24 ms");
        assert_eq!(took(ms(999)), "999 ms");
        assert_eq!(took(ms(3200)), "3.2 s");
        assert_eq!(took(ms(65_300)), "01:05.3", "past a minute, as the timer reads");
        assert_eq!(rows_of(1), "1 row");
        assert_eq!(RowCount::Exact(1).words(Some((1, 1))), "1–1 of 1 row");
        assert_eq!(RowCount::Estimate(0).words(Some((1, 100))), "1–100", "never ~0");
        assert_eq!(RowCount::Exact(0).words(None), "0 rows");
        assert_eq!(RowCount::Unknown.words(None), "");
        for (state, caps, style) in [
            (RunState::Done { rows: 3, ms: Some(24) }, "24 MS", Text::Label),
            (RunState::Done { rows: 3, ms: Some(65_300) }, "01:05.3", Text::Label),
            (RunState::Cancelled { elapsed: Some(ms(3200)) }, "3.2 S", Text::Label),
            (RunState::Running { elapsed: Some(ms(2400)) }, "00:02.4", Text::Data),
        ] {
            let (shapes, _) = drawn(Theme::Light, |ui| {
                ResultBar { tabs: Vec::new(), state, first_of: None, export: false, value: Some(false), time_hover: None, value_off: false }.show(ui);
            });
            assert_eq!(sections(&shapes, caps), [(caps.to_owned(), style.font(), crate::theme::LIGHT.text_muted)], "{state:?}");
        }
    }

    #[test]
    fn with_no_timer_running_shows_todays_spinner_and_counting_its_words_alone() {
        // The query tab gives no timer yet: RUNNING shows today's spinner where the timer goes.
        // A count shows nothing of its own for its first 300 ms (no timer yet): the words alone,
        // no COUNTING and no spinner. An estimate of 0 asks to COUNT ROWS.
        let (shapes, at) = drawn(Theme::Light, |ui| {
            ResultBar {
                tabs: Vec::new(),
                state: RunState::Running { elapsed: None },
                first_of: None,
                export: false,
                value: Some(false),
                time_hover: None,
                value_off: false,
            }
            .show(ui);
        });
        let (running, _) = word(&shapes, "RUNNING");
        let slot =
            Rect::from_min_max(pos2(running.right() - Text::Label.spacing() + 12.0, at.top()), pos2(running.right() + 12.0 + SPINNER, at.bottom()));
        assert_eq!(icon_in(&shapes, slot.expand(1.0)).0, [crate::theme::LIGHT.text_muted], "the spinner 12 after RUNNING: {slot:?}");
        let shapes = painted(Theme::Light, |ui| {
            ui.horizontal(|ui| {
                RowCount::Counting { estimate: Some(100_000), elapsed: None }.show(ui, Some((1, 100)));
                RowCount::Estimate(0).show(ui, Some((1, 100)));
            });
        });
        let (words, _) = whole(&shapes, "1–100 OF ~100,000 ROWS").expect("the words");
        assert!(whole(&shapes, "COUNTING").is_none(), "no COUNTING before the timer");
        let after = Rect::from_min_max(pos2(words.right(), words.top() - 6.0), pos2(words.right() + 12.0 + SPINNER + 4.0, words.bottom() + 6.0));
        assert!(icon_in(&shapes, after).0.is_empty(), "no spinner after the words");
        assert!(whole(&shapes, "COUNT ROWS").is_some() && whole(&shapes, "COUNT").is_none(), "an estimate of 0 is no estimate");
    }

    #[test]
    fn a_narrow_result_bar_draws_nothing_over_another() {
        // The tabs take what the facts leave and are cut there; short of room EXPORT goes, then
        // the time, then the words are cut; the toggle stays.
        let tabs: Vec<ResultTab<'static>> =
            (0..6).map(|i| ResultTab::new(["select ... orders", "select ... series"][i % 2]).active(i == 0)).collect();
        let result = ResultBar {
            tabs,
            state: RunState::Done { rows: 10_000, ms: Some(80) },
            first_of: Some((10_000, 25_000)),
            export: true,
            value: Some(false),
            time_hover: None,
            value_off: false,
        };
        for width in [760.0, 560.0, 420.0, 300.0, 200.0] {
            let mut seen = (Vec::new(), None, Rect::NOTHING, Rect::NOTHING, Rect::NOTHING);
            let shapes = painted(Theme::Light, |ui| {
                ui.allocate_ui(vec2(width, BAR_H), |ui| {
                    let at = Rect::from_min_size(ui.cursor().min, vec2(width, BAR_H));
                    let s = result.clone().show(ui);
                    seen = (
                        s.tabs.iter().map(|r| r.interact_rect).collect(),
                        s.export.map(|r| r.rect),
                        s.value.map_or(Rect::NOTHING, |r| r.rect),
                        s.facts.rect,
                        at,
                    );
                });
            });
            let (tabs, export, value, facts, at) = seen;
            // EXPORT gives way first; the facts end before it, or before the toggle.
            let export = export.unwrap_or(Rect::from_min_max(pos2(value.left(), value.top()), pos2(value.left(), value.bottom())));
            assert!(value.right() <= at.right() + 0.5 && export.right() <= value.left(), "{width}: EXPORT, then the toggle at the end");
            let drawn: Vec<(String, Rect)> = texts(&shapes).into_iter().map(|(w, r, _)| (w, r)).collect();
            let facts_drawn: Vec<&(String, Rect)> = drawn.iter().filter(|(w, _)| !w.starts_with("select") && !w.contains("EXPORT")).collect();
            assert!(!facts_drawn.is_empty(), "{width}: the facts drawn");
            for (w, r) in &facts_drawn {
                assert!(r.left() >= at.left() - 0.5 && r.right() <= export.left() + 0.5, "{width}: `{w}` {r:?} between the bar's start and EXPORT");
            }
            if let Some((_, time)) = drawn.iter().find(|(w, _)| w == "80 MS") {
                assert!(time.right() - Text::Label.spacing() <= export.left() - 11.5, "{width}: the time 12 before EXPORT: {time:?}");
            }
            for (i, tab) in tabs.iter().enumerate() {
                assert!(tab.width() <= 0.0 || tab.right() <= facts.left() - 11.5, "{width}: tab {i} {tab:?} cut 12 before the facts {facts:?}");
            }
            // What the tabs paint is cut there too.
            for s in &shapes {
                if let egui::Shape::Text(t) = &s.shape
                    && t.galley.text().starts_with("select")
                {
                    let clip = s.clip_rect;
                    assert!(clip.width() <= 0.0 || clip.right() <= facts.left() - 11.5, "{width}: a tab's title cut before the facts: {clip:?}");
                }
            }
        }
    }

    /// The widths the give-way sweeps draw a bar at, from wide to narrow.
    fn sweep() -> impl Iterator<Item = f32> {
        (0..=39).map(|i| 900.0 - 20.0 * i as f32)
    }

    /// No two of `rects` overlap (by more than half a point).
    fn assert_apart(rects: &[(&str, Rect)], at_: &str) {
        for (i, (a, ra)) in rects.iter().enumerate() {
            for (b, rb) in &rects[i + 1..] {
                let o = ra.intersect(*rb);
                assert!(!o.is_positive() || o.area() <= 0.5, "{at_}: `{a}` {ra:?} over `{b}` {rb:?}");
            }
        }
    }

    /// Nothing `shapes` paints reaches past `bar`.
    fn assert_within(shapes: &[ClippedShape], bar: Rect, at_: &str) {
        for s in shapes {
            let seen = s.shape.visual_bounding_rect().intersect(s.clip_rect);
            assert!(!seen.is_positive() || bar.expand(0.5).contains_rect(seen), "{at_}: {seen:?} drawn past the bar {bar:?}");
        }
    }

    #[test]
    fn the_run_row_gives_way_in_its_order() {
        // RUN CURRENT's shortcut, RUN's, RUN CURRENT, then the lead narrows to 96; past that the
        // row is cut at its end. RUN and CANCEL never go, nothing overlaps, nothing is drawn past
        // the row, and narrowing never takes room back.
        for lead in [None, Some(150.0)] {
            for running in [false, true] {
                let row = RunRow { running, cancel: running, run_current: true, can_run: true, lead };
                let forms = row.forms();
                let mut last = 0;
                for width in sweep() {
                    let at_ = format!("lead {lead:?} running {running} {width} wide");
                    let mut seen = None;
                    let mut bar = Rect::NOTHING;
                    let shapes = painted_in(vec2(1000.0, 200.0), Theme::Light, |ui| {
                        ui.allocate_ui(vec2(width, BAR_H), |ui| {
                            bar = Rect::from_min_size(ui.cursor().min, vec2(width, BAR_H));
                            let mut lead_rect = None;
                            let s = row.show_with(ui, |ui, w| {
                                if lead.is_some() {
                                    lead_rect = Some(ui.allocate_exact_size(vec2(w, 28.0), Sense::hover()).0);
                                }
                            });
                            let fits = row.needs(ui, s.fit) <= width;
                            seen = Some((s.fit, fits, lead_rect, s.run.rect, s.cancel.map(|r| r.rect), s.run_current.map(|r| r.rect)));
                        });
                    });
                    let (fit, fits, lead_rect, run, cancel, current) = seen.unwrap_or_else(|| panic!("{at_}: drawn"));
                    let k = forms.iter().position(|f| *f == fit).unwrap_or_else(|| panic!("{at_}: {fit:?} is a form"));
                    assert!(k >= last, "{at_}: form {k} after form {last}: room taken back");
                    last = k;
                    assert_eq!(current.is_some(), fit.run_current, "{at_}: RUN CURRENT as the form says");
                    assert_eq!(cancel.is_some(), running, "{at_}: CANCEL while running");
                    let keys = |words: &str| whole(&shapes, words).is_some();
                    assert_eq!(keys("CTRL+SHIFT+ENTER"), fit.run_current && fit.run_current_keys, "{at_}: RUN CURRENT's keys");
                    assert_eq!(keys("CTRL+ENTER"), fit.run_keys, "{at_}: RUN's keys");
                    if let Some(l) = lead_rect {
                        assert!(close(l.width(), fit.lead_w), "{at_}: the lead as wide as given: {l:?}");
                        assert!(fit.lead_w >= RUN_CHIP_MIN_W - 0.5, "{at_}: the lead never under 96");
                    }
                    assert!(run.width() > 0.0 && run.left() < bar.right(), "{at_}: RUN drawn: {run:?}");
                    let mut parts = vec![("run", run)];
                    parts.extend(lead_rect.map(|r| ("lead", r)));
                    parts.extend(cancel.map(|r| ("cancel", r)));
                    parts.extend(current.map(|r| ("run current", r)));
                    assert_apart(&parts, &at_);
                    if fits {
                        for (name, r) in &parts {
                            assert!(r.right() <= bar.right() - 11.5, "{at_}: {name} {r:?} whole, 12 before the end");
                        }
                    }
                    assert_within(&shapes, bar, &at_);
                }
                // Without a lead the last two forms are one.
                let narrowest = if lead.is_some() { 4 } else { 3 };
                assert_eq!(last, narrowest, "lead {lead:?} running {running}: the sweep reaches the narrowest form");
            }
        }
    }

    #[test]
    fn the_result_bar_gives_way_in_its_order() {
        // EXPORT's word, EXPORT, the facts' time, then the facts' words cut; the tabs scroll
        // behind the list; VALUE never goes, nothing overlaps, nothing is drawn past the bar, and
        // narrowing never takes room back.
        for n in [0, 1, 3] {
            let result = ResultBar {
                tabs: MANY.iter().take(n).enumerate().map(|(i, title)| ResultTab::new(title).active(i == 0)).collect(),
                state: RunState::Done { rows: 10_000, ms: Some(80) },
                first_of: Some((10_000, 25_000)),
                export: true,
                value: Some(false),
                time_hover: None,
                value_off: false,
            };
            let mut last = 0;
            for width in sweep() {
                let at_ = format!("{n} tabs, {width} wide");
                let mut seen = None;
                let mut bar = Rect::NOTHING;
                let shapes = painted_in(vec2(1000.0, 200.0), Theme::Light, |ui| {
                    ui.allocate_ui(vec2(width, BAR_H), |ui| {
                        bar = Rect::from_min_size(ui.cursor().min, vec2(width, BAR_H));
                        let s = result.clone().show(ui);
                        let tabs: Vec<Rect> = s.tabs.iter().map(|r| r.interact_rect).filter(|r| r.is_positive()).collect();
                        seen = Some((
                            s.fit,
                            tabs,
                            s.list.map(|r| r.rect),
                            s.facts.rect,
                            s.time.map(|r| r.rect),
                            s.export.map(|r| r.rect),
                            s.value.map(|r| r.rect),
                        ));
                    });
                });
                let (fit, tabs, list, facts, time, export, value) = seen.unwrap_or_else(|| panic!("{at_}: drawn"));
                let k = ResultFit::FORMS.iter().position(|f| *f == fit).unwrap_or_else(|| panic!("{at_}: {fit:?} is a form"));
                assert!(k >= last, "{at_}: form {k} after form {last}: room taken back");
                last = k;
                let value = value.unwrap_or_else(|| panic!("{at_}: VALUE never goes"));
                assert!(close(value.right(), bar.right()), "{at_}: VALUE at the end: {value:?}");
                assert_eq!(export.is_some(), fit.export, "{at_}: EXPORT as the form says");
                assert_eq!(whole(&shapes, "EXPORT").is_some(), fit.export && fit.export_word, "{at_}: EXPORT's word");
                assert!(time.is_none() || fit.time, "{at_}: no time once it gave way");
                let mut parts = vec![("value", value), ("facts", facts)];
                parts.extend(export.map(|r| ("export", r)));
                parts.extend(time.map(|r| ("time", r)));
                parts.extend(list.map(|r| ("list", r)));
                parts.extend(tabs.iter().map(|r| ("tab", *r)));
                assert_apart(&parts, &at_);
                for (name, r) in &parts {
                    assert!(r.left() >= bar.left() - 0.5 && r.right() <= bar.right() + 0.5, "{at_}: {name} {r:?} inside the bar");
                }
                assert_within(&shapes, bar, &at_);
            }
            assert_eq!(last, 4, "{n} tabs: the sweep reaches the narrowest form");
        }
    }

    /// The page's many results (174564), the first one shown.
    const MANY: [&str; 6] =
        ["select ... orders", "select ... series", "select ... customers", "update ... dealers", "select ... line_items", "select ... totals"];
    const PAGE_W: f32 = 840.0;

    /// What a harness's result bar drew in its last frame, and the result its caller shows.
    struct Many {
        shown: usize,
        bar: Rect,
        tabs: Vec<Rect>,
        list: Option<Rect>,
        export: Rect,
        items: Vec<(usize, Rect)>,
        asked: Vec<ResultAsked>,
    }

    /// A result bar `width` wide with the first `n` of the page's results, done; room under it for
    /// its list. The caller shows the result it is asked to.
    fn many(theme: Theme, width: f32, n: usize) -> egui_kittest::Harness<'static, Many> {
        let state = Many { shown: 0, bar: Rect::NOTHING, tabs: Vec::new(), list: None, export: Rect::NOTHING, items: Vec::new(), asked: Vec::new() };
        let mut h = harness_in(vec2(width + 2.0 * ROOM, 400.0), theme, state, move |ui, s| {
            ui.add_space(ROOM);
            ui.allocate_ui(vec2(width, BAR_H), |ui| {
                s.bar = Rect::from_min_size(ui.cursor().min, vec2(width, BAR_H));
                let tabs = MANY.iter().take(n).enumerate().map(|(i, title)| ResultTab::new(title).active(i == s.shown)).collect();
                let shown = ResultBar {
                    tabs,
                    state: RunState::Done { rows: 10, ms: Some(24) },
                    first_of: None,
                    export: true,
                    value: Some(false),
                    time_hover: None,
                    value_off: false,
                }
                .show(ui);
                s.tabs = shown.tabs.iter().map(|r| r.rect).collect();
                s.list = shown.list.map(|r| r.rect);
                s.export = shown.export.map_or(Rect::NOTHING, |r| r.rect);
                s.items = shown.listed.iter().map(|(i, r)| (*i, r.rect)).collect();
                if let Some(asked) = shown.asked {
                    s.asked.push(asked);
                    if let ResultAsked::Show(i) = asked {
                        s.shown = i;
                    }
                }
            });
        });
        h.run();
        h
    }

    /// A click of the primary button at `pos`.
    fn click_at<S>(h: &mut egui_kittest::Harness<'_, S>, pos: egui::Pos2) {
        h.input_mut().events.push(egui::Event::PointerMoved(pos));
        for pressed in [true, false] {
            h.input_mut().events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
            h.run();
        }
    }

    #[test]
    fn many_results_scroll_with_the_list_after_them() {
        // Node 174564 (`state=many results`), 840 wide: the result tabs scroll from the bar's left
        // end in what the facts leave, resting at a tab's edge and cut at the list button
        // (`tab/list`, 32 by 36 on the bar's line), which stands right after them and right before
        // the facts' words; the facts end 12 before EXPORT as ever. The list shows every result in
        // order, the shown one checked; choosing one asks to show it, and its tab comes into view.
        for (theme, t) in both() {
            let mut h = many(theme, PAGE_W, 6);
            let s = h.state();
            let list = s.list.unwrap_or_else(|| panic!("{theme:?}: the list button with more results than fit"));
            let shapes = &h.output().shapes;
            let (words, time) = (word(shapes, "10 ROWS").0, word(shapes, "24 MS").0);
            assert_eq!(list, Rect::from_min_size(pos2(list.left(), s.bar.top()), vec2(32.0, 36.0)), "{theme:?}: 32 by 36 on the bar's line");
            assert!(close(list.right(), words.left()), "{theme:?}: the list right before the facts' words: {list:?} {words:?}");
            assert!(close(s.export.left() - (time.right() - Text::Label.spacing()), 12.0), "{theme:?}: the facts end 12 before EXPORT");
            assert_eq!(s.tabs[0].left(), s.bar.left(), "{theme:?}: the tabs from the bar's left end");
            for pair in s.tabs.windows(2) {
                assert_eq!(pair[0].right(), pair[1].left(), "{theme:?}: side by side: {:?}", s.tabs);
            }
            assert!(s.tabs[5].right() > list.left(), "{theme:?}: the last ones out of view: {:?}", s.tabs);
            for c in shapes {
                if let egui::Shape::Text(text) = &c.shape
                    && MANY.contains(&text.galley.text())
                {
                    assert!(c.clip_rect.right() <= list.left() + 0.5, "{theme:?}: `{}` cut at the list: {:?}", text.galley.text(), c.clip_rect);
                }
            }
            let line = (Rect::from_min_max(pos2(list.left(), list.bottom() - 1.0), list.max), t.line);
            assert!(fills_in(shapes, list).contains(&line), "{theme:?}: its line on the bar's");
            assert_eq!(icon_in(shapes, list).0, [t.text_muted], "{theme:?}: ⌄");
            // Its list: every result in order, the shown one checked.
            h.get_by_label("All results").click();
            h.run();
            h.run();
            let s = h.state();
            assert_eq!(s.items.iter().map(|(i, _)| *i).collect::<Vec<_>>(), (0..6).collect::<Vec<_>>(), "{theme:?}: every result in order");
            let drawn = texts(&h.output().shapes);
            for (i, item) in &s.items {
                let title = MANY[*i];
                assert!(
                    drawn.iter().any(|(w, r, c)| w == title && item.contains_rect(*r) && *c == t.text),
                    "{theme:?}: `{title}` in item {i}: {drawn:?}"
                );
                let mark = Rect::from_min_size(pos2(item.left() + 8.0, item.top() + 7.0), vec2(14.0, 14.0));
                let want: Vec<Color32> = if *i == 0 { vec![t.text] } else { vec![] };
                assert_eq!(icon_in(&h.output().shapes, mark).0, want, "{theme:?}: item {i}'s mark");
            }
            // Choosing the last asks to show it, and its tab comes into view.
            let last = s.items.iter().find(|(i, _)| *i == 5).map(|(_, r)| r.center());
            click_at(&mut h, last.unwrap_or_else(|| panic!("{theme:?}: the last result listed")));
            h.run();
            let s = h.state();
            assert_eq!(s.asked, [ResultAsked::Show(5)], "{theme:?}: choosing a result asks to show it");
            let list = s.list.unwrap_or(Rect::NOTHING);
            assert!(s.tabs[5].left() >= s.bar.left() - 0.5 && s.tabs[5].right() <= list.left() + 0.5, "{theme:?}: its tab in view: {:?}", s.tabs);
        }
        // Results that fit: no list button.
        let h = many(Theme::Light, PAGE_W, 2);
        assert!(h.state().list.is_none(), "no list button while the results fit");
    }

    /// What a harness of the three bars saw clicked.
    #[derive(Default)]
    struct Asked {
        run: Vec<RunAsked>,
        result: Vec<ResultAsked>,
        count: u32,
    }

    #[test]
    fn the_bars_say_what_was_asked() {
        let mut h = harness(Theme::Light, Asked::default(), |ui, seen| {
            ui.allocate_ui(vec2(NARROW, BAR_H), |ui| {
                seen.run.extend(RunRow { running: false, cancel: false, run_current: true, can_run: true, lead: None }.show(ui).asked)
            });
            ui.allocate_ui(vec2(NARROW, BAR_H), |ui| {
                seen.run.extend(RunRow { running: true, cancel: true, run_current: false, can_run: true, lead: None }.show(ui).asked)
            });
            ui.allocate_ui(vec2(NARROW, BAR_H), |ui| {
                let tabs = vec![ResultTab::new("first").active(true), ResultTab::new("second")];
                seen.result.extend(
                    ResultBar {
                        tabs,
                        state: RunState::Done { rows: 3, ms: None },
                        first_of: None,
                        export: true,
                        value: Some(false),
                        time_hover: None,
                        value_off: false,
                    }
                    .show(ui)
                    .asked,
                );
            });
            ui.horizontal(|ui| seen.count += u32::from(RowCount::Unknown.show(ui, None).asked));
        });
        h.run();
        h.get_by_label("Run current").click();
        h.run();
        h.get_by_label("Cancel").click();
        h.run();
        assert_eq!(h.state().run, [RunAsked::RunCurrent, RunAsked::Cancel], "RUN CURRENT and CANCEL");
        for run in h.get_all_by_label("Run") {
            run.click();
        }
        h.run();
        assert_eq!(h.state().run, [RunAsked::RunCurrent, RunAsked::Cancel, RunAsked::Run], "RUN while idle; while running it is off");
        h.get_by_label("first").click();
        h.run();
        assert!(h.state().result.is_empty(), "the shown result's tab asks nothing");
        h.get_by_label("second").click();
        h.run();
        h.get_by_label("panel").click();
        h.run();
        h.get_by_label("Export").click();
        h.run();
        assert_eq!(h.state().result, [ResultAsked::Show(1), ResultAsked::Value], "a tab, the toggle; EXPORT is off");
        h.get_by_label("Count rows").click();
        h.run();
        assert_eq!(h.state().count, 1, "COUNT ROWS");
        // EXPORT says why it is off.
        h.get_by_label("Export").hover();
        h.run();
        h.run();
        assert!(texts(&h.output().shapes).iter().any(|(w, _, _)| w == EXPORT_LATER), "EXPORT's hover says why");
        // Tab reaches a result tab; Enter shows it.
        let mut h = harness(Theme::Light, Vec::new(), |ui, seen: &mut Vec<ResultAsked>| {
            ui.allocate_ui(vec2(NARROW, BAR_H), |ui| {
                let tabs = vec![ResultTab::new("first").active(true), ResultTab::new("second")];
                seen.extend(
                    ResultBar {
                        tabs,
                        state: RunState::Cancelled { elapsed: None },
                        first_of: None,
                        export: false,
                        value: Some(false),
                        time_hover: None,
                        value_off: false,
                    }
                    .show(ui)
                    .asked,
                );
            });
        });
        h.run();
        for _ in 0..2 {
            h.key_press(Key::Tab);
            h.run();
        }
        assert!(h.get_by_label("second").is_focused(), "Tab reaches the second tab");
        h.key_press(Key::Enter);
        h.run();
        assert_eq!(*h.state(), [ResultAsked::Show(1)], "Enter shows it");
    }
}
