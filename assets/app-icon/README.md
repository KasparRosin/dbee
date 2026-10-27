# dbee app icon

`dbee.svg` is the source: the letters d and b on an ink tile. `dbee-small.svg` is the same mark
drawn for 16 and 32 px, with heavier strokes snapped to the pixel grid. Every other file here is
generated from the two by `packaging/icons.sh` (resvg, `cargo xtask ico`, `cargo xtask icns`), which
uses the small drawing for every size of 32 px and below. The outputs are committed, so builds
never need those tools.

- Geometry: 512 × 512 view box, corner radius 112; the macOS variant (`dbee-macos-512.png`) uses
  the 824/1024 body of Apple's icon grid.
- Palette: ink `#111719` (the tile), paper `#f6f8f7` (the d), green `#4caf1e` (the b). The same
  three colours work on light and dark grounds.
- Linux names: `hicolor/<size>/apps/dev.rosin.dbee.png` and `hicolor/scalable/apps/dev.rosin.dbee.svg`.

Original work by Kaspar Rosin and the dbee contributors, MIT OR Apache-2.0 (`LICENSE.txt`).
