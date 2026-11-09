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
        for family in [FontFamily::Proportional, FontFamily::Monospace, FontFamily::Name(INTER_MEDIUM.into()), FontFamily::Name(MONO_MEDIUM.into())] {
            for c in ['é', 'Ж', 'Ω', '→'] {
                assert!(has(definitions(), &family, c), "{family:?} {c}");
            }
        }
        // egui's own fonts draw no 🙂 in its Proportional family, so it is held to what they draw.
        for (family, theirs) in [
            (FontFamily::Proportional, FontFamily::Proportional),
            (FontFamily::Monospace, FontFamily::Monospace),
            (FontFamily::Name(INTER_MEDIUM.into()), FontFamily::Proportional),
            (FontFamily::Name(MONO_MEDIUM.into()), FontFamily::Monospace),
        ] {
            if has(FontDefinitions::default(), &theirs, '🙂') {
                assert!(has(definitions(), &family, '🙂'), "{family:?} 🙂");
            }
        }
    }
}
