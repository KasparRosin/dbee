use egui::{FontData, FontDefinitions, FontFamily};

pub const INTER_MEDIUM: &str = "inter-medium";
pub const MONO_MEDIUM: &str = "mono-medium";

/// Inter and JetBrains Mono first in each family, egui's default fonts behind them.
pub(crate) fn definitions() -> FontDefinitions {
    let mut d = FontDefinitions::default();
    let files: [(&str, &'static [u8]); 4] = [
        ("Inter-Regular", include_bytes!("../../../../assets/fonts/Inter-Regular.ttf")),
        ("Inter-Medium", include_bytes!("../../../../assets/fonts/Inter-Medium.ttf")),
        ("JetBrainsMono-Regular", include_bytes!("../../../../assets/fonts/JetBrainsMono-Regular.ttf")),
        ("JetBrainsMono-Medium", include_bytes!("../../../../assets/fonts/JetBrainsMono-Medium.ttf")),
    ];
    for (name, bytes) in files {
        d.font_data.insert(name.to_owned(), std::sync::Arc::new(FontData::from_static(bytes)));
    }
    for (family, first, behind) in [
        (FontFamily::Proportional, "Inter-Regular", FontFamily::Proportional),
        (FontFamily::Monospace, "JetBrainsMono-Regular", FontFamily::Monospace),
        (FontFamily::Name(INTER_MEDIUM.into()), "Inter-Medium", FontFamily::Proportional),
        (FontFamily::Name(MONO_MEDIUM.into()), "JetBrainsMono-Medium", FontFamily::Monospace),
    ] {
        let mut list = vec![first.to_owned()];
        list.extend(FontDefinitions::default().families[&behind].iter().cloned());
        d.families.insert(family, list);
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::FontId;

    #[test]
    fn the_new_fonts_come_first_and_eguis_stay_behind_them() {
        let (d, default) = (definitions(), FontDefinitions::default());
        for (family, first, behind) in [
            (FontFamily::Proportional, "Inter-Regular", FontFamily::Proportional),
            (FontFamily::Monospace, "JetBrainsMono-Regular", FontFamily::Monospace),
            (FontFamily::Name(INTER_MEDIUM.into()), "Inter-Medium", FontFamily::Proportional),
            (FontFamily::Name(MONO_MEDIUM.into()), "JetBrainsMono-Medium", FontFamily::Monospace),
        ] {
            let list = &d.families[&family];
            assert_eq!(list[0], first);
            assert_eq!(list[1..], default.families[&behind][..], "{first}: egui's fonts follow");
        }
    }

    #[test]
    fn every_family_still_draws_what_the_new_fonts_lack() {
        let has = |defs: FontDefinitions, family: &FontFamily, c: char| {
            let ctx = egui::Context::default();
            ctx.set_fonts(defs);
            for _ in 0..2 {
                ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
            }
            ctx.fonts_mut(|f| f.has_glyph(&FontId::new(12.0, family.clone()), c))
        };
        let mut new_only = definitions();
        let families = [FontFamily::Proportional, FontFamily::Monospace, FontFamily::Name(INTER_MEDIUM.into()), FontFamily::Name(MONO_MEDIUM.into())];
        for family in &families {
            if let Some(list) = new_only.families.get_mut(family) {
                list.truncate(1);
            }
        }
        // The gear is in none of the four new files, and egui's fonts draw it in every family.
        let gear = '\u{2699}';
        for family in &families {
            assert!(!has(new_only.clone(), family, gear), "{family:?}: the new file alone lacks the gear");
            assert!(has(definitions(), family, gear), "{family:?}: egui's fonts behind it draw the gear");
        }
    }
}
