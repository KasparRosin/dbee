//! The query tab's bars: the run row over the results (`run row`), and the result bar
//! (`result bar`) with a tab per result (`result tab`), what the shown result holds and how long
//! the run took, EXPORT and the value panel's toggle. And the row count (`rows/count`), which the
//! table bar holds.
//!
//! Each is a view: it takes what to show and says what was clicked; the screen tags what it drew
//! and acts. Numbers, timings and a result's name are data: drawn as written (in `data` where the
//! page's words are mono), so only dbee's own words go to capitals.

use std::sync::Arc;
use std::time::Duration;

use egui::text::TextWrapping;
use egui::{Align, Color32, CornerRadius, Galley, Id, Layout, Rect, Response, Sense, Ui, UiBuilder, Widget, WidgetInfo, WidgetType, pos2, vec2};

use super::button::{BarButton, BoxButton, TextButton};
use super::icon::Icon;
use super::spinner::Spinner;
use super::status::{Piece, piece, timer};
use super::tooltip::Hint;
use super::{Look, focus, painter};
use crate::theme::size::{
    BAR_CELL_H, BAR_H, BAR_LINE, BUTTON_H, RESULT_FACT_GAP, RESULT_TAB_PAD_X, RESULT_TAB_TITLE_MAX, ROW_COUNT_COUNTING_GAP, ROW_COUNT_GAP,
    RUN_ROW_GAP, RUN_ROW_PAD_X, SPINNER, TAB_ACTIVE_LINE,
};
use crate::theme::{Text, Tokens, Words, tokens};
use crate::tree::thousands;

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

/// The row of run buttons: RUN, CANCEL while a run goes, and RUN CURRENT when shown.
#[derive(Clone, Copy, Debug)]
pub struct RunRow {
    /// The tab's run goes: RUN and RUN CURRENT are off, CANCEL shows.
    pub running: bool,
    /// RUN CURRENT shows (as today, while Kaspar hasn't decided on it).
    pub run_current: bool,
    /// A run can start (the engine's own check): RUN and RUN CURRENT are on while nothing runs.
    pub can_run: bool,
}

/// What a click in the run row asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunAsked {
    Run,
    RunCurrent,
    Cancel,
}

/// The run row as drawn: each button, and what a click asked for.
pub struct RunShown {
    pub run: Response,
    pub cancel: Option<Response>,
    pub run_current: Option<Response>,
    pub asked: Option<RunAsked>,
}

impl RunRow {
    /// Draws it as wide as `ui`, [`BAR_H`] tall.
    pub fn show(self, ui: &mut Ui) -> RunShown {
        let (bar, cell) = bar_ground(ui);
        let row = Rect::from_min_max(pos2(cell.left() + RUN_ROW_PAD_X, cell.top()), cell.max);
        let mut ui = ui.new_child(UiBuilder::new().id_salt(bar.id).max_rect(row).layout(Layout::left_to_right(Align::Center)));
        ui.spacing_mut().item_spacing.x = RUN_ROW_GAP;
        let on = self.can_run && !self.running;
        let run = ui.add_enabled(on, BoxButton::primary("Run").shortcut("Ctrl+Enter"));
        let cancel = self.running.then(|| ui.add(BoxButton::primary("Cancel")));
        let run_current = self.run_current.then(|| ui.add_enabled(on, BoxButton::outline("Run current").shortcut("Ctrl+Shift+Enter")));
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
        RunShown { run, cancel, run_current, asked }
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
    /// The shown statement changed `rows` rows and returned none.
    Affected {
        rows: u64,
        ms: Option<u64>,
    },
}

/// What a click in the result bar asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResultAsked {
    /// Show the result of this tab (0-based): a click on a tab that isn't the shown one, or Enter
    /// or Space on it.
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
    /// The value panel is open.
    pub value_on: bool,
}

/// The result bar as drawn: its tabs, its facts' words, EXPORT and the toggle, and what a click
/// asked for.
pub struct ResultShown {
    pub tabs: Vec<Response>,
    pub facts: Response,
    pub export: Option<Response>,
    pub value: Response,
    pub asked: Option<ResultAsked>,
}

/// `n rows`, `1 row`: the number as written in `data`.
fn rows_of(words: Words, n: u64) -> Words {
    words.name(thousands(n)).plain(if n == 1 { " row" } else { " rows" })
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
enum Time {
    Words(Piece),
    Spinner,
}

impl Time {
    fn of(ui: &Ui, t: &Tokens, words: String) -> Time {
        Time::Words(Piece::new(ui, Text::Data.job_as_written(&words, t.text_muted), words))
    }

    fn width(&self) -> f32 {
        match self {
            Time::Words(p) => p.width(),
            Time::Spinner => SPINNER,
        }
    }

    /// Draws it from `x`, centred on `mid`.
    fn show(self, ui: &Ui, id: Id, x: f32, mid: f32, t: &Tokens) {
        match self {
            Time::Words(p) => {
                piece(ui, id, &p, pos2(x, mid), t.text_muted);
            }
            Time::Spinner => Spinner::new().paint(ui, pos2(x + SPINNER / 2.0, mid)),
        }
    }
}

impl ResultBar<'_> {
    /// The facts: the words and their colour, and how long the run took or has gone.
    fn facts(&self, ui: &Ui, t: &Tokens) -> (Words, Color32, Option<Time>) {
        let ms = |ms: Option<u64>| ms.map(|ms| Time::of(ui, t, took(Duration::from_millis(ms))));
        match self.state {
            RunState::Done { rows, ms: m } => {
                let words = match self.first_of {
                    Some((kept, seen)) => rows_of(Words::from("First ").name(thousands(kept)).plain(" of "), seen),
                    None => rows_of(Words::default(), rows),
                };
                (words, t.text_muted, ms(m))
            }
            RunState::Running { elapsed } => {
                (Words::from("Running"), t.text_muted, Some(elapsed.map_or(Time::Spinner, |e| Time::of(ui, t, timer(e)))))
            }
            RunState::Failed { ms: m } => (Words::from("Failed"), t.danger, ms(m)),
            RunState::Cancelled { elapsed } => (Words::from("Cancelled"), t.text_muted, elapsed.map(|e| Time::of(ui, t, took(e)))),
            RunState::Affected { rows, ms: m } => (rows_of(Words::default(), rows).plain(" affected"), t.text_muted, ms(m)),
        }
    }

    /// Draws it as wide as `ui`, [`BAR_H`] tall.
    pub fn show(self, ui: &mut Ui) -> ResultShown {
        let t = tokens(ui.ctx());
        let (bar, cell) = bar_ground(ui);
        let id = bar.id;
        let mid = cell.center().y;
        let mut ui = ui.new_child(UiBuilder::new().id_salt(id).max_rect(cell).layout(Layout::left_to_right(Align::Min)));
        // Measured from the right end: the toggle, EXPORT, then the facts 12 in from them.
        let value = BarButton::icon(Icon::Panel).on(self.value_on).over_line();
        let value_rect = Rect::from_min_max(pos2(cell.right() - value.width(&ui), cell.top()), cell.max);
        let export = self.export.then(|| BarButton::label(Icon::Export, "Export").over_line());
        let export_rect =
            export.as_ref().map(|b| Rect::from_min_max(pos2(value_rect.left() - b.width(&ui), cell.top()), pos2(value_rect.left(), cell.bottom())));
        let (words, colour, mut time) = self.facts(&ui, t);
        let says = words.to_string();
        let mut job = words.caps_job(colour);
        let laid = Piece::new(&ui, job.clone(), says.clone());
        // Short of room the time goes, then the words are cut.
        let facts_end = export_rect.unwrap_or(value_rect).left() - RESULT_FACT_GAP;
        let facts_room = (facts_end - cell.left()).max(0.0);
        if time.as_ref().is_some_and(|time| laid.width() + RESULT_FACT_GAP + time.width() > facts_room) {
            time = None;
        }
        let words = if laid.width() > facts_room {
            job.wrap = TextWrapping::truncate_at_width(facts_room.max(1.0));
            Piece::new(&ui, job, says)
        } else {
            laid
        };
        let facts_w = words.width() + time.as_ref().map_or(0.0, |time| RESULT_FACT_GAP + time.width());
        let facts_left = facts_end - facts_w;

        // Left to right, so Tab reaches them as they read: the tabs, kept left of the facts.
        let tabs_room = Rect::from_min_max(cell.min, pos2((facts_left - RESULT_FACT_GAP).max(cell.left()), cell.bottom()));
        let mut asked = None;
        let tabs = ui
            .scope_builder(UiBuilder::new().max_rect(tabs_room).layout(Layout::left_to_right(Align::Min)), |ui| {
                ui.set_clip_rect(tabs_room.intersect(ui.clip_rect()));
                ui.spacing_mut().item_spacing.x = 0.0;
                let mut drawn = Vec::with_capacity(self.tabs.len());
                for (i, tab) in self.tabs.into_iter().enumerate() {
                    let active = tab.active;
                    let r = ui.add(tab);
                    if r.clicked() && !active {
                        asked = Some(ResultAsked::Show(i));
                    }
                    drawn.push(r);
                }
                drawn
            })
            .inner;
        let facts = piece(&ui, id.with("facts"), &words, pos2(facts_left, mid), colour);
        if let Some(time) = time {
            time.show(&ui, id.with("time"), facts_left + words.width() + RESULT_FACT_GAP, mid, t);
        }
        let export = export.zip(export_rect).map(|(b, rect)| ui.add_enabled_ui(false, |ui| ui.put(rect, b)).inner.hint_disabled(EXPORT_LATER));
        let value = ui.put(value_rect, value);
        if value.clicked() {
            asked = Some(ResultAsked::Value);
        }
        ResultShown { tabs, facts, export, value, asked }
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
    /// Counting, for `elapsed` so far (today's spinner while the app gives no timer), with the
    /// estimate shown meanwhile, if any. Its cancel is the status bar's job.
    Counting {
        estimate: Option<u64>,
        elapsed: Option<Duration>,
    },
}

/// The row count as drawn: its words (none with no rows shown and no count), its COUNT or COUNT
/// ROWS, and whether that was clicked.
pub struct CountShown {
    pub words: Option<Response>,
    pub count: Option<Response>,
    pub asked: bool,
}

impl RowCount {
    /// The words: the rows shown (`1–100`) and how many there are (`of 25,000 rows`, `of ~100,000
    /// rows`), either alone when the other isn't there.
    fn words(self, shown: Option<(u64, u64)>) -> Words {
        let total = match self {
            RowCount::Exact(n) => Some(thousands(n)),
            RowCount::Estimate(n) | RowCount::Counting { estimate: Some(n), .. } if n > 0 => Some(format!("~{}", thousands(n))),
            _ => None,
        };
        let n = match self {
            RowCount::Exact(n) | RowCount::Estimate(n) | RowCount::Counting { estimate: Some(n), .. } => n,
            _ => 0,
        };
        let rows = if n == 1 { " row" } else { " rows" };
        match (shown, total) {
            (Some((first, last)), Some(total)) => {
                Words::default().name(format!("{}–{}", thousands(first), thousands(last))).plain(" of ").name(total).plain(rows)
            }
            (Some((first, last)), None) => Words::default().name(format!("{}–{}", thousands(first), thousands(last))),
            (None, Some(total)) => Words::default().name(total).plain(rows),
            (None, None) => Words::default(),
        }
    }

    /// Draws it where `ui` puts it next, as wide as what it holds and as tall as its button, with
    /// the rows shown (`1–100`), if any.
    pub fn show(self, ui: &mut Ui, shown: Option<(u64, u64)>) -> CountShown {
        let t = tokens(ui.ctx());
        let words = self.words(shown);
        let says = words.to_string();
        let words = (!says.is_empty()).then(|| Piece::new(ui, words.caps_job(t.text_muted), says));
        let button = match self {
            RowCount::Estimate(n) if n > 0 => Some(TextButton::new("Count")),
            RowCount::Estimate(_) | RowCount::Unknown => Some(TextButton::new("Count rows")),
            RowCount::Exact(_) | RowCount::Counting { .. } => None,
        };
        let counting = match self {
            RowCount::Counting { elapsed, .. } => {
                let time = elapsed.map_or(Time::Spinner, |e| Time::of(ui, t, timer(e)));
                Some((Piece::new(ui, Text::Label.job("Counting", t.text_muted), "Counting"), time))
            }
            _ => None,
        };
        // Each part after the words with the gap before it; none before the first.
        let words_w = words.as_ref().map_or(0.0, Piece::width);
        let any = words.is_some();
        let gap = |w: f32| if any { w } else { 0.0 };
        let button_w = button.as_ref().map_or(0.0, |b| gap(ROW_COUNT_GAP) + b.width(ui));
        let counting_w = counting.as_ref().map_or(0.0, |(c, time)| gap(ROW_COUNT_COUNTING_GAP) + c.width() + ROW_COUNT_COUNTING_GAP + time.width());
        let (rect, r) = ui.allocate_exact_size(vec2((words_w + button_w + counting_w).ceil(), BUTTON_H), Sense::hover());
        let id = r.id;
        let mid = rect.center().y;
        let words = words.map(|w| piece(ui, id.with("words"), &w, pos2(rect.left(), mid), t.text_muted));
        let mut x = rect.left() + words_w;
        let count = button.map(|b| {
            x += gap(ROW_COUNT_GAP);
            let w = b.width(ui);
            ui.put(Rect::from_min_size(pos2(x, rect.top()), vec2(w, BUTTON_H)), b)
        });
        if let Some((c, time)) = counting {
            x += gap(ROW_COUNT_COUNTING_GAP);
            piece(ui, id.with("counting"), &c, pos2(x, mid), t.text_muted);
            time.show(ui, id.with("timer"), x + c.width() + ROW_COUNT_COUNTING_GAP, mid, t);
        }
        let asked = count.as_ref().is_some_and(Response::clicked);
        CountShown { words, count, asked }
    }
}

#[cfg(test)]
mod tests {
    use egui::epaint::ClippedShape;
    use egui::{Color32, CornerRadius, Key, Theme, pos2, vec2};
    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::components::{Edge, Look, ROOM, both, edges_in, fills_in, harness, icon_in, painted, sections, texts};
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
                    let s = RunRow { running, run_current, can_run: true }.show(ui);
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
        ResultBar { tabs, state, first_of, export, value_on }
    }

    #[test]
    fn each_result_bar_state_matches_the_page() {
        // Node 164136 and the page's rows of it: 36 tall on `card` with a 1 px `line` along its
        // bottom. From the left the result tabs, side by side; at the right end the value panel's
        // toggle (a bar button, 36 wide), EXPORT before it (off), each with its divider and 35
        // tall; before them, 12 in, the facts 12 apart, centred above the line: what the result
        // holds or what became of the run in `label` capitals with its numbers in `data` as
        // written, and how long it took in `data`, all `text_muted` but FAILED in `danger`.
        use Text::{Data, Label};
        let s = Duration::from_millis;
        for (theme, t) in both() {
            let m = t.text_muted;
            let rows: Vec<(&str, ResultBar<'static>, Vec<(&str, Text, Color32)>, Option<&str>)> = vec![
                (
                    "done",
                    bar(true, RunState::Done { rows: 10, ms: Some(24) }, None, true, false),
                    vec![("10", Data, m), (" ROWS", Label, m)],
                    Some("24 ms"),
                ),
                ("running", bar(true, RunState::Running { elapsed: Some(s(2400)) }, None, true, false), vec![("RUNNING", Label, m)], Some("00:02.4")),
                ("failed", bar(false, RunState::Failed { ms: Some(12) }, None, false, false), vec![("FAILED", Label, t.danger)], Some("12 ms")),
                (
                    "cancelled",
                    bar(false, RunState::Cancelled { elapsed: Some(s(3200)) }, None, false, false),
                    vec![("CANCELLED", Label, m)],
                    Some("3.2 s"),
                ),
                (
                    "affected",
                    bar(true, RunState::Affected { rows: 2, ms: Some(5) }, None, false, false),
                    vec![("2", Data, m), (" ROWS AFFECTED", Label, m)],
                    Some("5 ms"),
                ),
                (
                    "first of",
                    bar(true, RunState::Done { rows: 10_000, ms: Some(80) }, Some((10_000, 25_000)), true, false),
                    vec![("FIRST ", Label, m), ("10,000", Data, m), (" OF ", Label, m), ("25,000", Data, m), (" ROWS", Label, m)],
                    Some("80 ms"),
                ),
                (
                    "value on",
                    bar(true, RunState::Done { rows: 10, ms: Some(24) }, None, true, true),
                    vec![("10", Data, m), (" ROWS", Label, m)],
                    Some("24 ms"),
                ),
            ];
            for (name, result, want, time) in rows {
                let at_ = format!("{theme:?} {name}");
                let (tabs, export, value_on) = (result.tabs.len(), result.export, result.value_on);
                let mut seen = (Vec::new(), None, Rect::NOTHING);
                let (shapes, at) = drawn(theme, |ui| {
                    let s = result.clone().show(ui);
                    seen = (s.tabs.iter().map(|r| r.rect).collect(), s.export.map(|r| r.rect), s.value.rect);
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
                // The facts.
                let whole_words: String = want.iter().map(|(w, _, _)| *w).collect();
                let (words, _) = word(&shapes, &whole_words);
                let parts: Vec<(String, egui::FontId, Color32)> = want.iter().map(|(w, style, c)| ((*w).to_owned(), style.font(), *c)).collect();
                assert_eq!(sections(&shapes, &whole_words), parts, "{at_}: the words, their numbers in data");
                let mid = cell.center().y;
                assert!(close(words.center().y, mid), "{at_}: the words centred above the line: {words:?}");
                // egui puts the letter spacing after a caps text's last letter too.
                let trail = if want.last().is_some_and(|(_, style, _)| *style == Label) { Label.spacing() } else { 0.0 };
                let last = match time {
                    Some(time) => {
                        assert_eq!(sections(&shapes, time), [(time.to_owned(), Data.font(), m)], "{at_}: the time in data, as written");
                        let (at_time, _) = word(&shapes, time);
                        assert!(close(at_time.left() - (words.right() - trail), 12.0), "{at_}: 12 between the facts: {words:?} {at_time:?}");
                        assert!(close(at_time.center().y, mid), "{at_}: the time centred: {at_time:?}");
                        at_time.right()
                    }
                    None => words.right() - trail,
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
    fn each_row_count_matches_the_page() {
        // Node 164223: 28 tall; the rows shown and how many there are in `label` capitals,
        // `text_muted`, the numbers in `data` as written; an estimate with COUNT and no count with
        // COUNT ROWS (`button/text`), 8 after the words; counting: 12, COUNTING in `label`, 12,
        // the timer in `data`. The page's COUNT is 56 wide, COUNT ROWS 92.
        use Text::{Data, Label};
        let s = Duration::from_millis;
        for (theme, t) in both() {
            let m = t.text_muted;
            let range = ("1–100", Data, m);
            let rows: Vec<(RowCount, Vec<(&str, Text, Color32)>, Option<(&str, f32)>, Option<&str>)> = vec![
                (RowCount::Exact(25_000), vec![range, (" OF ", Label, m), ("25,000", Data, m), (" ROWS", Label, m)], None, None),
                (
                    RowCount::Estimate(100_000),
                    vec![range, (" OF ", Label, m), ("~100,000", Data, m), (" ROWS", Label, m)],
                    Some(("COUNT", 56.0)),
                    None,
                ),
                (RowCount::Unknown, vec![range], Some(("COUNT ROWS", 92.0)), None),
                (
                    RowCount::Counting { estimate: Some(100_000), elapsed: Some(s(1200)) },
                    vec![range, (" OF ", Label, m), ("~100,000", Data, m), (" ROWS", Label, m)],
                    None,
                    Some("00:01.2"),
                ),
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
                let whole_words: String = want.iter().map(|(w, _, _)| *w).collect();
                let (words, _) = word(&shapes, &whole_words);
                let parts: Vec<(String, egui::FontId, Color32)> = want.iter().map(|(w, style, c)| ((*w).to_owned(), style.font(), *c)).collect();
                assert_eq!(sections(&shapes, &whole_words), parts, "{at_}: the words, their numbers in data");
                assert!(close(words.left(), at.left()) && close(words.center().y, at.center().y), "{at_}: the words first, centred: {words:?}");
                assert!(words_r.is_some_and(|r| r.contains_rect(words.shrink(0.5))), "{at_}: the words' response: {words_r:?}");
                let trail = if want.last().is_some_and(|(_, style, _)| *style == Label) { Label.spacing() } else { 0.0 };
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
        // A result's name is drawn and read out as written; the facts' numbers too, and the
        // words read out as written; only dbee's words are drawn in capitals.
        let mut h = harness(Theme::Light, (), |ui, _| {
            ui.allocate_ui(vec2(NARROW, BAR_H), |ui| {
                let tabs = vec![ResultTab::new("select … Orders").active(true), ResultTab::new("select … Line_Items")];
                ResultBar {
                    tabs,
                    state: RunState::Done { rows: 10_000, ms: Some(80) },
                    first_of: Some((10_000, 25_000)),
                    export: true,
                    value_on: false,
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
        for number in ["10,000", "25,000", "~25,000", "1–100"] {
            assert!(drawn.iter().any(|w| w == number), "`{number}` drawn as written: {drawn:?}");
        }
        for says in ["First 10,000 of 25,000 rows", "1–100 of ~25,000 rows", "80 ms"] {
            assert!(h.query_by_label(says).is_some(), "`{says}` read out as written");
        }
        assert!(h.query_by_label("FIRST 10,000 OF 25,000 ROWS").is_none(), "the capitals are only drawn");
    }

    #[test]
    fn timings_read_as_the_page_writes_them() {
        let ms = Duration::from_millis;
        assert_eq!(took(ms(24)), "24 ms");
        assert_eq!(took(ms(999)), "999 ms");
        assert_eq!(took(ms(3200)), "3.2 s");
        assert_eq!(took(ms(65_300)), "01:05.3", "past a minute, as the timer reads");
        assert_eq!(rows_of(Words::default(), 1).to_string(), "1 row");
        assert_eq!(RowCount::Exact(1).words(Some((1, 1))).to_string(), "1–1 of 1 row");
        assert_eq!(RowCount::Estimate(0).words(Some((1, 100))).to_string(), "1–100", "never ~0");
        assert_eq!(RowCount::Exact(0).words(None).to_string(), "0 rows");
        assert_eq!(RowCount::Unknown.words(None).to_string(), "");
    }

    #[test]
    fn with_no_timer_todays_spinner_stands_in() {
        // The app gives no timer yet (the group's rule): RUNNING and COUNTING show today's
        // spinner where the timer goes; an estimate of 0 asks to COUNT ROWS.
        let (shapes, at) = drawn(Theme::Light, |ui| {
            ResultBar { tabs: Vec::new(), state: RunState::Running { elapsed: None }, first_of: None, export: false, value_on: false }.show(ui);
        });
        let (running, _) = word(&shapes, "RUNNING");
        let slot =
            Rect::from_min_max(pos2(running.right() - Text::Label.spacing() + 12.0, at.top()), pos2(running.right() + 12.0 + SPINNER, at.bottom()));
        assert_eq!(icon_in(&shapes, slot.expand(1.0)).0, [crate::theme::LIGHT.text_muted], "the spinner 12 after RUNNING: {slot:?}");
        let shapes = painted(Theme::Light, |ui| {
            ui.horizontal(|ui| {
                RowCount::Counting { estimate: None, elapsed: None }.show(ui, Some((1, 100)));
                RowCount::Estimate(0).show(ui, Some((1, 100)));
            });
        });
        let (counting, _) = word(&shapes, "COUNTING");
        let slot = Rect::from_min_max(
            pos2(counting.right() - Text::Label.spacing() + 12.0, counting.top() - 6.0),
            pos2(counting.right() + 12.0 + SPINNER, counting.bottom() + 6.0),
        );
        assert_eq!(icon_in(&shapes, slot.expand(1.0)).0, [crate::theme::LIGHT.text_muted], "the spinner 12 after COUNTING: {slot:?}");
        assert!(whole(&shapes, "COUNT ROWS").is_some() && whole(&shapes, "COUNT").is_none(), "an estimate of 0 is no estimate");
    }

    #[test]
    fn a_narrow_result_bar_draws_nothing_over_another() {
        // The tabs take what the facts leave and are cut there; short of room the time goes,
        // then the words are cut; EXPORT and the toggle stay.
        let tabs: Vec<ResultTab<'static>> =
            (0..6).map(|i| ResultTab::new(["select ... orders", "select ... series"][i % 2]).active(i == 0)).collect();
        let result =
            ResultBar { tabs, state: RunState::Done { rows: 10_000, ms: Some(80) }, first_of: Some((10_000, 25_000)), export: true, value_on: false };
        for width in [760.0, 560.0, 420.0, 300.0, 200.0] {
            let mut seen = (Vec::new(), None, Rect::NOTHING, Rect::NOTHING, Rect::NOTHING);
            let shapes = painted(Theme::Light, |ui| {
                ui.allocate_ui(vec2(width, BAR_H), |ui| {
                    let at = Rect::from_min_size(ui.cursor().min, vec2(width, BAR_H));
                    let s = result.clone().show(ui);
                    seen = (s.tabs.iter().map(|r| r.interact_rect).collect(), s.export.map(|r| r.rect), s.value.rect, s.facts.rect, at);
                });
            });
            let (tabs, export, value, facts, at) = seen;
            let export = export.unwrap_or_else(|| panic!("{width}: EXPORT stays"));
            assert!(value.right() <= at.right() + 0.5 && export.right() <= value.left(), "{width}: EXPORT, then the toggle at the end");
            let drawn: Vec<(String, Rect)> = texts(&shapes).into_iter().map(|(w, r, _)| (w, r)).collect();
            let facts_drawn: Vec<&(String, Rect)> = drawn.iter().filter(|(w, _)| !w.starts_with("select") && !w.contains("EXPORT")).collect();
            assert!(!facts_drawn.is_empty(), "{width}: the facts drawn");
            for (w, r) in &facts_drawn {
                assert!(r.left() >= at.left() - 0.5 && r.right() <= export.left() + 0.5, "{width}: `{w}` {r:?} between the bar's start and EXPORT");
            }
            if let Some((_, time)) = drawn.iter().find(|(w, _)| w == "80 ms") {
                assert!(time.right() <= export.left() - 11.5, "{width}: the time 12 before EXPORT: {time:?}");
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
            ui.allocate_ui(vec2(NARROW, BAR_H), |ui| seen.run.extend(RunRow { running: false, run_current: true, can_run: true }.show(ui).asked));
            ui.allocate_ui(vec2(NARROW, BAR_H), |ui| seen.run.extend(RunRow { running: true, run_current: false, can_run: true }.show(ui).asked));
            ui.allocate_ui(vec2(NARROW, BAR_H), |ui| {
                let tabs = vec![ResultTab::new("first").active(true), ResultTab::new("second")];
                seen.result.extend(
                    ResultBar { tabs, state: RunState::Done { rows: 3, ms: None }, first_of: None, export: true, value_on: false }.show(ui).asked,
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
                    ResultBar { tabs, state: RunState::Cancelled { elapsed: None }, first_of: None, export: false, value_on: false }.show(ui).asked,
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
