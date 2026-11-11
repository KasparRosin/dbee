//! The ten text styles. Capitals are applied only when drawing, never to the stored text.

use egui::{Align, Align2, Color32, FontFamily, FontId, Painter, Pos2, Rect, RichText, TextFormat, text::LayoutJob};

use super::{INTER_MEDIUM, MONO_MEDIUM};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Text {
    Title,
    Subtitle,
    Body,
    Button,
    ButtonCaps,
    Small,
    Header,
    Label,
    Data,
    Code,
}

impl Text {
    pub const ALL: [Text; 10] =
        [Text::Title, Text::Subtitle, Text::Body, Text::Button, Text::ButtonCaps, Text::Small, Text::Header, Text::Label, Text::Data, Text::Code];

    pub fn font(self) -> FontId {
        let (family, size) = match self {
            Text::Title => (FontFamily::Name(INTER_MEDIUM.into()), 18.0),
            Text::Subtitle => (FontFamily::Name(INTER_MEDIUM.into()), 14.0),
            Text::Body => (FontFamily::Proportional, 13.0),
            Text::Button => (FontFamily::Name(INTER_MEDIUM.into()), 13.0),
            Text::ButtonCaps => (FontFamily::Name(MONO_MEDIUM.into()), 11.0),
            Text::Small => (FontFamily::Proportional, 12.0),
            Text::Header => (FontFamily::Name(INTER_MEDIUM.into()), 12.0),
            Text::Label => (FontFamily::Name(MONO_MEDIUM.into()), 10.5),
            Text::Data => (FontFamily::Monospace, 12.0),
            Text::Code => (FontFamily::Monospace, 13.0),
        };
        FontId::new(size, family)
    }

    pub const fn line(self) -> f32 {
        match self {
            Text::Title => 24.0,
            Text::Subtitle => 20.0,
            Text::Body | Text::Button => 18.0,
            Text::ButtonCaps | Text::Label => 14.0,
            Text::Small | Text::Header | Text::Data => 16.0,
            Text::Code => 20.0,
        }
    }

    pub const fn spacing(self) -> f32 {
        if self.caps() { 0.6 } else { 0.0 }
    }

    pub const fn caps(self) -> bool {
        matches!(self, Text::ButtonCaps | Text::Label)
    }

    fn drawn(self, s: String) -> String {
        if self.caps() { s.to_uppercase() } else { s }
    }

    /// Capitals where the style has them.
    pub fn rich(self, s: impl Into<String>) -> RichText {
        self.rich_as_written(self.drawn(s.into()))
    }

    /// The same look, the case untouched: for what came from a database or the user.
    pub fn rich_as_written(self, s: impl Into<String>) -> RichText {
        RichText::new(s).font(self.font()).extra_letter_spacing(self.spacing()).line_height(Some(self.line()))
    }

    pub fn format(self, color: Color32) -> TextFormat {
        TextFormat { font_id: self.font(), color, extra_letter_spacing: self.spacing(), line_height: Some(self.line()), ..TextFormat::default() }
    }

    pub fn job(self, s: &str, color: Color32) -> LayoutJob {
        LayoutJob::single_section(self.drawn(s.to_owned()), self.format(color))
    }

    /// Like [`Text::job`], the case untouched.
    pub fn job_as_written(self, s: &str, color: Color32) -> LayoutJob {
        LayoutJob::single_section(s.to_owned(), self.format(color))
    }

    /// Draws `s` with `anchor` at `pos`; returns the rect it took.
    pub fn paint(self, painter: &Painter, pos: Pos2, anchor: Align2, s: &str, color: Color32) -> Rect {
        let galley = painter.layout_job(self.job(s, color));
        let rect = anchor.anchor_size(pos, galley.size());
        painter.galley(rect.min, galley, color);
        rect
    }
}

/// A sentence dbee writes for a dialog or a toast, the names in it marked: a connection, a user,
/// a host, a path, a number. Laid out as one job, the names in `data` as written and the rest in
/// `body`; read out and tagged as the parts run together.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Words {
    /// Each part, and whether it is a name.
    parts: Vec<(String, bool)>,
}

impl Words {
    /// More words, in `body`.
    pub fn plain(self, s: impl Into<String>) -> Words {
        self.push(s.into(), false)
    }

    /// A name, in `data` as written.
    pub fn name(self, s: impl Into<String>) -> Words {
        self.push(s.into(), true)
    }

    fn push(mut self, s: String, name: bool) -> Words {
        match self.parts.last_mut() {
            _ if s.is_empty() => {}
            Some((last, was)) if *was == name => last.push_str(&s),
            _ => self.parts.push((s, name)),
        }
        self
    }

    /// The parts in `color`, as one job: the words in `body`, the names in `data`.
    pub fn job(&self, color: Color32) -> LayoutJob {
        let mut job = LayoutJob::default();
        for (s, name) in &self.parts {
            // egui puts each part's letters at the top of its own line; a name's 16 line centred
            // in the words' 18 puts its baseline on theirs.
            let format = if *name { TextFormat { valign: Align::Center, ..Text::Data.format(color) } } else { Text::Body.format(color) };
            job.append(s, 0.0, format);
        }
        job
    }

    /// The parts in `color` as a bar says them: the words in `label` capitals, the names in
    /// `data` as written, on one baseline.
    pub fn caps_job(&self, color: Color32) -> LayoutJob {
        let mut job = LayoutJob::default();
        for (s, name) in &self.parts {
            if *name {
                job.append(s, 0.0, TextFormat { valign: Align::Center, ..Text::Data.format(color) });
            } else {
                job.append(&Text::Label.drawn(s.clone()), 0.0, TextFormat { valign: Align::Center, ..Text::Label.format(color) });
            }
        }
        job
    }
}

impl std::fmt::Display for Words {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.parts.iter().try_for_each(|(s, _)| f.write_str(s))
    }
}

impl From<&str> for Words {
    fn from(s: &str) -> Words {
        Words::default().plain(s)
    }
}

impl From<String> for Words {
    fn from(s: String) -> Words {
        Words::default().plain(s)
    }
}

impl From<&String> for Words {
    fn from(s: &String) -> Words {
        Words::default().plain(s.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ten_styles_match_the_design() {
        let inter = FontFamily::Name(INTER_MEDIUM.into());
        let mono = FontFamily::Name(MONO_MEDIUM.into());
        let rows = [
            (Text::Title, inter.clone(), 18.0, 24.0, 0.0, false),
            (Text::Subtitle, inter.clone(), 14.0, 20.0, 0.0, false),
            (Text::Body, FontFamily::Proportional, 13.0, 18.0, 0.0, false),
            (Text::Button, inter.clone(), 13.0, 18.0, 0.0, false),
            (Text::ButtonCaps, mono.clone(), 11.0, 14.0, 0.6, true),
            (Text::Small, FontFamily::Proportional, 12.0, 16.0, 0.0, false),
            (Text::Header, inter, 12.0, 16.0, 0.0, false),
            (Text::Label, mono, 10.5, 14.0, 0.6, true),
            (Text::Data, FontFamily::Monospace, 12.0, 16.0, 0.0, false),
            (Text::Code, FontFamily::Monospace, 13.0, 20.0, 0.0, false),
        ];
        assert_eq!(rows.len(), Text::ALL.len());
        for (i, (style, family, size, line, spacing, caps)) in rows.into_iter().enumerate() {
            assert_eq!(Text::ALL[i], style);
            assert_eq!(style.font().family, family, "{style:?}");
            assert_eq!(style.font().size, size, "{style:?}");
            assert_eq!(style.line(), line, "{style:?}");
            assert_eq!(style.spacing(), spacing, "{style:?}");
            assert_eq!(style.caps(), caps, "{style:?}");
        }
    }

    #[test]
    fn a_sentences_names_sit_on_its_words_baseline() {
        // A name in `data` amid words in `body`: the same baseline, the words' 18 line, and the
        // parts read out together.
        let words = Words::from("Delete ").name("Shop xp").plain(" xp?");
        assert_eq!(words.to_string(), "Delete Shop xp xp?");
        for ppp in [1.0, 1.15] {
            let mut ready = false;
            let mut h = egui_kittest::Harness::builder().with_pixels_per_point(ppp).build_ui_state(
                move |ui, seen: &mut Vec<f32>| {
                    if !ready {
                        crate::components::showcase::prepare(ui.ctx(), egui::Theme::Light);
                        ready = true;
                        ui.ctx().request_repaint();
                        return;
                    }
                    let g = ui.fonts_mut(|f| f.layout_job(Words::from("Delete ").name("Shop xp").plain(" xp?").job(Color32::PLACEHOLDER)));
                    let row = &g.rows[0];
                    *seen = row.glyphs.iter().filter(|g| g.chr == 'x').map(|g| g.pos.y).chain([row.size.y]).collect();
                },
                Vec::new(),
            );
            h.run();
            let seen = h.state();
            assert_eq!(seen.len(), 3, "{ppp}: an x in the name and one in the words, and the row: {seen:?}");
            assert_eq!(seen[0], seen[1], "{ppp}: one baseline: {seen:?}");
            assert!((seen[2] - Text::Body.line()).abs() < 0.5, "{ppp}: the words' 18 line: {seen:?}");
        }
    }

    #[test]
    fn a_bars_names_sit_on_its_capitals_baseline() {
        // dbee's words in `label` capitals and a table's name in `data` as written: one baseline,
        // and the parts read out together, the name's case kept.
        let words = Words::from("Loading ").name("Orders").plain("…");
        assert_eq!(words.to_string(), "Loading Orders…");
        // At some scales the two fonts' rounding leaves them a pixel apart.
        for ppp in [1.0, 1.25] {
            let mut ready = false;
            let mut h = egui_kittest::Harness::builder().with_pixels_per_point(ppp).build_ui_state(
                move |ui, seen: &mut (String, Vec<f32>)| {
                    if !ready {
                        crate::components::showcase::prepare(ui.ctx(), egui::Theme::Light);
                        ready = true;
                        ui.ctx().request_repaint();
                        return;
                    }
                    let g = ui.fonts_mut(|f| f.layout_job(Words::from("Loading ").name("Orders").plain("…").caps_job(Color32::PLACEHOLDER)));
                    let row = &g.rows[0];
                    *seen = (g.text().to_owned(), row.glyphs.iter().filter(|g| g.chr == 'L' || g.chr == 'd').map(|g| g.pos.y).collect());
                },
                (String::new(), Vec::new()),
            );
            h.run();
            let (text, ys) = h.state();
            assert_eq!(text, "LOADING Orders…", "{ppp}");
            assert_eq!(ys.len(), 2, "{ppp}: an L in the words and a d in the name: {ys:?}");
            assert!((ys[0] - ys[1]).abs() < 0.5, "{ppp}: one baseline: {ys:?}");
        }
    }

    #[test]
    fn capitals_are_only_drawn() {
        assert_eq!(Text::Label.rich("13 rows").text(), "13 ROWS");
        assert_eq!(Text::Body.rich("13 rows").text(), "13 rows");
        assert_eq!(Text::Label.rich_as_written("db.Example.com").text(), "db.Example.com");
        let job = Text::ButtonCaps.job("Run", Color32::PLACEHOLDER);
        assert_eq!(job.text, "RUN");
        assert_eq!(job.sections[0].format.extra_letter_spacing, 0.6);
        assert_eq!(job.sections[0].format.line_height, Some(14.0));
    }
}
