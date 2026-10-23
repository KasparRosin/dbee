//! Automation ids: kebab-case names (`conn-new`, `tab-close-3`) for the widgets agents and
//! tests drive.
//!
//! Each frame the screens tag what they draw ([`Tags::tag`]). The rects kept are those of the
//! last drawn frame: `ui.inspect` lists them, and `ui.click {id}` clicks the centre of one. A
//! frame starts the list over ([`Tags::begin_frame`]), so a widget that is no longer drawn is no
//! longer listed. Rects are in window points.
//!
//! A rect is the part of the widget a click can reach: inside a scroll area, the part in view. A
//! widget scrolled entirely out of view is listed with `visible: false` and its whole rect; a
//! click must not go there (another widget is drawn in that place), so `ui.click` first asks for
//! it to be scrolled into view ([`Tags::scroll_to`]): the next frame that tags it scrolls to it.
//!
//! A tag can carry what the widget shows ([`Tags::tag_value`]): a text field's text, a check
//! box's state, the choice of a selector, a message's text. A secret field (a password, a
//! passphrase, a URL that can hold one) is tagged with [`Tags::tag_secret`]: only whether it is
//! filled in, never its text. A disabled widget says so. In the first frame of a dialog or a
//! menu (egui's sizing pass) every widget reads as disabled, so those are tagged with the state
//! their code knows ([`Tags::tag_value_as`]).

use std::collections::HashMap;

use egui::{Rect, Response};
use serde_json::{Map, Value as Json, json};

/// What one tag shows besides its place.
#[derive(Debug, Clone, PartialEq)]
enum Shows {
    Nothing,
    Value(Json),
    /// A secret field, and whether it is filled in.
    Secret(bool),
}

#[derive(Debug, Clone)]
struct Entry {
    id: String,
    /// The part in view, or the whole widget when none of it is.
    rect: Rect,
    visible: bool,
    shows: Shows,
    enabled: bool,
}

/// The tagged widgets of the last drawn frame, in drawing order.
#[derive(Debug, Default)]
pub struct Tags {
    entries: Vec<Entry>,
    index: HashMap<String, usize>,
    /// The widget to scroll into view when it is next tagged.
    scroll_to: Option<String>,
}

impl Tags {
    /// Forgets the last frame's tags; the screens tag this frame's widgets next.
    pub fn begin_frame(&mut self) {
        self.entries.clear();
        self.index.clear();
    }

    fn put(&mut self, id: &str, r: &Response, shows: Shows) {
        self.put_as(id, r, shows, r.enabled());
    }

    fn put_as(&mut self, id: &str, r: &Response, shows: Shows, enabled: bool) {
        if self.scroll_to.as_deref() == Some(id) {
            self.scroll_to = None;
            r.scroll_to_me_animation(Some(egui::Align::Center), egui::style::ScrollAnimation::none());
        }
        let seen = r.interact_rect;
        let visible = seen.is_finite() && seen.width() > 0.0 && seen.height() > 0.0;
        let rect = if visible { seen } else { r.rect };
        let entry = Entry { id: id.to_string(), rect, visible, shows, enabled };
        match self.index.get(id).and_then(|&i| self.entries.get_mut(i)) {
            Some(slot) => *slot = entry,
            None => {
                self.index.insert(id.to_string(), self.entries.len());
                self.entries.push(entry);
            }
        }
    }

    /// Tags `r` as `id`. The same id twice in a frame keeps the later widget.
    pub fn tag(&mut self, id: &str, r: &Response) {
        self.put(id, r, Shows::Nothing);
    }

    /// Tags `r` as `id`, showing `value` (text, a flag, a choice). Never a secret: those go
    /// through [`Tags::tag_secret`].
    pub fn tag_value(&mut self, id: &str, r: &Response, value: impl Into<Json>) {
        self.put(id, r, Shows::Value(value.into()));
    }

    /// Tags the secret field `r` as `id`: listed as masked, with only whether it is `set`.
    pub fn tag_secret(&mut self, id: &str, r: &Response, set: bool) {
        self.put(id, r, Shows::Secret(set));
    }

    /// [`Tags::tag_value`] for a widget whose code knows whether it is `enabled`: one in a
    /// dialog or a menu, which egui draws disabled in their first frame (the sizing pass).
    pub fn tag_value_as(&mut self, id: &str, r: &Response, value: impl Into<Json>, enabled: bool) {
        self.put_as(id, r, Shows::Value(value.into()), enabled);
    }

    /// [`Tags::tag_secret`] for a widget whose code knows whether it is `enabled`.
    pub fn tag_secret_as(&mut self, id: &str, r: &Response, set: bool, enabled: bool) {
        self.put_as(id, r, Shows::Secret(set), enabled);
    }

    /// Where `id` can be clicked: the part of it in view; `None` when it was not drawn or is
    /// scrolled out of view.
    pub fn rect(&self, id: &str) -> Option<Rect> {
        self.entry(id).filter(|e| e.visible).map(|e| e.rect)
    }

    /// `id` was drawn, but scrolled out of view.
    pub fn hidden(&self, id: &str) -> bool {
        self.entry(id).is_some_and(|e| !e.visible)
    }

    /// Scrolls `id` into view in the next frame that tags it.
    pub fn scroll_to(&mut self, id: &str) {
        self.scroll_to = Some(id.to_string());
    }

    /// The widget waiting to be scrolled into view, if any.
    pub fn scroll_pending(&self) -> Option<&str> {
        self.scroll_to.as_deref()
    }

    /// Forgets the request to scroll `id` into view (a click that gave up waiting for it).
    pub fn cancel_scroll(&mut self, id: &str) {
        if self.scroll_to.as_deref() == Some(id) {
            self.scroll_to = None;
        }
    }

    /// What `id` showed (`None` for a tag without a value, and for secrets).
    pub fn value(&self, id: &str) -> Option<&Json> {
        match &self.entry(id)?.shows {
            Shows::Value(v) => Some(v),
            Shows::Nothing | Shows::Secret(_) => None,
        }
    }

    fn entry(&self, id: &str) -> Option<&Entry> {
        self.index.get(id).and_then(|&i| self.entries.get(i))
    }

    /// The tagged ids, in drawing order.
    pub fn ids(&self) -> Vec<&str> {
        self.entries.iter().map(|e| e.id.as_str()).collect()
    }

    /// `[{id, rect: [x, y, width, height], visible?, value?, masked?, set?, enabled?}]`, as
    /// `ui.inspect` lists them: `visible: false` for a widget scrolled out of view, `value` for a
    /// widget that shows one, `masked: true` and `set` for a secret field, `enabled: false` for a
    /// disabled widget.
    pub fn to_json(&self) -> Json {
        Json::Array(
            self.entries
                .iter()
                .map(|e| {
                    let r = e.rect;
                    let mut m = Map::new();
                    m.insert("id".into(), json!(e.id));
                    m.insert("rect".into(), json!([r.min.x, r.min.y, r.width(), r.height()]));
                    if !e.visible {
                        m.insert("visible".into(), json!(false));
                    }
                    match &e.shows {
                        Shows::Nothing => {}
                        Shows::Value(v) => {
                            m.insert("value".into(), v.clone());
                        }
                        Shows::Secret(set) => {
                            m.insert("masked".into(), json!(true));
                            m.insert("set".into(), json!(set));
                        }
                    }
                    if !e.enabled {
                        m.insert("enabled".into(), json!(false));
                    }
                    Json::Object(m)
                })
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_keep_the_last_drawn_frame() {
        let ctx = egui::Context::default();
        let mut tags = Tags::default();
        ctx.run_ui(egui::RawInput::default(), |ui| {
            tags.begin_frame();
            let r = ui.button("one");
            tags.tag("one", &r);
            let r = ui.button("two");
            tags.tag("two", &r);
            // The same id again: the later widget wins.
            let r = ui.button("two, again");
            tags.tag("two", &r);
        })
        .drop_without_applying_deltas();
        assert_eq!(tags.ids(), vec!["one", "two"]);
        let two = tags.rect("two").unwrap();
        assert!(two.min.y > tags.rect("one").unwrap().max.y, "{two:?}");
        assert_eq!(tags.to_json()[1]["id"], "two");
        assert_eq!(tags.to_json()[1]["rect"].as_array().map(Vec::len), Some(4));
        ctx.run_ui(egui::RawInput::default(), |_ui| tags.begin_frame()).drop_without_applying_deltas();
        assert!(tags.ids().is_empty() && tags.rect("one").is_none());
    }

    #[test]
    fn scrolled_out_widgets_are_listed_hidden_until_scrolled_to() {
        let ctx = egui::Context::default();
        let mut tags = Tags::default();
        let frame = |tags: &mut Tags| {
            ctx.run_ui(egui::RawInput::default(), |ui| {
                tags.begin_frame();
                egui::ScrollArea::vertical().max_height(60.0).show(ui, |ui| {
                    for i in 0..20 {
                        let r = ui.button(format!("button {i}"));
                        tags.tag(&format!("b{i}"), &r);
                    }
                });
                let r = ui.button("below");
                tags.tag("below", &r);
            })
            .drop_without_applying_deltas();
        };
        frame(&mut tags);
        // Only what a click can reach: b19 is out of view, and where it would be drawn is the
        // button below the scroll area.
        assert!(tags.rect("b0").is_some());
        assert_eq!(tags.rect("b19"), None);
        assert!(tags.hidden("b19") && !tags.hidden("b0") && !tags.hidden("nope"));
        let json = tags.to_json();
        let b19 = json.as_array().unwrap().iter().find(|e| e["id"] == "b19").unwrap();
        assert_eq!(b19["visible"], false, "{b19}");
        let below = tags.rect("below").unwrap();
        assert!(tags.rect("b0").unwrap().max.y <= below.min.y);
        tags.scroll_to("b19");
        // egui takes a frame or two to get there.
        for _ in 0..5 {
            if tags.rect("b19").is_some() {
                break;
            }
            frame(&mut tags);
        }
        let b19 = tags.rect("b19").expect("scrolled into view");
        assert!(b19.max.y <= below.min.y, "{b19:?} {below:?}");
        assert!(!tags.hidden("b19"));
    }

    #[test]
    fn values_show_and_secrets_stay_masked() {
        let ctx = egui::Context::default();
        let mut tags = Tags::default();
        let (mut text, mut secret) = ("db.example.com".to_string(), "hunter2-secret".to_string());
        ctx.run_ui(egui::RawInput::default(), |ui| {
            tags.begin_frame();
            let r = ui.text_edit_singleline(&mut text);
            tags.tag_value("host", &r, text.as_str());
            let r = ui.add(egui::TextEdit::singleline(&mut secret).password(true));
            tags.tag_secret("password", &r, !secret.is_empty());
            let r = ui.add_enabled(false, egui::Button::new("Test"));
            tags.tag_value("test", &r, "Test");
        })
        .drop_without_applying_deltas();
        let json = tags.to_json();
        assert_eq!(json[0]["value"], "db.example.com");
        assert_eq!(json[0].get("enabled"), None, "enabled widgets do not say so");
        assert_eq!((json[1]["masked"].clone(), json[1]["set"].clone()), (json!(true), json!(true)));
        assert_eq!(json[1].get("value"), None);
        assert!(!json.to_string().contains("hunter2"), "{json}");
        assert_eq!(tags.value("password"), None);
        assert_eq!(json[2]["enabled"], false);
        assert_eq!(tags.value("test"), Some(&json!("Test")));
    }
}
