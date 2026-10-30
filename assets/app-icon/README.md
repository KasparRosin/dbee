# dbee app icon

`dbee.svg` is the source: the database drum, three white discs with dark gaps and a green top, on
a dark rounded tile with soft light. Every other file here is generated from it by
`packaging/icons.sh` (resvg, ImageMagick, `cargo xtask ico`, `cargo xtask icns`). The outputs are
committed, so builds never need those tools.

- Geometry: 512 × 512 view box, tile corner radius 112; drum centre x 256, ovals 150 × 46, discs 70
  high with 23 between them, the top oval's centre at y 127; a flat darker strip covers the right 45
  units of each disc. The macOS variant (`dbee-macos-512.png`) uses the 824/1024 body of Apple's
  icon grid.
- Colours: tile `#26302F` to `#0A0E0F` (top to bottom); discs `#FFFFFF` to `#D3DAD7`; top `#5CC424`
  to `#3A8E14`; the strip is 24 % black; the shadow under the drum is black at 50 %.
- Sizes: 64 px and up are rendered from the SVG. Every size below 64 is the 64 px render resized
  down, so there is no separate small drawing.
- Linux names: `hicolor/<size>/apps/dev.rosin.dbee.png` and `hicolor/scalable/apps/dev.rosin.dbee.svg`.

Original work by Kaspar Rosin and the dbee contributors, MIT OR Apache-2.0 (`LICENSE.txt`).
