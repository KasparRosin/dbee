//! The dialog (`dialog`) over the scrim (`scrim`): every dialog in the app is this card, centred
//! near the top of the window over a scrim that covers the window. A click on the scrim does
//! nothing; Escape presses the dialog's cancel button.
//!
//! A dialog is laid out top to bottom: its title (`title`) and, 6 under it, its text (`body`, the
//! names in it in `data`, wrapping); 20 under that its body (a field, a checkbox: whatever the screen puts there, 12
//! apart); 20 under that its buttons, the first on the right, 8 apart, each at least 80 wide.
//! The buttons are drawn from the left, so Tab reaches them in the order they read.

use egui::{Align, Align2, Context, CornerRadius, Frame, Id, Key, Label, Layout, Margin, Modal, Modifiers, Response, Stroke, Ui, vec2};

use super::button::BoxButton;
use crate::theme::size::{
    BUTTON_H, DIALOG_BODY_GAP, DIALOG_BUTTON_GAP, DIALOG_BUTTON_MIN_W, DIALOG_GAP, DIALOG_HEAD_GAP, DIALOG_PAD, DIALOG_W, RADIUS_FLOAT,
};
use crate::theme::{Text, Tokens, Words, float_shadow, tokens};

/// The dialog's edge.
const EDGE: f32 = 1.0;
/// How far down the window a dialog's top sits: this share of the window's height, at most `TOP`.
const TOP_SHARE: f32 = 0.18;
const TOP: f32 = 140.0;

/// How a dialog's button looks: the main answer (`ink`), a destructive one (`danger`), or the
/// others (an outline: Cancel).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Primary,
    Outline,
    Danger,
}

/// One of a dialog's buttons: its words, its look, whether it is on, and the automation id the
/// screen tags it with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Button {
    pub label: &'static str,
    pub style: Style,
    pub enabled: bool,
    pub tag: &'static str,
}

impl Button {
    pub fn primary(label: &'static str, tag: &'static str) -> Button {
        Button { label, style: Style::Primary, enabled: true, tag }
    }

    pub fn outline(label: &'static str, tag: &'static str) -> Button {
        Button { label, style: Style::Outline, enabled: true, tag }
    }

    pub fn danger(label: &'static str, tag: &'static str) -> Button {
        Button { label, style: Style::Danger, enabled: true, tag }
    }

    /// On, or off.
    pub fn enabled(mut self, on: bool) -> Button {
        self.enabled = on;
        self
    }

    fn boxed(&self) -> BoxButton<'static> {
        let button = match self.style {
            Style::Primary => BoxButton::primary(self.label),
            Style::Outline => BoxButton::outline(self.label),
            Style::Danger => BoxButton::danger(self.label),
        };
        button.min_width(DIALOG_BUTTON_MIN_W)
    }
}

/// What a dialog drew this frame.
pub struct Shown {
    /// The button pressed: clicked, or Enter or Space on it, or Escape for the last one.
    pub pressed: Option<usize>,
    /// Its text, if it has one.
    pub text: Option<Response>,
    /// Each button as drawn (on or off as the body left it), in the order given.
    pub buttons: Vec<(Button, Response)>,
}

type Body<'a> = Box<dyn FnOnce(&mut Ui, &mut [Button]) + 'a>;

/// A dialog: a title, a text, a body (fields, checkboxes) and its buttons.
pub struct Dialog<'a> {
    id: Id,
    title: &'a str,
    text: Option<Words>,
    body: Option<Body<'a>>,
    buttons: Vec<Button>,
}

impl<'a> Dialog<'a> {
    pub fn new(id: Id, title: &'a str) -> Dialog<'a> {
        Dialog { id, title, text: None, body: None, buttons: Vec::new() }
    }

    /// The words under the title.
    pub fn text(mut self, words: impl Into<Words>) -> Dialog<'a> {
        self.text = Some(words.into());
        self
    }

    /// What goes between the text and the buttons. It gets the buttons too, to turn one on or off
    /// by what was typed this frame.
    pub fn body(mut self, add: impl FnOnce(&mut Ui, &mut [Button]) + 'a) -> Dialog<'a> {
        self.body = Some(Box::new(add));
        self
    }

    /// The buttons, the first on the right; the last one is the cancel (Escape presses it).
    pub fn buttons(mut self, buttons: impl Into<Vec<Button>>) -> Dialog<'a> {
        self.buttons = buttons.into();
        self
    }

    /// Shows it over the scrim, centred near the top of the window.
    pub fn show(self, ctx: &Context) -> Shown {
        let t = tokens(ctx);
        let window = ctx.content_rect();
        // A fixed top left corner: egui's first frame of it (its sizing pass) puts every widget
        // where the frames after it do, so a click on a rect from `ui.inspect` lands.
        let corner = vec2(((window.width() - DIALOG_W) / 2.0).max(0.0), (window.height() * TOP_SHARE).min(TOP));
        // The sizing pass lays the content out in the default size: all the height below the
        // corner, so no row gets less room than in the frames after it.
        let below = (window.height() - corner.y).max(1.0);
        // No fade-in: a screenshot right after a dialog opens shows it as it is.
        let area = Modal::default_area(self.id).anchor(Align2::LEFT_TOP, corner).default_size(vec2(DIALOG_W, below)).fade_in(false);
        let cancel = self.buttons.len().checked_sub(1);
        let modal = Modal::new(self.id).area(area).frame(frame(t)).backdrop_color(t.scrim).show(ctx, |ui| self.content(ui));
        let mut shown = modal.inner;
        // A click on the scrim (the modal's backdrop) does nothing.
        let escape = || ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        if shown.pressed.is_none() && modal.is_top_modal && !modal.any_popup_open && escape() {
            shown.pressed = cancel;
        }
        shown
    }

    /// Draws it where `ui` puts it, with no scrim (the showcase).
    pub fn in_place(self, ui: &mut Ui) -> Shown {
        let t = tokens(ui.ctx());
        frame(t).show(ui, |ui| self.content(ui)).inner
    }

    fn content(self, ui: &mut Ui) -> Shown {
        let Dialog { title, text, body, mut buttons, .. } = self;
        let width = inside_width();
        ui.set_width(width);
        ui.with_layout(Layout::top_down(Align::Min), |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.add(Label::new(Text::Title.rich_as_written(title)).wrap());
            let text = text.map(|words| {
                ui.add_space(DIALOG_HEAD_GAP);
                ui.add(Label::new(words.job(tokens(ui.ctx()).text)).wrap())
            });
            if let Some(body) = body {
                ui.add_space(DIALOG_GAP);
                ui.scope(|ui| {
                    ui.spacing_mut().item_spacing.y = DIALOG_BODY_GAP;
                    body(ui, &mut buttons);
                });
            }
            ui.add_space(DIALOG_GAP);
            let drawn = row(ui, &buttons, width);
            let pressed = drawn.iter().position(Response::clicked);
            Shown { pressed, text, buttons: buttons.into_iter().zip(drawn).collect() }
        })
        .inner
    }
}

/// The width inside the dialog's edge and padding.
fn inside_width() -> f32 {
    DIALOG_W - 2.0 * (f32::from(DIALOG_PAD) + EDGE)
}

/// The dialog's card: `raised`, a 1 px `line` edge, corners 4, the shadow; its content 20 inside
/// the edge. egui counts the edge into the card's size, outside the padding.
fn frame(t: &Tokens) -> Frame {
    Frame::new()
        .fill(t.raised)
        .stroke(Stroke::new(EDGE, t.line))
        .shadow(float_shadow(t))
        .corner_radius(CornerRadius::same(RADIUS_FLOAT))
        .inner_margin(Margin::same(DIALOG_PAD))
}

/// The buttons on one row `width` wide, the first on the right; their responses in the order
/// given. Drawn from the left, so Tab reaches them as they read.
fn row(ui: &mut Ui, buttons: &[Button], width: f32) -> Vec<Response> {
    let boxed: Vec<BoxButton<'_>> = buttons.iter().map(Button::boxed).collect();
    let widths: f32 = boxed.iter().map(|b| b.width(ui)).sum();
    let gaps = DIALOG_BUTTON_GAP * buttons.len().saturating_sub(1) as f32;
    // Top-aligned: egui centres nothing in a dialog's first frame (its sizing pass), so a centred
    // row would sit a pixel off there.
    ui.allocate_ui_with_layout(vec2(width, BUTTON_H), Layout::left_to_right(Align::Min), |ui| {
        ui.spacing_mut().item_spacing.x = DIALOG_BUTTON_GAP;
        ui.add_space((width - widths - gaps).max(0.0));
        let mut drawn: Vec<Option<Response>> = vec![None; buttons.len()];
        for (i, (b, button)) in buttons.iter().zip(boxed).enumerate().rev() {
            drawn[i] = Some(ui.add_enabled(b.enabled, button));
        }
        drawn.into_iter().flatten().collect()
    })
    .inner
}

#[cfg(test)]
mod tests {
    use egui::epaint::RectShape;
    use egui::{Color32, CornerRadius, Key, Pos2, Rect, Shape, Theme, Vec2, pos2, vec2};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::components::button::BoxButton;
    use crate::components::checkbox::Checkbox;
    use crate::components::input::{Field, Input};
    use crate::components::{Edge, both, edges, flat, showcase, texts};
    use crate::theme::Text;

    const WINDOW: Vec2 = vec2(900.0, 600.0);
    /// Where the page puts a 440 dialog in this window: centred, 18 % of the height down (at most
    /// 140), as today's dialogs sit.
    const AT: Pos2 = pos2(230.0, 108.0);
    const CLOSE: &str = "Close Query 1? Its query text will be lost.";
    const PASSWORD: &str = "Enter the password of shop on pg14.internal.";
    const URL: &str = "Paste a database URL to fill in the form.";
    const COMMIT: &str = "Query 1 has changes that aren't committed.";
    const LONG: &str = "Connection lost during Apply: the server closed the connection unexpectedly. This probably \
                        means the server terminated abnormally before or while processing the request. Reset and \
                        reload to check what was written, or Apply again to retry.";

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= 0.5
    }

    /// An engine's reason a URL didn't parse: three lines at the field's width.
    const LONG_ERROR: &str = "invalid connection URL: expected a postgres://, postgresql://, mysql:// or mariadb:// URL, but this one \
                              starts with shop@pg14. Paste the whole URL your provider gave you, its scheme and two slashes first.";

    /// The page's five contents (node 162771), a confirm with a long message and a field with a
    /// long error.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Content {
        Confirm,
        Field,
        FieldAndCheckbox,
        FieldWithError,
        ThreeButtons,
        Long,
        LongError,
    }

    const FIVE: [Content; 5] = [Content::Confirm, Content::Field, Content::FieldAndCheckbox, Content::FieldWithError, Content::ThreeButtons];

    /// What the harness keeps: the dialog's content, what its body holds, and what it drew.
    #[derive(Default)]
    struct Seen {
        content: Option<Content>,
        typed: String,
        save: bool,
        /// The body's input, checkbox and error line.
        field: Option<Rect>,
        checkbox: Option<Rect>,
        line: Option<Rect>,
        text: Option<Rect>,
        buttons: Vec<(Button, Rect)>,
        pressed: Vec<usize>,
        /// Clicks on a button behind the scrim.
        behind: usize,
    }

    fn dialog(ctx: &Context, content: Content, s: &mut Seen) -> Shown {
        let Seen { typed, save, field, checkbox, line, .. } = s;
        let id = Id::new("dialog-test");
        let two = |main: Button| [main, Button::outline("Cancel", "dialog-cancel")];
        let d = match content {
            Content::Confirm => Dialog::new(id, "Close anyway?").text(CLOSE).buttons(two(Button::danger("Close anyway", "dialog-ok"))),
            Content::Long => Dialog::new(id, "Apply again?").text(LONG).buttons(two(Button::danger("Apply again", "dialog-ok"))),
            // Connect is on once something is typed.
            Content::Field => Dialog::new(id, "Database password")
                .text(PASSWORD)
                .body(|ui, buttons| {
                    *field = Some(ui.add(Input::body(typed).password(true)).rect);
                    buttons[0].enabled = !typed.is_empty();
                })
                .buttons(two(Button::primary("Connect", "dialog-ok").enabled(false))),
            Content::FieldAndCheckbox => Dialog::new(id, "Database password")
                .text(PASSWORD)
                .body(|ui, _| {
                    *field = Some(ui.add(Input::body(typed).password(true)).rect);
                    *checkbox = Some(ui.add(Checkbox::new(save, "Save in keyring")).rect);
                })
                .buttons(two(Button::primary("Connect", "dialog-ok"))),
            Content::FieldWithError | Content::LongError => Dialog::new(id, "Fill from URL")
                .text(URL)
                .body(move |ui, _| {
                    let error = if content == Content::LongError { LONG_ERROR } else { "Not a database URL" };
                    let shown = Field::unlabelled(Input::data(typed).error(true)).error(Some(error)).keep_error_room(true).show(ui);
                    *field = Some(shown.input.rect);
                    *line = shown.error.map(|r| r.rect);
                })
                .buttons(two(Button::primary("Fill", "dialog-ok"))),
            Content::ThreeButtons => Dialog::new(id, "Commit the transaction?").text(COMMIT).buttons([
                Button::primary("Commit", "commit"),
                Button::outline("Rollback", "rollback"),
                Button::outline("Cancel", "cancel"),
            ]),
        };
        d.show(ctx)
    }

    /// A window drawing `content`'s dialog in `theme` over a button ("Behind") at its top left,
    /// its field holding `typed`.
    fn open(theme: Theme, content: Content) -> Harness<'static, Seen> {
        open_with(theme, content, if matches!(content, Content::FieldWithError | Content::LongError) { "shop@pg14:5432" } else { "secretpass" })
    }

    fn open_with(theme: Theme, content: Content, typed: &str) -> Harness<'static, Seen> {
        let mut ready = false;
        let mut h = Harness::builder().with_size(WINDOW).build_ui_state(
            move |ui, s: &mut Seen| {
                if !ready {
                    showcase::prepare(ui.ctx(), theme);
                    ready = true;
                    ui.ctx().request_repaint();
                    return;
                }
                if ui.add(BoxButton::outline("Behind")).clicked() {
                    s.behind += 1;
                }
                let Some(content) = s.content else { return };
                let ctx = ui.ctx().clone();
                let shown = dialog(&ctx, content, s);
                s.text = shown.text.map(|r| r.rect);
                s.buttons = shown.buttons.iter().map(|(b, r)| (*b, r.rect)).collect();
                s.pressed.extend(shown.pressed);
            },
            Seen { content: Some(content), typed: typed.to_owned(), ..Seen::default() },
        );
        h.run();
        h
    }

    /// The rect drawn with `fill` and a `width` px edge in `colour`: the dialog's frame.
    fn frame(shapes: &[Shape], fill: Color32, colour: Color32) -> Option<RectShape> {
        shapes.iter().find_map(|s| match s {
            Shape::Rect(r) if r.fill == fill && r.stroke.width == 1.0 && r.stroke.color == colour && r.blur_width == 0.0 => Some(r.clone()),
            _ => None,
        })
    }

    #[test]
    fn each_dialog_matches_the_page() {
        // Node 162771: 440 wide, `raised`, a 1 px `line` edge, corners 4, the shadow (0 6, the
        // page's blur 18, egui's 36); 20 inside the edge; the title (`title`) and 6 under it the
        // text (`body`), both `text`; 20 to the body (a field, 12 to a checkbox; a field's error
        // line under it), 20 to the buttons: 28 tall, at least 80 wide, 8 apart, the first on the
        // right. Over a `scrim` covering the window, the dialog centred near the top.
        for (theme, t) in both() {
            for content in FIVE {
                let at = format!("{theme:?} {content:?}");
                let h = open(theme, content);
                let shapes = flat(&h.output().shapes);
                let s = h.state();
                let window = Rect::from_min_size(Pos2::ZERO, WINDOW);
                assert!(
                    shapes.iter().any(|sh| matches!(sh, Shape::Rect(r) if r.rect == window && r.fill == t.scrim)),
                    "{at}: the scrim over the window"
                );
                let edge = edges(&h.output().shapes)
                    .into_iter()
                    .find(|e| e.colour == t.line && e.width == 1.0 && e.radius == CornerRadius::same(4))
                    .unwrap_or_else(|| panic!("{at}: the dialog's 1 px line edge, corners 4"));
                let outer = edge.outer;
                assert!(frame(&shapes, t.raised, t.line).is_some(), "{at}: on raised");
                assert_eq!(outer.min, AT, "{at}: centred near the top: {outer:?}");
                assert_eq!(outer.width(), 440.0, "{at}: 440 wide");
                let shadow = shapes.iter().find_map(|sh| match sh {
                    Shape::Rect(r) if r.blur_width > 0.0 && r.fill == t.shadow => Some(r.clone()),
                    _ => None,
                });
                let shadow = shadow.unwrap_or_else(|| panic!("{at}: the shadow"));
                assert_eq!(shadow.blur_width, f32::from(float_shadow(&t).blur), "{at}: the one shadow's blur");
                assert!(close(shadow.rect.top(), outer.top() + 6.0) && close(shadow.rect.left(), outer.left()), "{at}: 6 down: {:?}", shadow.rect);
                let inside = outer.shrink(21.0);
                // The title and the text.
                let drawn = texts(&h.output().shapes);
                let title = match content {
                    Content::Confirm | Content::Long => "Close anyway?",
                    Content::Field | Content::FieldAndCheckbox => "Database password",
                    Content::FieldWithError | Content::LongError => "Fill from URL",
                    Content::ThreeButtons => "Commit the transaction?",
                };
                let (_, title_at, colour) = drawn.iter().find(|(w, _, _)| w == title).cloned().unwrap_or_else(|| panic!("{at}: the title"));
                assert_eq!(colour, t.text, "{at}: the title in text");
                assert!(close(title_at.left(), inside.left()) && close(title_at.top(), inside.top()), "{at}: the title 21 in: {title_at:?}");
                assert!(close(title_at.height(), Text::Title.line()), "{at}: the title's 24 line");
                let fonts = font_of(&h, title);
                assert_eq!(fonts, Some(Text::Title.font()), "{at}: the title in title");
                let text = s.text.unwrap_or_else(|| panic!("{at}: the text"));
                assert!(close(text.top(), title_at.top() + 30.0) && close(text.left(), inside.left()), "{at}: the text 6 under the title: {text:?}");
                assert!(close(text.height(), Text::Body.line()), "{at}: one 18 line: {text:?}");
                let words = match content {
                    Content::Confirm | Content::Long => CLOSE,
                    Content::Field | Content::FieldAndCheckbox => PASSWORD,
                    Content::FieldWithError | Content::LongError => URL,
                    Content::ThreeButtons => COMMIT,
                };
                assert_eq!(font_of(&h, words), Some(Text::Body.font()), "{at}: the text in body");
                assert_eq!(drawn.iter().find(|(w, _, _)| w == words).map(|(_, _, c)| *c), Some(t.text), "{at}: the text in text");
                // The body.
                let mut bottom = text.bottom();
                if let Some(field) = s.field {
                    assert!(close(field.top(), bottom + 20.0), "{at}: the field 20 under the text: {field:?}");
                    assert!(
                        close(field.left(), inside.left()) && close(field.width(), 398.0),
                        "{at}: the field as wide as the dialog's inside: {field:?}"
                    );
                    bottom = field.bottom();
                }
                if let Some(checkbox) = s.checkbox {
                    assert!(close(checkbox.top(), bottom + 12.0), "{at}: the checkbox 12 under the field: {checkbox:?}");
                    bottom = checkbox.bottom();
                }
                if content == Content::FieldWithError {
                    let line = s.line.unwrap_or_else(|| panic!("{at}: the error line"));
                    assert!(close(line.top(), bottom + 4.0), "{at}: the field's error line under it: {line:?}");
                    bottom = line.bottom();
                }
                // The buttons, the first on the right.
                let mut right = inside.right();
                for (b, r) in &s.buttons {
                    let at = format!("{at} {}", b.label);
                    assert!(close(r.right(), right), "{at}: {r:?} ends at {right}");
                    assert!(close(r.top(), bottom + 20.0) && close(r.bottom(), inside.bottom()), "{at}: 20 under the body, 20 above the edge: {r:?}");
                    assert_eq!(r.height(), 28.0, "{at}: 28 tall");
                    assert!(r.width() >= 80.0, "{at}: at least 80 wide: {r:?}");
                    if ["Cancel", "Connect", "Fill", "Commit"].contains(&b.label) {
                        assert_eq!(r.width(), 80.0, "{at}: 80 wide");
                    }
                    let fills: Vec<Color32> = shapes
                        .iter()
                        .filter_map(|sh| match sh {
                            Shape::Rect(x) if x.rect == *r && x.fill.a() > 0 => Some(x.fill),
                            _ => None,
                        })
                        .collect();
                    let (ground, on) = match b.style {
                        Style::Primary => (Some(t.ink), t.on_ink),
                        Style::Danger => (Some(t.danger), t.on_danger),
                        Style::Outline => (None, t.text),
                    };
                    assert_eq!(fills, ground.into_iter().collect::<Vec<_>>(), "{at}: its ground");
                    if ground.is_none() {
                        let line = Edge { outer: *r, width: 1.0, colour: t.line_strong, radius: CornerRadius::same(2) };
                        assert!(edges(&h.output().shapes).contains(&line), "{at}: an outline");
                    }
                    let caps = b.label.to_uppercase();
                    assert_eq!(drawn.iter().find(|(w, _, _)| *w == caps).map(|(_, _, c)| *c), Some(on), "{at}: its words");
                    assert_eq!(font_of(&h, &caps), Some(Text::ButtonCaps.font()), "{at}: in button caps");
                    h.get_by_label(b.label);
                    right = r.left() - 8.0;
                }
                assert!(!s.buttons.is_empty(), "{at}: the buttons");
                let height = match content {
                    Content::Confirm | Content::ThreeButtons => 138.0,
                    Content::Field => 186.0,
                    Content::FieldAndCheckbox => 216.0,
                    Content::FieldWithError => 206.0,
                    Content::Long | Content::LongError => 0.0,
                };
                assert_eq!(outer.height(), height, "{at}: its height");
            }
        }
    }

    /// The font `words` were drawn in, in the last frame.
    fn font_of(h: &Harness<'_, Seen>, words: &str) -> Option<egui::FontId> {
        flat(&h.output().shapes).into_iter().find_map(|s| match s {
            Shape::Text(t) if t.galley.text() == words => Some(t.galley.job.sections[0].format.font_id.clone()),
            _ => None,
        })
    }

    #[test]
    fn a_click_on_the_scrim_presses_nothing() {
        // Node 162837: a click on the scrim does nothing for a dialog, and what is under it gets
        // nothing either.
        let mut h = open(Theme::Light, Content::Confirm);
        let behind = h.get_by_label("Behind").rect().center();
        h.hover_at(behind);
        h.drag_at(behind);
        h.drop_at(behind);
        h.run();
        h.run();
        let s = h.state();
        assert_eq!((s.pressed.clone(), s.behind), (vec![], 0), "nothing pressed, nothing under it clicked");
        assert_eq!(s.buttons.len(), 2, "the dialog is still drawn");
    }

    #[test]
    fn escape_presses_the_last_button() {
        for content in [Content::Confirm, Content::ThreeButtons, Content::Field] {
            let mut h = open(Theme::Light, content);
            h.key_press(Key::Escape);
            h.run();
            let last = h.state().buttons.len() - 1;
            assert_eq!(h.state().pressed, [last], "{content:?}: Escape presses Cancel");
        }
    }

    #[test]
    fn a_click_on_a_button_presses_it() {
        let mut h = open(Theme::Light, Content::ThreeButtons);
        for (label, i) in [("Rollback", 1), ("Commit", 0), ("Cancel", 2)] {
            h.get_by_label(label).click();
            h.run();
            assert_eq!(h.state_mut().pressed.pop(), Some(i), "{label}");
        }
    }

    #[test]
    fn a_long_message_wraps_inside() {
        // Review Focus 3: an engine's long message wraps inside the dialog, which stays 440 wide
        // and grows to hold it, its buttons 20 under it.
        for (theme, t) in both() {
            let h = open(theme, Content::Long);
            let s = h.state();
            let rows = flat(&h.output().shapes).into_iter().find_map(|sh| match sh {
                Shape::Text(x) if x.galley.text() == LONG => Some(x.galley.rows.len()),
                _ => None,
            });
            assert!(rows.is_some_and(|n| n >= 3), "{theme:?}: wrapped: {rows:?} rows");
            let edge = edges(&h.output().shapes).into_iter().find(|e| e.colour == t.line && e.radius == CornerRadius::same(4));
            let outer = edge.unwrap_or_else(|| panic!("{theme:?}: the frame")).outer;
            assert_eq!(outer.width(), 440.0, "{theme:?}: still 440 wide");
            let text = s.text.unwrap_or_else(|| panic!("{theme:?}: the text"));
            assert!(outer.shrink(21.0).contains_rect(text), "{theme:?}: inside: {text:?} in {outer:?}");
            assert!(close(text.width(), 398.0) || text.width() < 398.0, "{theme:?}: within 398: {text:?}");
            let button = s.buttons.first().map(|(_, r)| *r).unwrap_or(Rect::NOTHING);
            assert!(close(button.top(), text.bottom() + 20.0), "{theme:?}: the buttons 20 under it: {button:?}");
        }
    }

    #[test]
    fn a_long_field_error_wraps_and_the_dialog_grows() {
        // The design: a long error wraps under the field, at the field's width, and the dialog
        // grows by its lines; it is never cut, since the reason is what the user needs.
        for (theme, t) in both() {
            let one = open(theme, Content::FieldWithError);
            let height = |h: &Harness<'_, Seen>| {
                let edge = edges(&h.output().shapes).into_iter().find(|e| e.colour == t.line && e.radius == CornerRadius::same(4));
                edge.unwrap_or_else(|| panic!("{theme:?}: the frame")).outer.height()
            };
            let h = open(theme, Content::LongError);
            let drawn = flat(&h.output().shapes).into_iter().find_map(|sh| match sh {
                Shape::Text(x) if x.galley.text() == LONG_ERROR => Some(x),
                _ => None,
            });
            let drawn = drawn.unwrap_or_else(|| panic!("{theme:?}: the error drawn"));
            let rows: Vec<String> = drawn.galley.rows.iter().map(|r| r.text()).collect();
            assert_eq!(rows.len(), 3, "{theme:?}: three lines: {rows:?}");
            assert!(!drawn.galley.elided, "{theme:?}: not cut");
            let shown: String = rows.concat();
            for word in LONG_ERROR.split_whitespace() {
                assert!(shown.contains(word), "{theme:?}: `{word}` drawn: {rows:?}");
            }
            let s = h.state();
            let (field, line) = (s.field.unwrap_or(Rect::NOTHING), s.line.unwrap_or(Rect::NOTHING));
            let at = drawn.galley.rect.translate(drawn.pos.to_vec2());
            assert!(close(at.top(), field.bottom() + 4.0) && close(at.left(), field.left()), "{theme:?}: 4 under the field: {at:?}");
            assert!(at.width() <= field.width() + 0.5, "{theme:?}: at the field's width: {at:?} under {field:?}");
            assert!(line.contains_rect(at), "{theme:?}: its line holds it: {at:?} in {line:?}");
            assert_eq!(height(&h), height(&one) + 2.0 * Text::Small.line(), "{theme:?}: the dialog grew by two lines");
            for (b, r) in &s.buttons {
                assert!(close(r.top(), at.bottom() + 20.0), "{theme:?} {}: 20 under the error, not over it: {r:?}", b.label);
            }
            let words = texts(&h.output().shapes).into_iter().find(|(w, _, _)| w == LONG_ERROR).map(|(_, _, c)| c);
            assert_eq!(words, Some(t.danger), "{theme:?}: in danger");
        }
    }

    #[test]
    fn the_body_can_turn_a_button_on() {
        // A field dialog's main button is on once something is typed, in the frame it is typed,
        // as it was when the field was drawn before the buttons.
        let mut h = open_with(Theme::Light, Content::Field, "");
        let ok = |h: &Harness<'_, Seen>| h.state().buttons.first().map(|(b, _)| b.enabled);
        assert_eq!(ok(&h), Some(false), "nothing typed: Connect is off");
        let field = h.state().field.unwrap_or(Rect::NOTHING);
        h.hover_at(field.center());
        h.drag_at(field.center());
        h.drop_at(field.center());
        h.run();
        h.event(egui::Event::Text("x".into()));
        h.step();
        assert_eq!(ok(&h), Some(true), "typed: Connect is on in that frame");
        let t = crate::theme::LIGHT;
        let drawn = flat(&h.output().shapes);
        let r = h.state().buttons[0].1;
        assert!(drawn.iter().any(|sh| matches!(sh, Shape::Rect(x) if x.rect == r && x.fill == t.ink)), "and drawn on: {r:?}");
    }
}
