# Licensing

> **Last reviewed:** 2026-10-27 · **Last updated:** 2026-10-27 · **Change:** minor (no NOTICE file; egui's fonts are compiled into the binaries)

## dbee

dbee is licensed **MIT OR Apache-2.0**. The texts are `LICENSE-MIT` and `LICENSE-APACHE`; the copyright line is in `LICENSE-MIT` and the README.

## Dependencies

Allowed: MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, Zlib, Unicode-3.0, CC0-1.0, BSL-1.0, and MPL-2.0 used unmodified.

Never GPL, LGPL or AGPL, linked or vendored. A licence outside the list needs a human decision.

`cargo xtask licenses` lists dependencies outside the allowlist (run it by hand after adding one; it is not a ci step).

Every new dependency gets a one-line justification in its commit and, for a significant one, an entry in [`docs/decisions.md`](../decisions.md).

## Assets

No Beekeeper Studio (or any other vendor's) iconography, images, artwork, fonts, templates, themes, sample databases or help text. One exception: the logos of the database engines dbee connects to, shown to name that engine, following each owner's trademark guidelines. See the asset rule in [`AGENTS.md`](../../AGENTS.md). Every asset file has a row in [`ATTRIBUTION.md`](../../ATTRIBUTION.md), enforced by `cargo xtask assets`.

The binaries embed egui's default fonts through the `epaint_default_fonts` crate (OFL-1.1, Ubuntu Font Licence 1.0, MIT), listed in ATTRIBUTION.md. Their licence texts, and those of the statically linked crates, ship with the binaries from the first release on ([releasing](../releasing.md)).

## Clean-room

Beekeeper Studio may only be observed black-box; never read its bundle, source or traffic, and never copy GPL, AGPL or LGPL code. The full rule is under "Non-negotiables" in [`AGENTS.md`](../../AGENTS.md).
