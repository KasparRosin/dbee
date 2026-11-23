//! The database engines' logos (`logo/postgresql`, `logo/mysql`, `logo/mariadb`, `logo/sqlite`):
//! Simple Icons 13.21.0's files in `assets/logos/` (CC0), kept as published, their shapes
//! unchanged and filled in the colour a use gives them.
//!
//! A logo's path is filled with its SVG's fill rule into a texture of how much of each pixel it
//! covers, as egui fills its glyphs (`kurbo`, `vello_cpu`), once per logo and size in pixels; each
//! use draws that texture tinted with its colour.

use std::collections::HashMap;

use dbee_config::DbKind;
use egui::{Color32, ColorImage, Context, Id, Painter, Rect, TextureHandle, TextureId, TextureOptions, Vec2, pos2};
use kurbo::{Affine, BezPath};
use vello_cpu::color::{OpaqueColor, Srgb};
use vello_cpu::peniko::Fill;
use vello_cpu::{Pixmap, RenderContext, Resources};

use crate::theme;

/// Simple Icons' box: every logo is drawn in 24 by 24.
const VIEW_BOX: f64 = 24.0;
/// The most pixels a logo's side is drawn at.
const MOST_PX: f32 = 1024.0;

/// An engine's logo.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Logo {
    Postgresql,
    Mysql,
    Mariadb,
    Sqlite,
}

impl Logo {
    pub const ALL: [Logo; 4] = [Logo::Postgresql, Logo::Mysql, Logo::Mariadb, Logo::Sqlite];

    /// The logo of the engine `kind`.
    pub fn of(kind: DbKind) -> Logo {
        match kind {
            DbKind::Postgres => Logo::Postgresql,
            DbKind::MySql => Logo::Mysql,
            DbKind::MariaDb => Logo::Mariadb,
            DbKind::Sqlite => Logo::Sqlite,
        }
    }

    /// Its name, after `logo/`, and its file's name in `assets/logos/`.
    pub fn name(self) -> &'static str {
        match self {
            Logo::Postgresql => "postgresql",
            Logo::Mysql => "mysql",
            Logo::Mariadb => "mariadb",
            Logo::Sqlite => "sqlite",
        }
    }

    /// Its SVG file, as published.
    fn svg(self) -> &'static str {
        match self {
            Logo::Postgresql => include_str!("../../../../assets/logos/postgresql.svg"),
            Logo::Mysql => include_str!("../../../../assets/logos/mysql.svg"),
            Logo::Mariadb => include_str!("../../../../assets/logos/mariadb.svg"),
            Logo::Sqlite => include_str!("../../../../assets/logos/sqlite.svg"),
        }
    }

    /// Draws it in `colour`, its 24 by 24 box fitted into `rect`, centred, on whole pixels. A rect
    /// that isn't drawable draws nothing.
    pub fn paint(self, painter: &Painter, rect: Rect, colour: Color32) {
        let ppp = painter.pixels_per_point();
        let px = (rect.width().min(rect.height()) * ppp).round();
        if !rect.is_finite() || !(1.0..=MOST_PX).contains(&px) {
            return;
        }
        let Some(texture) = texture(painter.ctx(), self, px as u16) else { return };
        let side = Vec2::splat(px / ppp);
        let corner = ((rect.center() - side / 2.0).to_vec2() * ppp).round() / ppp;
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        painter.image(texture, Rect::from_min_size(corner.to_pos2(), side), uv, colour);
    }

    /// How much of each pixel it covers, row by row, drawn `side` pixels square.
    fn coverage(self, side: u16) -> Option<Vec<u8>> {
        coverage(self.svg(), side)
    }
}

/// The logos' textures drawn so far, by logo and side in pixels.
#[derive(Clone, Default)]
struct Textures(HashMap<(Logo, u16), TextureHandle>);

/// `logo`'s texture `px` square in `ctx`, drawn the first time it is asked for.
fn texture(ctx: &Context, logo: Logo, px: u16) -> Option<TextureId> {
    let id = Id::new("dbee-logo-textures");
    let key = (logo, px);
    if let Some(drawn) = ctx.data_mut(|d| d.get_temp_mut_or_default::<Textures>(id).0.get(&key).map(TextureHandle::id)) {
        return Some(drawn);
    }
    let pixels = logo.coverage(px)?.into_iter().map(theme::mask).collect();
    let image = ColorImage::new([usize::from(px); 2], pixels);
    let handle = ctx.load_texture(format!("logo/{} {px}px", logo.name()), image, TextureOptions::LINEAR);
    let drawn = handle.id();
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Textures>(id).0.insert(key, handle));
    Some(drawn)
}

/// How much of each pixel the path of `svg` covers, row by row, its 24 by 24 box drawn `side`
/// pixels square, filled with the rule the SVG names (nonzero unless it says evenodd).
fn coverage(svg: &str, side: u16) -> Option<Vec<u8>> {
    if side == 0 {
        return None;
    }
    let path = BezPath::from_svg(attribute(svg, "d")?).ok()?;
    let rule = if attribute(svg, "fill-rule") == Some("evenodd") { Fill::EvenOdd } else { Fill::NonZero };
    let mut ctx = RenderContext::new(side, side);
    ctx.set_transform(Affine::scale(f64::from(side) / VIEW_BOX));
    ctx.set_fill_rule(rule);
    ctx.set_paint(OpaqueColor::<Srgb>::WHITE);
    ctx.fill_path(&path);
    let mut pixmap = Pixmap::new(side, side);
    ctx.render(&mut pixmap, &mut Resources::new());
    Some(pixmap.data_as_u8_slice().as_chunks::<4>().0.iter().map(|[_, _, _, alpha]| *alpha).collect())
}

/// The value of the first `name="…"` attribute in `svg`.
fn attribute<'a>(svg: &'a str, name: &str) -> Option<&'a str> {
    let marker = format!(" {name}=\"");
    let rest = svg.get(svg.find(&marker)? + marker.len()..)?;
    rest.get(..rest.find('"')?)
}

#[cfg(test)]
mod tests {
    use egui::{Pos2, Vec2, pos2, vec2};

    use super::*;
    use crate::components::images;
    use crate::theme::{DARK, LIGHT};

    /// SHA-256 (FIPS 180-4) of `bytes`, in lowercase hex.
    fn sha256(bytes: &[u8]) -> String {
        const K: [u32; 64] = [
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be,
            0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa,
            0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85,
            0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
            0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f,
            0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
        ];
        let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
        let mut message = bytes.to_vec();
        message.push(0x80);
        while message.len() % 64 != 56 {
            message.push(0);
        }
        message.extend_from_slice(&(bytes.len() as u64 * 8).to_be_bytes());
        for block in message.chunks(64) {
            let mut w = [0u32; 64];
            for (i, word) in block.chunks(4).enumerate() {
                w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
            }
            for i in 16..64 {
                let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
                let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
                w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
            }
            let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
            for i in 0..64 {
                let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
                let ch = (e & f) ^ (!e & g);
                let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
                let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
                let t2 = s0.wrapping_add((a & b) ^ (a & c) ^ (b & c));
                (hh, g, f, e, d, c, b, a) = (g, f, e, d.wrapping_add(t1), c, b, a, t1.wrapping_add(t2));
            }
            for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
                *x = x.wrapping_add(y);
            }
        }
        h.iter().map(|x| format!("{x:08x}")).collect()
    }

    #[test]
    fn the_logo_files_are_simple_icons_13_21_0() {
        // Review Focus 5: each file is Simple Icons 13.21.0's, byte for byte: its SHA-256 is the
        // one ATTRIBUTION.md gives, in its row with the source, the version, CC0 and the change.
        assert_eq!(sha256(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad", "SHA-256 itself");
        let attribution = include_str!("../../../../ATTRIBUTION.md");
        for logo in Logo::ALL {
            let name = logo.name();
            let row = attribution
                .lines()
                .find(|l| l.starts_with(&format!("| `assets/logos/{name}.svg` |")))
                .unwrap_or_else(|| panic!("{name}: a row in ATTRIBUTION.md"));
            let digest = sha256(logo.svg().as_bytes());
            assert!(row.contains(&digest), "{name}: the file's SHA-256 {digest} in its row: {row}");
            for words in [
                &format!("https://cdn.jsdelivr.net/npm/simple-icons@{}/icons/{name}.svg", "13.21.0"),
                "Simple Icons 13.21.0",
                "CC0-1.0",
                "colour changed when drawn",
            ] {
                assert!(row.contains(words), "{name}: `{words}` in its row: {row}");
            }
            assert!(logo.svg().starts_with("<svg role=\"img\" viewBox=\"0 0 24 24\""), "{name}: Simple Icons' 24 by 24 box");
        }
        let kinds: Vec<Logo> = DbKind::ALL.into_iter().map(Logo::of).collect();
        assert_eq!(kinds, Logo::ALL, "one logo per engine");
    }

    /// Each logo's points, in its 24 by 24 box: one in a hole or a gap its shape closes round
    /// (MariaDB's and SQLite's shapes close no hole), and one in its ink. Read off Simple Icons'
    /// files drawn by another renderer (resvg), each over 0.4 from the nearest edge.
    const PROBES: [(Logo, Pos2, Pos2); 4] = [
        (Logo::Postgresql, pos2(13.75, 5.05), pos2(19.45, 6.25)),
        (Logo::Mysql, pos2(16.45, 15.05), pos2(4.75, 13.85)),
        (Logo::Mariadb, pos2(12.65, 18.25), pos2(16.65, 12.85)),
        (Logo::Sqlite, pos2(16.45, 8.95), pos2(7.45, 7.25)),
    ];

    /// The coverage at `at` (in the 24 box) of `pixels`, drawn `side` square.
    fn at(pixels: &[u8], side: u16, at: Pos2) -> u8 {
        let scale = f32::from(side) / 24.0;
        let (x, y) = ((at.x * scale) as usize, (at.y * scale) as usize);
        pixels.get(y * usize::from(side) + x).copied().unwrap_or_else(|| panic!("{at:?} in a {side} square"))
    }

    #[test]
    fn each_logo_fills_with_its_holes() {
        // Review Focus 5: the path is filled with the SVG's rule, so a hole stays the ground and
        // the ink is the colour. Drawn 240 square, 10 pixels to the box's unit.
        for (logo, hole, ink) in PROBES {
            let pixels = logo.coverage(240).unwrap_or_else(|| panic!("{logo:?} drawn"));
            assert_eq!(pixels.len(), 240 * 240, "{logo:?}: one value a pixel");
            assert_eq!(at(&pixels, 240, hole), 0, "{logo:?}: the hole at {hole:?} is the ground");
            assert_eq!(at(&pixels, 240, ink), 255, "{logo:?}: the ink at {ink:?} is the colour");
            let covered = pixels.iter().filter(|&&a| a == 255).count();
            assert!(covered > 240 * 240 / 20 && covered < 240 * 240 * 3 / 4, "{logo:?}: {covered} pixels of ink");
        }
        // The rule the SVG names: two squares drawn the same way round, the inner one is filled
        // under the default nonzero rule and a hole under evenodd.
        let squares = |rule: &str| format!(r#"<svg viewBox="0 0 24 24"><path{rule} d="M0 0h24v24H0z M6 6h12v12H6z"/></svg>"#);
        let centre = pos2(12.0, 12.0);
        let nonzero = coverage(&squares(""), 48).unwrap_or_default();
        let evenodd = coverage(&squares(r#" fill-rule="evenodd""#), 48).unwrap_or_default();
        assert_eq!((at(&nonzero, 48, centre), at(&evenodd, 48, centre)), (255, 0), "nonzero fills the inner square, evenodd leaves it");
        assert_eq!(coverage("<svg/>", 48), None, "no path, nothing drawn");
        assert_eq!(coverage(&squares(""), 0), None, "no pixels, nothing drawn");
    }

    #[test]
    fn a_logo_takes_the_colour_given() {
        // The coverage, drawn once for a size and pixels per point into a texture, tinted with the
        // colour each use gives: one texture for every colour, its own for another size; the
        // logo's box fitted into the rect.
        for ppp in [1.0, 2.0] {
            let rects = [Rect::from_min_size(pos2(12.0, 13.0), Vec2::splat(14.0)), Rect::from_min_size(pos2(60.0, 13.0), Vec2::splat(24.0))];
            let colours = [LIGHT.text_muted, DARK.text, LIGHT.conn_red];
            let mut harness = egui_kittest::Harness::builder().with_size(vec2(200.0, 60.0)).with_pixels_per_point(ppp).build_ui(move |ui| {
                for logo in Logo::ALL {
                    for colour in colours {
                        logo.paint(ui.painter(), rects[0], colour);
                    }
                    logo.paint(ui.painter(), rects[1], colours[0]);
                }
            });
            harness.run();
            let drawn = images(&harness.ctx, &harness.output().shapes);
            assert_eq!(drawn.len(), Logo::ALL.len() * 4, "ppp {ppp}: every logo drawn: {drawn:?}");
            let px = |pt: f32| (pt * ppp) as usize;
            for (logo, shown) in Logo::ALL.into_iter().zip(drawn.chunks(4)) {
                let small = (format!("logo/{} {}px", logo.name(), px(14.0)), [px(14.0); 2]);
                for (i, colour) in colours.into_iter().enumerate() {
                    let (rect, tint, name, size) = &shown[i];
                    assert_eq!((*rect, *tint), (rects[0], colour), "ppp {ppp} {logo:?}: in its rect, in the colour given");
                    assert_eq!((name.clone(), *size), small, "ppp {ppp} {logo:?}: its own texture, 14 points in pixels, for every colour");
                }
                let (rect, tint, name, size) = &shown[3];
                assert_eq!((*rect, *tint), (rects[1], colours[0]), "ppp {ppp} {logo:?}: the bigger one");
                assert_eq!((name.clone(), *size), (format!("logo/{} {}px", logo.name(), px(24.0)), [px(24.0); 2]), "ppp {ppp} {logo:?}");
            }
            let logos = harness.ctx.tex_manager().read().allocated().filter(|(_, m)| m.name.starts_with("logo/")).count();
            assert_eq!(logos, Logo::ALL.len() * 2, "ppp {ppp}: drawn once per logo and size, whatever the colour");
            harness.run();
            let again = harness.ctx.tex_manager().read().allocated().filter(|(_, m)| m.name.starts_with("logo/")).count();
            assert_eq!(again, logos, "ppp {ppp}: not drawn again the next frame");
        }
        // A rect that isn't drawable draws nothing.
        let mut harness = egui_kittest::Harness::builder().with_size(vec2(100.0, 40.0)).build_ui(|ui| {
            for rect in [Rect::NOTHING, Rect::from_min_size(Pos2::ZERO, Vec2::ZERO), Rect::from_min_size(pos2(f32::NAN, 0.0), Vec2::splat(14.0))] {
                Logo::Postgresql.paint(ui.painter(), rect, LIGHT.text);
            }
        });
        harness.run();
        assert!(images(&harness.ctx, &harness.output().shapes).is_empty(), "nothing drawn");
    }
}
