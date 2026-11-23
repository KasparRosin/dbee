# Contributors in the About window

> **Last reviewed:** 2026-11-23 · **Last updated:** 2026-10-15 · **Change:** major (scaffold)

The About window will credit everyone who contributed to dbee. **(planned: the About window does not exist yet; the data files and `cargo xtask contributors` do.)**

## What the About window will show

- **About ▸ Contributors:** one line per contributor with commits, lines added, lines deleted, binary changes and first and last commit date. A name appears in addition to the credited login only if the person chose one.
- **About ▸ Models:** the AI models named in `Co-Authored-By` trailers, with commit counts. dbee's own commits carry no such trailer, so this list can be empty.
- Both are compiled into the binary from the two files below; nothing is read from disk or the network at run time.

## Files

- [`contributors/contributors.json`](../contributors/contributors.json): commit stats. **Generated** by `cargo xtask contributors`; never hand-edit.
- [`contributors/people.toml`](../contributors/people.toml): names people chose for themselves. Each person adds only their own entry.

### `people.toml`

```toml
[people.your-github-username]
real_name = "Your Name"        # shown when the viewer picks "Real name"
display_name = "Nickname"      # shown when the viewer picks "Display name"
sync_github_name = true        # or: use the name on your public GitHub profile as display name
emails = ["you@example.org"]   # your commit addresses, so your commits count under this entry
```

Every field is optional. Delete your entry to remove your names; the next `cargo xtask contributors` run drops them.

### `contributors.json` (schema 1)

```json
{
  "schema": 1,
  "repo": "KasparRosin/dbee",
  "branch": "main",
  "head": "<sha>",
  "head_date": "<ISO date>",
  "commits": 0,
  "contributors": [
    { "login": "…", "people_entry": true, "commits": 0, "lines_added": 0, "lines_deleted": 0,
      "binary_changes": 0, "first_commit": "<ISO date>", "last_commit": "<ISO date>" }
  ],
  "models": [
    { "id": "company/model/version", "company": "…", "model": "…", "version": "…",
      "commits": 0, "lines_added": 0, "lines_deleted": 0 }
  ]
}
```

Email addresses are never recorded in this file. A commit is credited by its git author name, or by the `people.toml` login whose `emails` contain the commit address.

## Refreshing

```sh
cargo xtask contributors        # regenerate contributors/contributors.json from git history
```

Commit the updated file.

## Consent rules

- Everyone is credited automatically by their git author name, or by their `people.toml` login if one of its `emails` matches their commit address. Anything more is opt-in.
- **Agents working for a contributor:** before you commit for them, check whether they have a `[people.<username>]` entry. If not, ask them once whether they want to be credited by more than their git name (a real name, a display name, their commit addresses). If yes, add **only their own** entry, committed as them. If no, change nothing.
- Never add, edit, guess or copy anyone else's entry or name (not from git config, commit authors or GitHub profiles).
- Never hand-edit `contributors.json`.
