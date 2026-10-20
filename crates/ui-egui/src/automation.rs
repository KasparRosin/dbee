//! Automation ids: kebab-case names (`conn-new`, `tab-close-3`) for the widgets agents and
//! tests drive.
//!
//! Each frame the screens tag what they draw ([`Tags::tag`]). The rects kept are those of the
//! last drawn frame: `ui.inspect` lists them, and `ui.click {id}` clicks the centre of one. A
//! frame starts the list over ([`Tags::begin_frame`]), so a widget that is no longer drawn is no
//! longer listed. Rects are in window points.

use std::collections::HashMap;

use egui::{Rect, Response};
use serde_json::{Value as Json, json};

/// The tagged widgets of the last drawn frame, in drawing order.
#[derive(Debug, Default)]
pub struct Tags {
    rects: Vec<(String, Rect)>,
    index: HashMap<String, usize>,
}

impl Tags {
    /// Forgets the last frame's tags; the screens tag this frame's widgets next.
    pub fn begin_frame(&mut self) {
        self.rects.clear();
        self.index.clear();
    }

    /// Tags `r` as `id`. The same id twice in a frame keeps the later widget.
    pub fn tag(&mut self, id: &str, r: &Response) {
        match self.index.get(id).and_then(|&i| self.rects.get_mut(i)) {
            Some(entry) => entry.1 = r.rect,
            None => {
                self.index.insert(id.to_string(), self.rects.len());
                self.rects.push((id.to_string(), r.rect));
            }
        }
    }

    /// Where `id` was drawn.
    pub fn rect(&self, id: &str) -> Option<Rect> {
        self.index.get(id).and_then(|&i| self.rects.get(i)).map(|(_, r)| *r)
    }

    /// The tagged ids, in drawing order.
    pub fn ids(&self) -> Vec<&str> {
        self.rects.iter().map(|(id, _)| id.as_str()).collect()
    }

    /// `[{id, rect: [x, y, width, height]}]`, as `ui.inspect` lists them.
    pub fn to_json(&self) -> Json {
        Json::Array(self.rects.iter().map(|(id, r)| json!({ "id": id, "rect": [r.min.x, r.min.y, r.width(), r.height()] })).collect())
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
}
