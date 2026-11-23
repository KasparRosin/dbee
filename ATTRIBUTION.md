# Asset attribution

Every non-code asset in this repository (images, icons, fonts, example files, presets) is listed here with its author, source and licence. `cargo xtask assets` (part of `cargo xtask ci`) fails if an asset file is missing from this table.

**Policy (mandatory):** dbee contains **no Beekeeper Studio (or other vendor) iconography, images, artwork, fonts, templates, themes or sample data**. Every asset is original or under a licence that allows redistribution, and has a row here.

## Generated in code

Drawn or defined in code and original to dbee (Kaspar Rosin and the dbee contributors, MIT OR Apache-2.0), so no asset file is involved:

- the UI icon set: `crates/ui-egui/src/components/icon.rs`, shapes drawn with egui's painter from dbee's own design;
- the colour theme: `crates/ui-egui/src/theme/tokens.rs`, dbee's own colour tokens and syntax colours;
- the test databases: `xtask/testdb/seed.sql` and the tests' own setup SQL, synthetic data only.

## Compiled in from dependencies

The binaries embed egui's default fonts from the `epaint_default_fonts` crate. They are not files in this repository. Since M3 they come second, as the fallback: the bundled Inter and JetBrains Mono (see Files) come first.

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
| `assets/app-icon/dbee.icns` | Kaspar Rosin and the dbee contributors | Generated from assets/app-icon/dbee.svg by packaging/icons.sh | MIT OR Apache-2.0 | Sizes below 64 px resized from the 64 px render |
| `assets/app-icon/dbee.ico` | Kaspar Rosin and the dbee contributors | Generated from assets/app-icon/dbee.svg by packaging/icons.sh | MIT OR Apache-2.0 | Sizes below 64 px resized from the 64 px render |
| `assets/app-icon/dbee-macos-512.png` | Kaspar Rosin and the dbee contributors | Generated from assets/app-icon/dbee.svg by packaging/icons.sh | MIT OR Apache-2.0 | |
| `assets/app-icon/dbee.svg` | Kaspar Rosin and the dbee contributors | Original | MIT OR Apache-2.0 | |
| `assets/app-icon/hicolor/128x128/apps/dev.rosin.dbee.png` | Kaspar Rosin and the dbee contributors | Generated from assets/app-icon/dbee.svg by packaging/icons.sh | MIT OR Apache-2.0 | |
| `assets/app-icon/hicolor/16x16/apps/dev.rosin.dbee.png` | Kaspar Rosin and the dbee contributors | Generated from assets/app-icon/dbee.svg by packaging/icons.sh | MIT OR Apache-2.0 | Resized from the 64 px render |
| `assets/app-icon/hicolor/24x24/apps/dev.rosin.dbee.png` | Kaspar Rosin and the dbee contributors | Generated from assets/app-icon/dbee.svg by packaging/icons.sh | MIT OR Apache-2.0 | Resized from the 64 px render |
| `assets/app-icon/hicolor/256x256/apps/dev.rosin.dbee.png` | Kaspar Rosin and the dbee contributors | Generated from assets/app-icon/dbee.svg by packaging/icons.sh | MIT OR Apache-2.0 | |
| `assets/app-icon/hicolor/32x32/apps/dev.rosin.dbee.png` | Kaspar Rosin and the dbee contributors | Generated from assets/app-icon/dbee.svg by packaging/icons.sh | MIT OR Apache-2.0 | Resized from the 64 px render |
| `assets/app-icon/hicolor/48x48/apps/dev.rosin.dbee.png` | Kaspar Rosin and the dbee contributors | Generated from assets/app-icon/dbee.svg by packaging/icons.sh | MIT OR Apache-2.0 | Resized from the 64 px render |
| `assets/app-icon/hicolor/512x512/apps/dev.rosin.dbee.png` | Kaspar Rosin and the dbee contributors | Generated from assets/app-icon/dbee.svg by packaging/icons.sh | MIT OR Apache-2.0 | |
| `assets/app-icon/hicolor/64x64/apps/dev.rosin.dbee.png` | Kaspar Rosin and the dbee contributors | Generated from assets/app-icon/dbee.svg by packaging/icons.sh | MIT OR Apache-2.0 | |
| `assets/app-icon/hicolor/scalable/apps/dev.rosin.dbee.svg` | Kaspar Rosin and the dbee contributors | Original | MIT OR Apache-2.0 | |
| `assets/app-icon/LICENSE.txt` | Kaspar Rosin and the dbee contributors | Original | MIT OR Apache-2.0 | |
| `assets/app-icon/README.md` | Kaspar Rosin and the dbee contributors | Original | MIT OR Apache-2.0 | |
| `assets/fonts/Inter-Medium.ttf` | The Inter Project Authors | https://github.com/rsms/inter/releases/download/v4.1/Inter-4.1.zip, `extras/ttf/`, v4.1 | SIL OFL 1.1 |  |
| `assets/fonts/Inter-Regular.ttf` | The Inter Project Authors | https://github.com/rsms/inter/releases/download/v4.1/Inter-4.1.zip, `extras/ttf/`, v4.1 | SIL OFL 1.1 |  |
| `assets/fonts/JetBrainsMono-Medium.ttf` | The JetBrains Mono Project Authors | https://github.com/JetBrains/JetBrainsMono/releases/download/v2.304/JetBrainsMono-2.304.zip, `fonts/ttf/`, v2.304 | SIL OFL 1.1 |  |
| `assets/fonts/JetBrainsMono-Regular.ttf` | The JetBrains Mono Project Authors | https://github.com/JetBrains/JetBrainsMono/releases/download/v2.304/JetBrainsMono-2.304.zip, `fonts/ttf/`, v2.304 | SIL OFL 1.1 |  |
| `assets/fonts/OFL-Inter.txt` | The Inter Project Authors | `LICENSE.txt` of the Inter 4.1 release zip (URL above) | SIL OFL 1.1 | Licence text; ships with the binaries |
| `assets/fonts/OFL-JetBrainsMono.txt` | The JetBrains Mono Project Authors | `OFL.txt` of the JetBrains Mono 2.304 release zip (URL above) | SIL OFL 1.1 | Licence text; ships with the binaries |
| `assets/logos/mariadb.svg` | Simple Icons contributors; the MariaDB name and sea lion logo are trademarks of MariaDB plc | https://cdn.jsdelivr.net/npm/simple-icons@13.21.0/icons/mariadb.svg (Simple Icons 13.21.0), unchanged | CC0-1.0 | SHA-256 477435746bb2024048c7b71529f615fb70d7e679618f37bac49f78fd07e67dfe; colour changed when drawn; names the engine only, no endorsement |
| `assets/logos/mysql.svg` | Simple Icons contributors; the MySQL name and logo are trademarks of Oracle Corporation | https://cdn.jsdelivr.net/npm/simple-icons@13.21.0/icons/mysql.svg (Simple Icons 13.21.0), unchanged | CC0-1.0 | SHA-256 67c144cc3e44ad14062a3f20ebe29c5e7eeb2c2c9c0f43cf48d87fe6291a8b2d; colour changed when drawn; names the engine only, no endorsement |
| `assets/logos/postgresql.svg` | Simple Icons contributors; the PostgreSQL name and elephant logo are trademarks of the PostgreSQL Community Association of Canada | https://cdn.jsdelivr.net/npm/simple-icons@13.21.0/icons/postgresql.svg (Simple Icons 13.21.0), unchanged | CC0-1.0 | SHA-256 c8ccadbd88c9312c2ad3734c4861b31d3485ce179ee91d64793143f83cfa745c; colour changed when drawn; names the engine only, no endorsement |
| `assets/logos/sqlite.svg` | Simple Icons contributors; the SQLite name is a trademark of Hipp, Wyrick & Company, Inc. | https://cdn.jsdelivr.net/npm/simple-icons@13.21.0/icons/sqlite.svg (Simple Icons 13.21.0), unchanged | CC0-1.0 | SHA-256 71d4153bc9661dfe6b92dad70f737ec2b6c7c839311e502b4ca66fe664fe12b6; colour changed when drawn; names the engine only, no endorsement |
| `assets/readme/screenshot-filters.png` | Kaspar Rosin and the dbee contributors | Original: a screenshot of dbee's window, taken with `ui.screenshot` in a `cargo xtask run-isolated` run on the Docker test servers, synthetic test data | MIT OR Apache-2.0 | Its text is drawn in the bundled Inter and JetBrains Mono (above) |
| `packaging/macos/dmg/background.svg` | Kaspar Rosin and the dbee contributors | Original | MIT OR Apache-2.0 | |
| `packaging/macos/dmg/background.tiff` | Kaspar Rosin and the dbee contributors | Generated from packaging/macos/dmg/background.svg by packaging/macos/dmg/generate.py | MIT OR Apache-2.0 | |
