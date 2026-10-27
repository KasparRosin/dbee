# Asset attribution

Every non-code asset in this repository (images, icons, fonts, example files, presets) is listed here with its author, source and licence. `cargo xtask assets` (part of `cargo xtask ci`) fails if an asset file is missing from this table.

**Policy (mandatory):** dbee contains **no Beekeeper Studio (or other vendor) iconography, images, artwork, fonts, templates, themes or sample data**. Every asset is original or under a licence that allows redistribution, and has a row here.

## Generated in code

Drawn or defined in code and original to dbee (Kaspar Rosin and the dbee contributors, MIT OR Apache-2.0), so no asset file is involved:

- the UI icon set: `crates/ui-egui/src/icons.rs`, shapes drawn with egui's painter;
- the colour theme: `crates/ui-egui/src/theme.rs`, dbee's own colour tokens and syntax colours;
- the test databases: `xtask/testdb/seed.sql` and the tests' own setup SQL, synthetic data only.

## Compiled in from dependencies

The binaries embed egui's default fonts from the `epaint_default_fonts` crate. They are not files in this repository.

| Font | Licence |
|---|---|
| Ubuntu Light | Ubuntu Font Licence 1.0 |
| Hack Regular | MIT (Hack); Bitstream Vera Licence (the glyphs it builds on) |
| Noto Emoji Regular | OFL-1.1 |
| emoji-icon-font | MIT |

Their licence texts ship with the binaries from the first release on ([docs/releasing.md](docs/releasing.md)).

## Files

| Asset | Author | Source | Licence | Notes |
|---|---|---|---|---|
| `assets/app-icon/dbee-1024.png` | Kaspar Rosin and the dbee contributors | Generated from assets/app-icon/dbee.svg by packaging/icons.sh | MIT OR Apache-2.0 | |
| `assets/app-icon/dbee.icns` | Kaspar Rosin and the dbee contributors | Generated from assets/app-icon/dbee.svg by packaging/icons.sh | MIT OR Apache-2.0 | |
| `assets/app-icon/dbee.ico` | Kaspar Rosin and the dbee contributors | Generated from assets/app-icon/dbee.svg by packaging/icons.sh | MIT OR Apache-2.0 | |
| `assets/app-icon/dbee-macos-512.png` | Kaspar Rosin and the dbee contributors | Generated from assets/app-icon/dbee.svg by packaging/icons.sh | MIT OR Apache-2.0 | |
| `assets/app-icon/dbee.svg` | Kaspar Rosin and the dbee contributors | Original | MIT OR Apache-2.0 | |
| `assets/app-icon/hicolor/128x128/apps/dev.rosin.dbee.png` | Kaspar Rosin and the dbee contributors | Generated from assets/app-icon/dbee.svg by packaging/icons.sh | MIT OR Apache-2.0 | |
| `assets/app-icon/hicolor/16x16/apps/dev.rosin.dbee.png` | Kaspar Rosin and the dbee contributors | Generated from assets/app-icon/dbee.svg by packaging/icons.sh | MIT OR Apache-2.0 | |
| `assets/app-icon/hicolor/24x24/apps/dev.rosin.dbee.png` | Kaspar Rosin and the dbee contributors | Generated from assets/app-icon/dbee.svg by packaging/icons.sh | MIT OR Apache-2.0 | |
| `assets/app-icon/hicolor/256x256/apps/dev.rosin.dbee.png` | Kaspar Rosin and the dbee contributors | Generated from assets/app-icon/dbee.svg by packaging/icons.sh | MIT OR Apache-2.0 | |
| `assets/app-icon/hicolor/32x32/apps/dev.rosin.dbee.png` | Kaspar Rosin and the dbee contributors | Generated from assets/app-icon/dbee.svg by packaging/icons.sh | MIT OR Apache-2.0 | |
| `assets/app-icon/hicolor/48x48/apps/dev.rosin.dbee.png` | Kaspar Rosin and the dbee contributors | Generated from assets/app-icon/dbee.svg by packaging/icons.sh | MIT OR Apache-2.0 | |
| `assets/app-icon/hicolor/512x512/apps/dev.rosin.dbee.png` | Kaspar Rosin and the dbee contributors | Generated from assets/app-icon/dbee.svg by packaging/icons.sh | MIT OR Apache-2.0 | |
| `assets/app-icon/hicolor/64x64/apps/dev.rosin.dbee.png` | Kaspar Rosin and the dbee contributors | Generated from assets/app-icon/dbee.svg by packaging/icons.sh | MIT OR Apache-2.0 | |
| `assets/app-icon/hicolor/scalable/apps/dev.rosin.dbee.svg` | Kaspar Rosin and the dbee contributors | Original | MIT OR Apache-2.0 | |
| `assets/app-icon/LICENSE.txt` | Kaspar Rosin and the dbee contributors | Original | MIT OR Apache-2.0 | |
| `assets/app-icon/README.md` | Kaspar Rosin and the dbee contributors | Original | MIT OR Apache-2.0 | |
| `assets/readme/banner-dark.svg` | Kaspar Rosin and the dbee contributors | Original | MIT OR Apache-2.0 | The README banner for dark mode: the dbee wordmark as outlined shapes, no text or external references |
| `assets/readme/banner-light.svg` | Kaspar Rosin and the dbee contributors | Original | MIT OR Apache-2.0 | The README banner for light mode: the dbee wordmark as outlined shapes, no text or external references |
| `assets/readme/screenshot-filters.png` | Kaspar Rosin and the dbee contributors | Original: a screenshot of dbee taken by apps/dbee/tests/m1_day.rs (step 3), synthetic test data, reduced to 256 colours | MIT OR Apache-2.0 | Its text is drawn in egui's default fonts (above) |
| `packaging/macos/dmg/background.svg` | Kaspar Rosin and the dbee contributors | Original | MIT OR Apache-2.0 | |
| `packaging/macos/dmg/background.tiff` | Kaspar Rosin and the dbee contributors | Generated from packaging/macos/dmg/background.svg by packaging/macos/dmg/generate.py | MIT OR Apache-2.0 | |
