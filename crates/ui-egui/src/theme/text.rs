//! The ten text styles. Capitals are applied only when drawing, never to the stored text.

use egui::{Align2, Color32, FontFamily, FontId, Painter, Pos2, Rect, RichText, TextFormat, text::LayoutJob};

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
