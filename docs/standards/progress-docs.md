# Progress docs

> **Last reviewed:** 2026-10-15 · **Last updated:** 2026-10-15 · **Change:** major (scaffold)

## Which docs

`ROADMAP.md` and the files in `docs/` named `roadmap.md`, `gaps.md`, `target-app-parity.md` and every `*-parity.md`. They say where dbee stands against Beekeeper Studio and what to do next. They are working documents: out-of-date progress docs mislead the next agent.

## Status line

Every progress doc has this line under its title:

```
> **Last reviewed:** YYYY-MM-DD · **Last updated:** YYYY-MM-DD · **Change:** trivial|minor|major (what) · **Target:** Beekeeper Studio <version>
```

- Bump **Last reviewed** whenever you check the doc against the code and find it still true, even with no edit.
- Bump **Last updated** and **Change** whenever you edit it. `trivial` is a typo or link, `minor` a changed row or number, `major` a new section or a reassessment.
- **Target** is the Beekeeper Studio version the doc was assessed against.

## Revision history

The last section of every progress doc is a table, newest first:

```
| Date | Change | Summary |
|---|---|---|
```

## Numbers

- Every number says whether it was **measured** (a test, a diff, a benchmark you ran) or **estimated**.
- Don't raise a percentage without evidence a reviewer can rerun.
- Grade by behaviour, not by whether a menu item exists. A stub, an approximation or a partly wired setting is not done.
- Err low.

## Hours

Estimates are agent wall-clock hours to about 95 % of that area, not human hours and not 100 %.
