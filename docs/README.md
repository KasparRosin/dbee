# dbee documentation

Start with the [README](../README.md) for what dbee is and how to build it. Agents start with [`AGENTS.md`](../AGENTS.md).

## Using and driving dbee

- [installing.md](installing.md): installing each download, and opening an unsigned build the first time.
- [agents.md](agents.md): driving dbee from an agent (the control channel, isolated runs) and developing dbee as one.
- [control-protocol.md](control-protocol.md): the localhost control channel, `dbee --control <port>`, method by method.
- [cli.md](cli.md): the command-line tool, `dbee-cli` (planned beyond `version` and `help`).

## How dbee is built

- [architecture.md](architecture.md): the crates, their layers and the data flow.
- [testing.md](testing.md): the kinds of tests, the Docker test servers and the timing tests.
- [decisions.md](decisions.md): significant decisions and why they were made.
- [releasing.md](releasing.md): the release pipeline, signing and the GitHub setup.
- [contributors.md](contributors.md): how contributors are credited in the About window.
- Standards: [never crash](standards/never-crash.md), [progress docs](standards/progress-docs.md), [licensing](standards/licensing.md).

## Where dbee stands

- [ROADMAP.md](../ROADMAP.md): the one-page summary (stage, headline numbers, progress log).
- [roadmap.md](roadmap.md): the detail behind it: current focus, alpha and beta gates, milestones.
- [gaps.md](gaps.md): known gaps and limits, ranked; the work list.
- [target-app-parity.md](target-app-parity.md): the full assessment against Beekeeper Studio, with every measurement.
- Area checklists: [database engines](driver-parity.md), [UI](ui-parity.md), [file formats](file-format-parity.md), [platforms](hardware-parity.md), [languages](localization-parity.md).
