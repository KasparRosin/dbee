//! The update button (`update`), at the right end of the menu bar: a box as tall as a menu
//! title, its words in the bar buttons' capitals. With no newer release there is none.
//!
//! - `UPDATE 0.4.0 → 0.5.0`, a ringed down arrow before it, in `text`: a click installs;
//! - `UPDATING 42%` (`UPDATING` while there is no percent), in `text_muted`, no button;
//! - `RESTART TO UPDATE`, the round arrow before it, in `text`;
//! - `UPDATE FAILED` in `danger`, no button, the reason on hover, and after it the button `RETRY`;
//! - `DOWNLOAD 0.5.0`, the ringed down arrow before it, in `text`: a click opens the release's
//!   page, for a copy that cannot update itself.
//!
//! A button takes `hover` under the pointer and the focus box from the keyboard. Its words are
//! reported to accessibility as written.

use std::sync::Arc;

use egui::{Align, Color32, CornerRadius, Galley, Layout, Rect, Response, Sense, Ui, UiBuilder, WidgetInfo, WidgetType, pos2, vec2};

use super::icon::Icon;
use super::tooltip::Hint;
use super::{Look, focus, painter};
use crate::theme::size::{MENU_BAR_PAD_X, MENU_TITLE_H, RADIUS_CONTROL, UPDATE_CLEAR, UPDATE_EDGE, UPDATE_ICON_GAP, UPDATE_PAD_X, UPDATE_RETRY_GAP};
use crate::theme::{Text, tokens};

/// The most of a failure's reason the hover says, in characters.
const REASON_MAX: usize = 300;

/// What the button says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateLook {
    /// A newer release dbee installs itself.
    Update { from: String, to: String },
    /// The install runs.
    Updating { percent: Option<u8> },
    /// The release is installed; a restart runs it.
    Restart,
    /// The install failed, for this reason.
    Failed { reason: String },
    /// A newer release this copy cannot install itself.
    Download { to: String },
}

impl UpdateLook {
    /// A stable name for the state: `update`, `updating`, `restart`, `failed`, `download`.
    pub fn id(&self) -> &'static str {
        match self {
            UpdateLook::Update { .. } => "update",
            UpdateLook::Updating { .. } => "updating",
            UpdateLook::Restart => "restart",
            UpdateLook::Failed { .. } => "failed",
            UpdateLook::Download { .. } => "download",
        }
    }

    /// The words as written; they are drawn in capitals.
    pub fn words(&self) -> String {
        match self {
            UpdateLook::Update { from, to } => format!("Update {from} → {to}"),
            UpdateLook::Updating { percent: Some(percent) } => format!("Updating {}%", (*percent).min(100)),
            UpdateLook::Updating { percent: None } => "Updating".to_string(),
            UpdateLook::Restart => "Restart to update".to_string(),
            UpdateLook::Failed { .. } => "Update failed".to_string(),
            UpdateLook::Download { to } => format!("Download {to}"),
        }
    }

    fn icon(&self) -> Option<Icon> {
        match self {
            UpdateLook::Update { .. } | UpdateLook::Download { .. } => Some(Icon::Download),
            UpdateLook::Restart => Some(Icon::Refresh),
            UpdateLook::Updating { .. } | UpdateLook::Failed { .. } => None,
        }
    }

    /// Whether the words are a button.
    fn clickable(&self) -> bool {
        !matches!(self, UpdateLook::Updating { .. } | UpdateLook::Failed { .. })
    }
}

/// The words of the button after `UPDATE FAILED`, as written.
pub const RETRY: &str = "Retry";

/// What [`UpdateButton::show`] drew.
pub struct Shown {
    /// The words: a button, or for `UPDATING` and `UPDATE FAILED` a label.
    pub main: Response,
    /// `RETRY`, after `UPDATE FAILED`.
    pub retry: Option<Response>,
}

/// The update button in one of its states.
pub struct UpdateButton<'a> {
    state: &'a UpdateLook,
    look: Option<Look>,
}

impl<'a> UpdateButton<'a> {
    pub fn new(state: &'a UpdateLook) -> UpdateButton<'a> {
        UpdateButton { state, look: None }
    }

    /// Draws its buttons with `look` whatever their responses say (the showcase only).
    pub fn look(mut self, look: Look) -> UpdateButton<'a> {
        self.look = Some(look);
        self
    }

    /// How wide it draws in `ui`, RETRY and the gap before it counted.
    pub fn width(&self, ui: &Ui) -> f32 {
        let main = cell_parts(ui, self.state.icon(), &self.state.words()).1;
        if matches!(self.state, UpdateLook::Failed { .. }) { main + UPDATE_RETRY_GAP + cell_parts(ui, None, RETRY).1 } else { main }
    }

    /// Where it stands in a menu bar whose titles' row is `row` ([`super::menu::bar`]'s) and end
    /// at `titles_right`: its right end [`UPDATE_EDGE`] from the bar's. `None` when that would
    /// leave less than [`UPDATE_CLEAR`] after the titles: it is then not drawn.
    pub fn place(&self, ui: &Ui, row: Rect, titles_right: f32) -> Option<Rect> {
        let right = row.right() + MENU_BAR_PAD_X - UPDATE_EDGE;
        let left = right - self.width(ui);
        (left.is_finite() && left >= titles_right + UPDATE_CLEAR)
            .then(|| Rect::from_min_max(pos2(left, row.top()), pos2(right, row.top() + MENU_TITLE_H)))
    }

    /// Draws it at its place in a menu bar ([`UpdateButton::place`]), or not at all.
    pub fn show_in_bar(self, ui: &mut Ui, row: Rect, titles_right: f32) -> Option<Shown> {
        let at = self.place(ui, row, titles_right)?;
        Some(ui.scope_builder(UiBuilder::new().max_rect(at).layout(Layout::left_to_right(Align::Min)), |ui| self.show(ui)).inner)
    }

    /// Draws it from `ui`'s cursor, in `ui`'s row: its boxes are items of that row, RETRY
    /// [`UPDATE_RETRY_GAP`] after the words.
    pub fn show(self, ui: &mut Ui) -> Shown {
        let t = tokens(ui.ctx());
        let words = self.state.words();
        let colour = match self.state {
            UpdateLook::Updating { .. } => t.text_muted,
            UpdateLook::Failed { .. } => t.danger,
            _ => t.text,
        };
        let failed = matches!(self.state, UpdateLook::Failed { .. });
        // The row's own gap follows the last box; only the one before RETRY is the button's.
        let row_gap = ui.spacing().item_spacing.x;
        if failed {
            ui.spacing_mut().item_spacing.x = UPDATE_RETRY_GAP;
        }
        let main = cell(ui, self.state.icon(), &words, colour, self.state.clickable(), self.look);
        ui.spacing_mut().item_spacing.x = row_gap;
        let UpdateLook::Failed { reason } = self.state else { return Shown { main, retry: None } };
        let reason: String = if reason.chars().count() > REASON_MAX {
            reason.chars().take(REASON_MAX).chain(std::iter::once('…')).collect()
        } else {
            reason.clone()
        };
        let main = main.hint(reason);
        let retry = cell(ui, None, RETRY, t.text, true, self.look);
        Shown { main, retry: Some(retry) }
    }
}

/// A cell's words in capitals and the width they make with the icon and the padding.
fn cell_parts(ui: &Ui, icon: Option<Icon>, words: &str) -> (Arc<Galley>, f32) {
    let galley = ui.painter().layout_job(Text::Label.job(words, Color32::PLACEHOLDER));
    // egui puts the letter spacing after the last letter too; the width counts none there.
    let words_w = galley.size().x - Text::Label.spacing();
    let icon_w = icon.map_or(0.0, |i| i.size().x + UPDATE_ICON_GAP);
    (galley, (2.0 * UPDATE_PAD_X + icon_w + words_w).ceil())
}

/// One box of the button: `icon` and `words` in `colour`; a button when `clickable`, else a label.
fn cell(ui: &mut Ui, icon: Option<Icon>, words: &str, colour: Color32, clickable: bool, look: Option<Look>) -> Response {
    let (galley, width) = cell_parts(ui, icon, words);
    let (rect, r) = ui.allocate_exact_size(vec2(width, MENU_TITLE_H), if clickable { Sense::click() } else { Sense::hover() });
    let kind = if clickable { WidgetType::Button } else { WidgetType::Label };
    r.widget_info(|| WidgetInfo::labeled(kind, ui.is_enabled(), words));
    if !ui.is_rect_visible(rect) {
        return r;
    }
    let look = look.filter(|_| clickable).unwrap_or_else(|| Look::of(ui, &r));
    let t = tokens(ui.ctx());
    let p = painter(ui);
    if clickable && look.hovered && !look.disabled {
        p.rect_filled(rect, CornerRadius::same(RADIUS_CONTROL), t.hover);
    }
    let colour = if look.disabled { t.text_faint } else { colour };
    let mut x = rect.left() + UPDATE_PAD_X;
    if let Some(icon) = icon {
        icon.paint(&p, pos2(x + icon.size().x / 2.0, rect.center().y), colour);
        x += icon.size().x + UPDATE_ICON_GAP;
    }
    p.galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, colour);
    if clickable && look.focused {
        focus::box_inside(&p, rect);
    }
    r
}

#[cfg(test)]
mod tests {
    use egui::accesskit::Role;
    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::components::{ROOM, both, harness, texts};

    fn states() -> Vec<UpdateLook> {
        vec![
            UpdateLook::Update { from: "0.4.0".into(), to: "0.5.0".into() },
            UpdateLook::Updating { percent: Some(42) },
            UpdateLook::Updating { percent: None },
            UpdateLook::Restart,
            UpdateLook::Failed { reason: "the download's checksum is wrong".into() },
            UpdateLook::Download { to: "0.5.0".into() },
        ]
    }

    /// What one state draws: its texts with their colours, and the boxes it took.
    struct Drawn {
        texts: Vec<(String, Color32)>,
        main: Rect,
        retry: Option<Rect>,
        width: f32,
    }

    fn drawn(theme: egui::Theme, state: UpdateLook) -> Drawn {
        let mut h = harness(theme, (Rect::NOTHING, None, 0.0), move |ui, out: &mut (Rect, Option<Rect>, f32)| {
            ui.add_space(ROOM);
            ui.horizontal(|ui| {
                ui.add_space(ROOM);
                let button = UpdateButton::new(&state);
                out.2 = button.width(ui);
                let shown = button.show(ui);
                out.0 = shown.main.rect;
                out.1 = shown.retry.map(|r| r.rect);
            });
        });
        h.run();
        let texts = texts(&h.output().shapes).into_iter().map(|(words, _, colour)| (words, colour)).collect();
        let (main, retry, width) = *h.state();
        Drawn { texts, main, retry, width }
    }

    #[test]
    fn each_state_says_its_words_in_its_colour() {
        for (theme, t) in both() {
            let expected = [
                ("UPDATE 0.4.0 → 0.5.0", t.text),
                ("UPDATING 42%", t.text_muted),
                ("UPDATING", t.text_muted),
                ("RESTART TO UPDATE", t.text),
                ("UPDATE FAILED", t.danger),
                ("DOWNLOAD 0.5.0", t.text),
            ];
            for (state, (words, colour)) in states().into_iter().zip(expected) {
                let failed = matches!(state, UpdateLook::Failed { .. });
                let d = drawn(theme, state);
                assert_eq!(d.texts.first(), Some(&(words.to_string(), colour)), "{theme:?}");
                assert_eq!(d.texts.len(), if failed { 2 } else { 1 }, "{theme:?} {words}: {:?}", d.texts);
                assert_eq!(d.main.height(), MENU_TITLE_H, "{words}: a box as tall as a menu title");
                if failed {
                    assert_eq!(d.texts.get(1), Some(&("RETRY".to_string(), t.text)));
                    let retry = d.retry.unwrap();
                    assert_eq!(retry.left() - d.main.right(), UPDATE_RETRY_GAP);
                    assert_eq!(retry.right() - d.main.left(), d.width, "the width counts RETRY");
                } else {
                    assert_eq!(d.retry, None, "{words}");
                    assert_eq!(d.main.width(), d.width, "{words}");
                }
            }
        }
    }

    #[test]
    fn the_words_are_reported_as_written_and_only_the_buttons_are_buttons() {
        for state in states() {
            let words = state.words();
            let clickable = state.clickable();
            let failed = matches!(state, UpdateLook::Failed { .. });
            let mut h = harness(egui::Theme::Light, (), move |ui, ()| {
                ui.horizontal(|ui| UpdateButton::new(&state).show(ui));
            });
            h.run();
            let role = if clickable { Role::Button } else { Role::Label };
            assert!(h.query_by_role_and_label(role, &words).is_some(), "{words}: {role:?}");
            assert_eq!(h.query_by_role_and_label(Role::Button, "Retry").is_some(), failed, "{words}");
        }
        assert_eq!(UpdateLook::Updating { percent: Some(250) }.words(), "Updating 100%");
        assert_eq!(states().iter().map(UpdateLook::id).collect::<Vec<_>>(), ["update", "updating", "updating", "restart", "failed", "download"]);
    }

    #[test]
    fn a_click_reaches_the_button_and_retry_but_not_the_labels() {
        for state in states() {
            let words = state.words();
            let clickable = state.clickable();
            let failed = matches!(state, UpdateLook::Failed { .. });
            let mut h = harness(egui::Theme::Light, (0u32, 0u32), move |ui, clicks: &mut (u32, u32)| {
                ui.horizontal(|ui| {
                    let shown = UpdateButton::new(&state).show(ui);
                    clicks.0 += u32::from(shown.main.clicked());
                    clicks.1 += u32::from(shown.retry.is_some_and(|r| r.clicked()));
                });
            });
            h.run();
            h.get_by_label(&words).click();
            h.run();
            assert_eq!(h.state().0, u32::from(clickable), "{words}");
            if failed {
                h.get_by_label("Retry").click();
                h.run();
                assert_eq!(h.state().1, 1);
            }
        }
    }

    #[test]
    fn the_failure_says_why_on_hover() {
        let state = UpdateLook::Failed { reason: "the download's checksum is wrong".into() };
        let mut h = harness(egui::Theme::Light, (), move |ui, ()| {
            ui.horizontal(|ui| UpdateButton::new(&state).show(ui));
        });
        h.run();
        h.get_by_label("Update failed").hover();
        h.run();
        h.run();
        let said: Vec<String> = texts(&h.output().shapes).into_iter().map(|(w, _, _)| w).collect();
        assert!(said.iter().any(|w| w == "the download's checksum is wrong"), "{said:?}");
        // A very long reason is cut.
        let long = UpdateLook::Failed { reason: "x".repeat(5000) };
        let mut h = harness(egui::Theme::Light, (), move |ui, ()| {
            ui.horizontal(|ui| UpdateButton::new(&long).show(ui));
        });
        h.run();
        h.get_by_label("Update failed").hover();
        h.run();
        h.run();
        let longest = texts(&h.output().shapes).into_iter().map(|(w, _, _)| w.chars().count()).max().unwrap_or(0);
        assert_eq!(longest, REASON_MAX + 1);
    }

    #[test]
    fn it_stands_twelve_from_the_bars_end_and_gives_way_to_the_titles() {
        let state = UpdateLook::Download { to: "0.5.0".into() };
        let mut h = harness(egui::Theme::Light, Vec::new(), move |ui, out: &mut Vec<(f32, Option<Rect>)>| {
            out.clear();
            let button = UpdateButton::new(&state);
            let width = button.width(ui);
            // A bar from 0 to 400, whose titles' row is 4 in from either end.
            let row = Rect::from_min_max(pos2(MENU_BAR_PAD_X, 3.0), pos2(400.0 - MENU_BAR_PAD_X, 3.0 + MENU_TITLE_H));
            for titles_right in [180.0, 400.0 - UPDATE_EDGE - width - UPDATE_CLEAR, 400.0 - UPDATE_EDGE - width - UPDATE_CLEAR + 1.0, 390.0] {
                out.push((width, button.place(ui, row, titles_right)));
            }
        });
        h.run();
        let placed = h.state().clone();
        let (width, first) = placed[0];
        let first = first.unwrap();
        assert_eq!(first.right(), 400.0 - UPDATE_EDGE);
        assert_eq!(first.width(), width);
        assert_eq!((first.top(), first.height()), (3.0, MENU_TITLE_H));
        assert!(placed[1].1.is_some(), "exactly the clear room is enough");
        assert_eq!(placed[2].1, None, "a point less and it is not drawn");
        assert_eq!(placed[3].1, None);
    }
}
