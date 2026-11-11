//! The one spinner: something runs. A [`size::SPINNER`] square in `text_muted`, as the app drew
//! it before the redesign; it shows at once (the design's 300 ms delay belongs to the empty and
//! loading states screen).

use egui::{Pos2, Rect, Response, Ui, Vec2, Widget};

use crate::theme::{size, tokens};

/// A spinner, added where it stands ([`Widget`]) or painted over a slot ([`Spinner::paint`]).
#[derive(Clone, Copy, Debug, Default)]
pub struct Spinner;

impl Spinner {
    pub fn new() -> Spinner {
        Spinner
    }

    fn egui(ui: &Ui) -> egui::Spinner {
        egui::Spinner::new().size(size::SPINNER).color(tokens(ui.ctx()).text_muted)
    }

    /// Paints it centred on `center`, taking no room: a busy tab's icon slot, the tree's refresh.
    pub fn paint(self, ui: &Ui, center: Pos2) {
        Spinner::egui(ui).paint_at(ui, Rect::from_center_size(center, Vec2::splat(size::SPINNER)));
    }
}

impl Widget for Spinner {
    fn ui(self, ui: &mut Ui) -> Response {
        ui.add(Spinner::egui(ui))
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use egui::{Shape, pos2};

    use super::*;
    use crate::components::{both, flat, painted};

    /// Every `.rs` file under `dir`.
    fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { panic!("{dir:?} can't be read") };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                sources(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    #[test]
    fn one_spinner_everywhere() {
        // No screen draws egui's spinner itself: they all add or paint this one.
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        for dir in ["src", "examples", "tests"] {
            sources(&root.join(dir), &mut files);
        }
        assert!(files.len() > 20, "the crate's sources: {files:?}");
        let own = root.join("src/components/spinner.rs");
        let mut found = Vec::new();
        for file in files.iter().filter(|f| **f != own) {
            let Ok(text) = std::fs::read_to_string(file) else { panic!("{file:?} can't be read") };
            let named = text.contains("egui::Spinner") || text.contains("widgets::Spinner");
            // `use egui::{…, Spinner, …};` names it too.
            let imported = text.split("use egui::{").skip(1).any(|rest| rest.split("};").next().is_some_and(|list| list.contains("Spinner")));
            if named || imported {
                found.push(file.strip_prefix(root).unwrap_or(file).display().to_string());
            }
        }
        assert_eq!(found, Vec::<String>::new(), "egui's spinner drawn outside components/spinner.rs");
    }

    #[test]
    fn the_spinner_keeps_todays_look() {
        // A 14 square in `text_muted`: an arc 3 wide of radius 5, round its centre.
        for (theme, t) in both() {
            let mut at = Rect::NOTHING;
            let shapes = painted(theme, |ui| {
                at = ui.add(Spinner::new()).rect;
                Spinner::new().paint(ui, pos2(300.0, 100.0));
            });
            assert_eq!(at.size(), Vec2::splat(14.0), "{theme:?}: it takes a 14 square");
            let arcs: Vec<(Pos2, f32, egui::Color32)> = flat(&shapes)
                .into_iter()
                .filter_map(|s| match s {
                    Shape::Path(p) => {
                        let rect = Rect::from_points(&p.points);
                        let colour = match p.stroke.color {
                            egui::epaint::ColorMode::Solid(c) => c,
                            egui::epaint::ColorMode::UV(_) => egui::Color32::RED,
                        };
                        Some((rect.center(), p.stroke.width, colour))
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(arcs.len(), 2, "{theme:?}: two spinners: {arcs:?}");
            for ((_, width, colour), centre) in arcs.iter().zip([at.center(), pos2(300.0, 100.0)]) {
                assert_eq!((*width, *colour), (3.0, t.text_muted), "{theme:?}: 3 wide, text_muted");
                let points: Vec<Pos2> = flat(&shapes)
                    .into_iter()
                    .filter_map(|s| match s {
                        Shape::Path(p) if p.points.iter().all(|q| (q.distance(centre) - 5.0).abs() < 0.01) => Some(p.points),
                        _ => None,
                    })
                    .flatten()
                    .collect();
                assert!(!points.is_empty(), "{theme:?}: an arc of radius 5 round {centre:?}");
            }
        }
    }
}
