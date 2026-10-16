# Licensing

> **Last reviewed:** 2026-10-15 · **Last updated:** 2026-10-15 · **Change:** major (scaffold)

## dbee

dbee is licensed **MIT OR Apache-2.0**. The texts are `LICENSE-MIT` and `LICENSE-APACHE`; `NOTICE` carries the copyright notice.

## Dependencies

Allowed: MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, Zlib, Unicode-3.0, CC0-1.0, BSL-1.0, and MPL-2.0 used unmodified.

Never GPL, LGPL or AGPL, linked or vendored. A licence outside the list needs a human decision.

Every new dependency gets a one-line justification in its commit and, for a significant one, an entry in [`docs/decisions.md`](../decisions.md).

## Assets

No Beekeeper Studio (or any other vendor's) iconography, images, artwork, fonts, templates, themes, sample databases or help text. See the asset rule in [`AGENTS.md`](../../AGENTS.md). Every asset file has a row in [`ATTRIBUTION.md`](../../ATTRIBUTION.md), enforced by `cargo xtask assets`.

## Clean-room

Beekeeper Studio may only be observed black-box; never read its bundle, source or traffic, and never copy GPL, AGPL or LGPL code. The full rule is under "Non-negotiables" in [`AGENTS.md`](../../AGENTS.md).
