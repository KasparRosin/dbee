# dbee app icon

**Status: placeholder.** `dbee.svg` is the source; every other file here is generated from it by
`packaging/icons.sh` (resvg, `cargo xtask ico`, `cargo xtask icns`) and committed, so builds never
need those tools.

- Geometry: 512 × 512 view box, corner radius 112; the macOS variant (`dbee-macos-512.png`) uses
  the 824/1024 body of Apple's icon grid.
- Palette: charcoal `#1f2328`, amber `#f5a524`.
- Linux names: `hicolor/<size>/apps/dev.rosin.dbee.png` and `hicolor/scalable/apps/dev.rosin.dbee.svg`.

Original work by Kaspar Rosin and the dbee contributors, MIT OR Apache-2.0 (`LICENSE.txt`).
