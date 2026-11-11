//! The status bar along the window's bottom (`status bar`), with the connection's block
//! (`status/connection`) and one activity at its right (`status/activity`).
//!
//! From the left: the connection's name on a block of its colour, under an edge of that colour
//! along the bar's top (a connection with no colour keeps a quiet bar: a `line` edge and the name
//! in `text`); then the facts in grey: the server's version as written, the link's security while
//! it is weaker than fully checked, and READ-ONLY; then SAVE CONNECTION while the saved list
//! doesn't hold the connection. At the right end, what the screen puts there (the value panel's
//! toggle, until the Phase 3 screens move it), and before it one activity.
//!
//! When the bar is too narrow, the facts give way in this order: the server version, the link's
//! security, READ-ONLY; then the name is cut. SAVE CONNECTION, the activity and its button stay.
//!
//! A bar is a view: it takes what to show and says what was clicked ([`Shown`]); the screen tags
//! what it drew and acts.

use std::sync::Arc;
use std::time::Duration;

use dbee_config::ConnectionColor;
use egui::text::{LayoutJob, TextWrapping};
use egui::{Align, Color32, CornerRadius, Galley, Id, Layout, Rect, Response, Sense, Ui, UiBuilder, WidgetInfo, WidgetType, pos2, vec2};

use super::button::TextButton;
use super::icon::Icon;
use super::spinner::Spinner;
use super::tooltip::Hint;
use crate::theme::size::{
    SLIM_BAR_H, SPINNER, STATUS_AFTER_BLOCK, STATUS_BLOCK_H, STATUS_BLOCK_PAD_X, STATUS_EDGE, STATUS_FACT_GAP, STATUS_GAP, STATUS_NAME_MAX,
    STATUS_PAD_X, TRANSACTION_MARK,
};
use crate::theme::{Text, Tokens, Words, connection_color, on_connection_color, tokens};

/// A connection with no colour: a `line` edge this thick.
const QUIET_EDGE: f32 = 1.0;

/// The connection the bar is about: its name and its colour.
#[derive(Clone, Copy, Debug)]
pub struct Connection<'a> {
    pub name: &'a str,
    pub colour: ConnectionColor,
    /// The user gave the name, so it may be drawn in capitals; a name dbee made from the user,
    /// host, database or file is drawn as written.
    pub own: bool,
}

/// The facts after the name.
#[derive(Clone, Copy, Debug, Default)]
pub struct Facts<'a> {
    /// The server's product and version, as it reports them.
    pub server: Option<&'a str>,
    /// What protects the link, and its longer text for the hover, while it is weaker than fully
    /// checked.
    pub link: Option<(&'a str, &'a str)>,
    pub read_only: bool,
    /// The saved list doesn't hold the connection: SAVE CONNECTION.
    pub unsaved: bool,
}

/// What goes on in the background: one at a time, at the bar's right.
#[derive(Clone, Debug, PartialEq)]
pub enum Activity {
    /// The connection's session has a transaction open (or failed, waiting for a ROLLBACK).
    InTransaction,
    /// The connection is lost: RECONNECT.
    Disconnected,
    Reconnecting {
        elapsed: Option<Duration>,
    },
    /// For M4 and the showcase.
    Exporting {
        elapsed: Option<Duration>,
    },
    /// For M4 and the showcase.
    Importing {
        elapsed: Option<Duration>,
    },
    /// What runs, and whether it can be cancelled: CANCEL.
    Job {
        text: Words,
        elapsed: Option<Duration>,
        cancel: bool,
    },
}

/// What a click in the bar asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    SaveConnection,
    Reconnect,
    Cancel,
}

/// The bar as drawn: each part shown, and what a click asked for.
pub struct Shown {
    pub connection: Response,
    pub server: Option<Response>,
    pub link: Option<Response>,
    pub read_only: Option<Response>,
    pub save: Option<Response>,
    /// The activity's words.
    pub activity: Option<Response>,
    /// Its button: RECONNECT or CANCEL.
    pub button: Option<Response>,
    pub act: Option<Act>,
}

/// One text of a bar: laid out, with what it reads as written.
pub(super) struct Piece {
    pub(super) galley: Arc<Galley>,
    says: String,
    /// The letter spacing egui puts after the last letter too; the page's measure has none there.
    trail: f32,
}

impl Piece {
    pub(super) fn new(ui: &Ui, job: LayoutJob, says: impl Into<String>) -> Piece {
        let trail = job.sections.last().map_or(0.0, |s| s.format.extra_letter_spacing);
        Piece { galley: ui.painter().layout_job(job), says: says.into(), trail }
    }

    pub(super) fn width(&self) -> f32 {
        self.galley.size().x - self.trail
    }
}

/// The bar, [`SLIM_BAR_H`] tall and as wide as `ui`: `connection`, its `facts`, then `activity` and
/// what `right` draws at the right end.
pub fn bar(ui: &mut Ui, connection: &Connection<'_>, facts: &Facts<'_>, activity: Option<&Activity>, right: impl FnOnce(&mut Ui)) -> Shown {
    let t = tokens(ui.ctx());
    let id = ui.id().with("dbee-status");
    let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), SLIM_BAR_H), Sense::hover());
    let colour = connection_color(connection.colour).zip(on_connection_color(connection.colour));
    let edge = if colour.is_some() { STATUS_EDGE } else { QUIET_EDGE };
    let p = ui.painter();
    p.rect_filled(bar, CornerRadius::ZERO, t.bg);
    p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width(), edge)), CornerRadius::ZERO, colour.map_or(t.line, |(fill, _)| fill));
    // What is under the edge: everything is centred in it.
    let under = Rect::from_min_max(pos2(bar.left(), bar.top() + edge), bar.max);
    let mid = under.center().y;

    // The right end first: what stays there, then the activity before it.
    let end = Rect::from_min_max(under.min, pos2(under.right() - STATUS_PAD_X, under.bottom()));
    let drawn = ui.scope_builder(UiBuilder::new().max_rect(end).layout(Layout::right_to_left(Align::Center)), right).response.rect;
    let mut x = if drawn.width() > 0.0 { drawn.left() - STATUS_GAP } else { end.right() };
    let mut shown_activity = None;
    let mut button = None;
    let mut act = None;
    if let Some(activity) = activity {
        let (words, b, a, left) = show_activity(ui, t, id, activity, x, mid);
        shown_activity = Some(words);
        if let Some(b) = b {
            if b.clicked() {
                act = a;
            }
            button = Some(b);
        }
        x = left - STATUS_GAP;
    }

    // The left: the block, then the facts that fit.
    let (text_colour, block_fill) = match colour {
        Some((fill, on)) => (on, Some(fill)),
        None => (t.text, None),
    };
    let name_job = |width: f32| {
        let mut job =
            if connection.own { Text::Label.job(connection.name, text_colour) } else { Text::Label.job_as_written(connection.name, text_colour) };
        job.wrap = TextWrapping::truncate_at_width(width.max(1.0));
        job
    };
    let mut name = Piece::new(ui, name_job(STATUS_NAME_MAX), connection.name);
    let server = facts.server.map(|s| Piece::new(ui, Text::Data.job_as_written(s, t.text_muted), s));
    let link = facts.link.map(|(words, _)| Piece::new(ui, Text::Label.job(words, t.text_muted), words));
    let read_only = facts.read_only.then(|| Piece::new(ui, Text::Label.job("Read-only", t.text_muted), "Read-only"));
    let save = facts.unsaved.then(|| TextButton::new("Save connection"));
    let save_w = save.as_ref().map_or(0.0, |b| b.width(ui));
    let mut kept = [server.is_some(), link.is_some(), read_only.is_some()];
    let widths = [server.as_ref(), link.as_ref(), read_only.as_ref()].map(|f| f.map_or(0.0, Piece::width));
    let room = x - under.left();
    // The block, 20, the facts 12 apart, then 8 and SAVE CONNECTION.
    let used = |kept: &[bool; 3], name_w: f32| {
        let facts: Vec<f32> = (0..3).filter(|i| kept[*i]).map(|i| widths[i]).collect();
        let facts_w = facts.iter().sum::<f32>() + STATUS_FACT_GAP * facts.len().saturating_sub(1) as f32;
        let save_w = if save_w > 0.0 && !facts.is_empty() { STATUS_GAP + save_w } else { save_w };
        name_w + 2.0 * STATUS_BLOCK_PAD_X + STATUS_AFTER_BLOCK + facts_w + save_w
    };
    // The facts give way one by one, then the name is cut.
    for i in 0..3 {
        if used(&kept, name.width()) > room {
            kept[i] = false;
        }
    }
    let over = used(&kept, name.width()) - room;
    if over > 0.0 {
        name = Piece::new(ui, name_job(name.width() - over), connection.name);
    }

    let block = Rect::from_min_size(pos2(under.left(), mid - STATUS_BLOCK_H / 2.0), vec2(name.width() + 2.0 * STATUS_BLOCK_PAD_X, STATUS_BLOCK_H));
    if let Some(fill) = block_fill {
        ui.painter().rect_filled(block, CornerRadius::ZERO, fill);
    }
    let shown_connection = piece(ui, id.with("connection"), &name, pos2(block.left() + STATUS_BLOCK_PAD_X, mid), text_colour);
    let mut x = block.right() + STATUS_AFTER_BLOCK;
    let mut any = false;
    let mut fact = |f: Option<Piece>, keep: bool, salt: &str| {
        f.filter(|_| keep).map(|f| {
            if any {
                x += STATUS_FACT_GAP;
            }
            let r = piece(ui, id.with(salt), &f, pos2(x, mid), t.text_muted);
            x += f.width();
            any = true;
            r
        })
    };
    let server = fact(server, kept[0], "server");
    let link = fact(link, kept[1], "link");
    let read_only = fact(read_only, kept[2], "read-only");
    let link = link.zip(facts.link).map(|(r, (_, hover))| r.hint(hover));
    if any {
        x += STATUS_GAP;
    }
    let save = save.map(|b| {
        let r = ui.put(Rect::from_min_size(pos2(x, mid - STATUS_BLOCK_H / 2.0), vec2(save_w, STATUS_BLOCK_H)), b);
        if r.clicked() {
            act = Some(Act::SaveConnection);
        }
        r
    });
    Shown { connection: shown_connection, server, link, read_only, save, activity: shown_activity, button, act }
}

/// Draws `piece` from `left`, centred on `mid`, in `colour`; it answers to hover as a label.
pub(super) fn piece(ui: &Ui, id: Id, piece: &Piece, at: egui::Pos2, colour: Color32) -> Response {
    let rect = Rect::from_min_size(pos2(at.x, at.y - piece.galley.size().y / 2.0), piece.galley.size());
    ui.painter().galley(rect.min, Arc::clone(&piece.galley), colour);
    let r = ui.interact(rect, id, Sense::hover());
    let says = piece.says.clone();
    r.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, says.clone()));
    r
}

/// The timer's words: minutes, seconds and tenths, `00:04.1`.
pub fn timer(elapsed: Duration) -> String {
    let tenths = elapsed.as_millis() / 100;
    format!("{:02}:{:02}.{}", tenths / 600, tenths / 10 % 60, tenths % 10)
}

/// Draws `activity` ending at `right`, centred on `mid`: its words, its button and what a click on
/// that asks for, and where it starts.
fn show_activity(ui: &mut Ui, t: &Tokens, id: Id, activity: &Activity, right: f32, mid: f32) -> (Response, Option<Response>, Option<Act>, f32) {
    // Each part, left to right: a mark, words, a timer (or today's spinner), a button; with the gap
    // before each.
    enum Part {
        Mark,
        Icon(Icon),
        Words(Piece, Color32),
        Timer(Option<Piece>),
        Button(&'static str, Act),
    }
    let words = |ui: &Ui, s: &str, c: Color32| Part::Words(Piece::new(ui, Text::Label.job(s, c), s), c);
    let clock = |ui: &Ui, e: &Option<Duration>| Part::Timer(e.map(|e| Piece::new(ui, Text::Data.job_as_written(&timer(e), t.text_muted), timer(e))));
    let parts: Vec<(f32, Part)> = match activity {
        Activity::InTransaction => vec![(0.0, Part::Mark), (STATUS_GAP, words(ui, "In transaction", t.text))],
        Activity::Disconnected => vec![(0.0, words(ui, "Disconnected", t.danger)), (STATUS_GAP, Part::Button("Reconnect", Act::Reconnect))],
        Activity::Reconnecting { elapsed } => vec![(0.0, words(ui, "Reconnecting", t.text_muted)), (STATUS_FACT_GAP, clock(ui, elapsed))],
        Activity::Exporting { elapsed } | Activity::Importing { elapsed } => {
            let (icon, says) = if matches!(activity, Activity::Exporting { .. }) { (Icon::Export, "Exporting") } else { (Icon::Import, "Importing") };
            vec![(0.0, Part::Icon(icon)), (STATUS_GAP, words(ui, says, t.text)), (STATUS_GAP, clock(ui, elapsed))]
        }
        Activity::Job { text, elapsed, cancel } => {
            let mut parts = vec![(0.0, Part::Words(Piece::new(ui, text.caps_job(t.text_muted), text.to_string()), t.text_muted))];
            parts.push((STATUS_FACT_GAP, clock(ui, elapsed)));
            if *cancel {
                parts.push((STATUS_FACT_GAP, Part::Button("Cancel", Act::Cancel)));
            }
            parts
        }
    };
    let width_of = |ui: &Ui, part: &Part| match part {
        Part::Mark => TRANSACTION_MARK,
        Part::Icon(icon) => icon.size().x,
        Part::Words(p, _) => p.width(),
        Part::Timer(Some(p)) => p.width(),
        Part::Timer(None) => SPINNER,
        Part::Button(label, _) => TextButton::new(label).width(ui),
    };
    let total: f32 = parts.iter().map(|(gap, part)| gap + width_of(ui, part)).sum();
    let left = right - total;
    let mut x = left;
    let mut said = None;
    let mut button = None;
    let mut asked = None;
    for (gap, part) in parts {
        x += gap;
        let w = width_of(ui, &part);
        match part {
            Part::Mark => {
                let mark = Rect::from_center_size(pos2(x + w / 2.0, mid), egui::Vec2::splat(TRANSACTION_MARK));
                ui.painter().rect_filled(mark, CornerRadius::ZERO, t.staged_strong);
            }
            Part::Icon(icon) => icon.paint(ui.painter(), pos2(x + w / 2.0, mid), t.text),
            Part::Words(p, c) => said = Some(piece(ui, id.with("activity"), &p, pos2(x, mid), c)),
            Part::Timer(Some(p)) => {
                ui.painter().galley(pos2(x, mid - p.galley.size().y / 2.0), p.galley, t.text_muted);
            }
            Part::Timer(None) => Spinner::new().paint(ui, pos2(x + w / 2.0, mid)),
            Part::Button(label, a) => {
                button = Some(ui.put(Rect::from_min_size(pos2(x, mid - STATUS_BLOCK_H / 2.0), vec2(w, STATUS_BLOCK_H)), TextButton::new(label)));
                asked = Some(a);
            }
        }
        x += w;
    }
    let said = said.unwrap_or_else(|| ui.interact(Rect::NOTHING, id.with("activity"), Sense::hover()));
    (said, button, asked, left)
}

#[cfg(test)]
mod tests {
    use egui::Theme;

    use super::*;
    use crate::components::{ROOM, both, fills_in, icon_in, painted, texts};

    /// The page's bar is 1100 wide; the harness holds this much.
    const W: f32 = 760.0;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= 0.6
    }

    /// One frame of a bar `W` wide: its shapes and where it lay.
    fn drawn(theme: Theme, connection: Connection<'_>, facts: Facts<'_>, activity: Option<Activity>) -> (Vec<egui::epaint::ClippedShape>, Rect) {
        let mut at = Rect::NOTHING;
        let shapes = painted(theme, |ui| {
            ui.add_space(ROOM);
            ui.allocate_ui(vec2(W, SLIM_BAR_H), |ui| {
                at = Rect::from_min_size(ui.cursor().min, vec2(W, SLIM_BAR_H));
                bar(ui, &connection, &facts, activity.as_ref(), |_| {});
            });
        });
        (shapes, at)
    }

    /// The words `words` drawn: where, and in what colour.
    fn word(shapes: &[egui::epaint::ClippedShape], words: &str) -> (Rect, Color32) {
        let all = texts(shapes);
        all.iter().find(|(w, _, _)| w == words).map(|(_, r, c)| (*r, *c)).unwrap_or_else(|| panic!("`{words}` drawn: {all:?}"))
    }

    const COLOURS: [ConnectionColor; 8] = [
        ConnectionColor::Red,
        ConnectionColor::Orange,
        ConnectionColor::Yellow,
        ConnectionColor::Green,
        ConnectionColor::Blue,
        ConnectionColor::Purple,
        ConnectionColor::Gray,
        ConnectionColor::None,
    ];

    #[test]
    fn each_connection_block_matches_the_page() {
        // Node 163915 in the status bar (163971): a 30 tall bar on `bg`, the colour as a 2 px edge
        // along its top and as the block under it, from the bar's left end, 28 tall, its name in
        // `label` capitals 12 in from either end, in the colour's own text colour. No colour: a
        // 1 px `line` edge, no block, the name in `text`.
        for (theme, t) in both() {
            for colour in COLOURS {
                let at_ = format!("{theme:?} {colour:?}");
                let (shapes, at) = drawn(theme, Connection { name: "Shop live", colour, own: true }, Facts::default(), None);
                let fills = fills_in(&shapes, at);
                assert_eq!(fills.first(), Some(&(at, t.bg)), "{at_}: the bar on bg");
                let (name, c) = word(&shapes, "SHOP LIVE");
                assert!(texts(&shapes).iter().all(|(w, _, _)| w != "Shop live"), "{at_}: the user's own name in capitals");
                match connection_color(colour).zip(on_connection_color(colour)) {
                    Some((fill, on)) => {
                        assert_eq!(fills.get(1), Some(&(Rect::from_min_size(at.min, vec2(W, 2.0)), fill)), "{at_}: the 2 px edge");
                        let (b, f) = fills.get(2).copied().unwrap_or_else(|| panic!("{at_}: the block: {fills:?}"));
                        assert_eq!(f, fill, "{at_}: the block in the colour");
                        assert!(close(b.left(), at.left()) && close(b.top(), at.top() + 2.0) && close(b.height(), 28.0), "{at_}: the block {b:?}");
                        assert!(close(b.width(), name.width() + 24.0), "{at_}: the block {b:?} 12 either side of {name:?}");
                        assert_eq!(c, on, "{at_}: the name in the colour's text colour");
                        assert!(close(name.left(), b.left() + 12.0) && close(name.center().y, b.center().y), "{at_}: the name {name:?} in {b:?}");
                    }
                    None => {
                        assert_eq!(fills.get(1), Some(&(Rect::from_min_size(at.min, vec2(W, 1.0)), t.line)), "{at_}: the 1 px line edge");
                        assert_eq!(fills.len(), 2, "{at_}: no block: {fills:?}");
                        assert_eq!(c, t.text, "{at_}: the name in text");
                        assert!(close(name.left(), at.left() + 12.0) && close(name.center().y, at.top() + 15.5), "{at_}: the name {name:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn each_activity_matches_the_page() {
        // Node 163955, at the bar's right, 12 from its end, centred under the edge. In
        // transaction: an 8 px `staged_strong` square, 8, the words in `text`. Disconnected: the
        // words in `danger`, 8, RECONNECT (`button/text`). Reconnecting: the words in `text_muted`,
        // 12, the timer in `data`. Exporting and importing: the icon and the words in `text`, 8
        // apart, 8, the timer. A job: the words in `text_muted`, 12, the timer, 12, CANCEL.
        let s = Duration::from_millis;
        for (theme, t) in both() {
            let rows: Vec<(Activity, Vec<(&str, Color32)>, Vec<f32>)> = vec![
                (Activity::InTransaction, vec![("IN TRANSACTION", t.text)], vec![]),
                (Activity::Disconnected, vec![("DISCONNECTED", t.danger), ("RECONNECT", t.text)], vec![8.0 + 10.0]),
                (Activity::Reconnecting { elapsed: Some(s(4100)) }, vec![("RECONNECTING", t.text_muted), ("00:04.1", t.text_muted)], vec![12.0]),
                (Activity::Exporting { elapsed: Some(s(12_400)) }, vec![("EXPORTING", t.text), ("00:12.4", t.text_muted)], vec![8.0]),
                (Activity::Importing { elapsed: Some(s(3900)) }, vec![("IMPORTING", t.text), ("00:03.9", t.text_muted)], vec![8.0]),
                (
                    Activity::Job { text: Words::from("Counting rows"), elapsed: Some(s(1200)), cancel: true },
                    vec![("COUNTING ROWS", t.text_muted), ("00:01.2", t.text_muted), ("CANCEL", t.text)],
                    vec![12.0, 12.0 + 10.0],
                ),
            ];
            for (activity, words, gaps) in rows {
                let at_ = format!("{theme:?} {activity:?}");
                let (shapes, at) =
                    drawn(theme, Connection { name: "Shop live", colour: ConnectionColor::Red, own: true }, Facts::default(), Some(activity.clone()));
                let mid = at.top() + 2.0 + 14.0;
                let drawn: Vec<(Rect, Color32)> = words.iter().map(|(w, _)| word(&shapes, w)).collect();
                for ((w, colour), (r, c)) in words.iter().zip(&drawn) {
                    assert_eq!(c, colour, "{at_}: `{w}`'s colour");
                    assert!(close(r.center().y, mid), "{at_}: `{w}` {r:?} centred under the edge");
                }
                for (k, gap) in gaps.iter().enumerate() {
                    assert!(close(drawn[k + 1].0.left() - drawn[k].0.right(), *gap), "{at_}: {gap} between part {k} and the next: {drawn:?}");
                }
                // The last part ends 12 from the bar's end: a button's words 10 inside it.
                let last = drawn.last().map_or(0.0, |(r, _)| r.right());
                let inset = if words.last().is_some_and(|(w, _)| *w == "RECONNECT" || *w == "CANCEL") { 22.0 } else { 12.0 };
                assert!(close(at.right() - last, inset), "{at_}: ends {inset} from the end: {last} in {at:?}");
                let first = drawn[0].0;
                match activity {
                    Activity::InTransaction => {
                        let mark = Rect::from_center_size(pos2(first.left() - 8.0 - 4.0, mid), egui::Vec2::splat(8.0));
                        assert!(fills_in(&shapes, at).contains(&(mark, t.staged_strong)), "{at_}: the mark {mark:?}");
                    }
                    Activity::Exporting { .. } | Activity::Importing { .. } => {
                        let icon = Rect::from_min_max(pos2(first.left() - 8.0 - 10.0, mid - 4.5), pos2(first.left() - 8.0, mid + 4.5));
                        let (colours, bounds) = icon_in(&shapes, icon.expand(1.0));
                        assert_eq!(colours, [t.text], "{at_}: the icon in text, at {icon:?}");
                        assert!((bounds.center() - icon.center()).length() <= 1.0, "{at_}: the icon {bounds:?} at {icon:?}");
                    }
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn a_job_with_no_timer_shows_todays_spinner_and_keeps_a_names_case() {
        let job = Activity::Job { text: Words::from("Loading ").name("Orders").plain("…"), elapsed: None, cancel: true };
        let (shapes, at) =
            drawn(Theme::Light, Connection { name: "Shop live", colour: ConnectionColor::None, own: true }, Facts::default(), Some(job));
        let all = texts(&shapes);
        let words: Vec<&str> = all.iter().map(|(w, _, _)| w.as_str()).collect();
        assert!(words.contains(&"LOADING ") && words.contains(&"Orders") && words.contains(&"…"), "{words:?}");
        let (cancel, _) = word(&shapes, "CANCEL");
        // The spinner: a 14 px slot 12 before CANCEL's button.
        let slot = Rect::from_min_max(pos2(cancel.left() - 10.0 - 12.0 - SPINNER, at.top()), pos2(cancel.left() - 10.0 - 12.0, at.bottom()));
        let (colours, _) = icon_in(&shapes, slot.expand(1.0));
        assert_eq!(colours, [crate::theme::LIGHT.text_muted], "the spinner in {slot:?}");
    }

    #[test]
    fn the_facts_give_way_in_order_then_the_name_is_cut() {
        // Narrower and narrower: the server version goes first, then the link's security, then
        // READ-ONLY; then the name is cut. SAVE CONNECTION and the activity stay.
        let facts = Facts { server: Some("PostgreSQL 14.24"), link: Some(("Not encrypted", "")), read_only: true, unsaved: true };
        let job = Activity::Job { text: Words::from("Counting rows"), elapsed: None, cancel: true };
        let shown_at = |width: f32| {
            let mut seen = Vec::new();
            let shapes = painted(Theme::Light, |ui| {
                ui.allocate_ui(vec2(width, SLIM_BAR_H), |ui| {
                    bar(ui, &Connection { name: "Shop live replica", colour: ConnectionColor::Red, own: true }, &facts, Some(&job), |_| {});
                });
            });
            for (w, r, _) in texts(&shapes) {
                seen.push((w, r));
            }
            seen
        };
        let has = |seen: &[(String, Rect)], w: &str| seen.iter().any(|(s, _)| s == w);
        let name_w = |seen: &[(String, Rect)]| seen.iter().find(|(s, _)| s == "SHOP LIVE REPLICA").map_or(0.0, |(_, r)| r.width());
        let whole = name_w(&shown_at(760.0));
        let mut gone = Vec::new();
        let mut cut = false;
        // Down to where the name has a few letters left.
        for width in (440..=760).rev().step_by(10) {
            let seen = shown_at(width as f32);
            for (w, r) in &seen {
                assert!(r.right() <= width as f32 + 0.5, "{width}: `{w}` {r:?} inside the bar");
            }
            let mut sorted: Vec<Rect> = seen.iter().map(|(_, r)| *r).collect();
            sorted.sort_by(|a, b| a.left().total_cmp(&b.left()));
            for pair in sorted.windows(2) {
                assert!(pair[0].right() <= pair[1].left() + 0.5, "{width}: nothing drawn over another: {seen:?}");
            }
            for w in ["SAVE CONNECTION", "COUNTING ROWS", "CANCEL"] {
                assert!(has(&seen, w), "{width}: `{w}` stays: {seen:?}");
            }
            assert!(!has(&seen, "POSTGRESQL 14.24"), "{width}: the server version as written");
            let now: Vec<&str> = ["PostgreSQL 14.24", "NOT ENCRYPTED", "READ-ONLY"].into_iter().filter(|w| !has(&seen, w)).collect();
            for w in now {
                if !gone.contains(&w) {
                    gone.push(w);
                }
            }
            if name_w(&seen) < whole - 0.5 {
                cut = true;
                assert_eq!(gone.len(), 3, "{width}: the name is cut only once every fact has gone: {gone:?}");
            }
        }
        assert!(cut, "the name was cut at last");
        assert_eq!(gone, ["PostgreSQL 14.24", "NOT ENCRYPTED", "READ-ONLY"], "in that order");
    }

    #[test]
    fn a_server_version_keeps_its_case() {
        let facts = Facts { server: Some("PostgreSQL 14.24"), ..Facts::default() };
        let made = Connection { name: "postgres@pg14.internal/Shop", colour: ConnectionColor::Red, own: false };
        let (shapes, _) = drawn(Theme::Light, made, facts, None);
        let words: Vec<String> = texts(&shapes).into_iter().map(|(w, _, _)| w).collect();
        assert_eq!(words, ["postgres@pg14.internal/Shop", "PostgreSQL 14.24"], "as written");
    }

    #[test]
    fn the_timer_reads_minutes_seconds_and_tenths() {
        assert_eq!(timer(Duration::from_millis(4100)), "00:04.1");
        assert_eq!(timer(Duration::from_millis(754_960)), "12:34.9");
    }
}
