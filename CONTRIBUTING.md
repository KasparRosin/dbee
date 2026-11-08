# Contributing to dbee

Thanks for helping. These notes keep the history easy to read and the checks green.

## Commit messages

The subject line is `<type>: <summary>`. When the change belongs to an issue or pull request, or to a milestone task,
add it as the scope: `<type>(<scope>): <summary>`.

| Type | For |
|---|---|
| `feat` | new behaviour |
| `fix` | a bug fix |
| `docs` | documentation only |
| `test` | tests only |
| `refactor` | a code change that keeps behaviour |
| `perf` | speed or memory |
| `build` | the build, dependencies, packaging |
| `ci` | the CI workflows |
| `chore` | anything else: tooling, housekeeping |
| `revert` | undoing an earlier commit |
| `style` | formatting only |

The scope is an issue or pull request (`#12`) or a milestone task (`M2.6`, see [ROADMAP.md](ROADMAP.md)). For example:

- `feat(M2.6): database types with their ports and URL schemes`
- `fix(#8): read-only connections stay read-only after a query tab run`
- `docs: the release steps that are done`

Pull request titles take the same form. The body, when there is one, says why.

## Before a pull request

Run `cargo xtask ci`: formatting, clippy, the tests, and the asset, layering and provenance checks. Tests that need
database servers run against `cargo xtask testdb up` (Docker); [docs/testing.md](docs/testing.md) has the details.

## The rules for the code

[AGENTS.md](AGENTS.md) holds the repository's rules for people and agents alike: never crash, the clean room, no vendor
artwork, and the crate layers. Report vulnerabilities as [SECURITY.md](SECURITY.md) says, not in a public issue.
