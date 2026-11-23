# dbee-cli

> **Last reviewed:** 2026-11-23 · **Last updated:** 2026-11-23 · **Change:** minor (checked against `apps/dbee-cli`: a non-UTF-8 argument, no argument, `-V`)

The command-line tool for dbee: run commands, run queries without a window. Today only `version` and `help` exist; everything else is **(planned)** and documented first. M1 (the daily Postgres workflow) left the CLI's commands out on purpose. Until they land, agents and scripts drive dbee through the desktop app's control channel ([`control-protocol.md`](control-protocol.md)), which reaches every command.

```sh
cargo run -p dbee-cli -- <subcommand>
```

## Subcommands

| Subcommand | Status | Example |
|---|---|---|
| `version` | available | `dbee-cli version` (also `--version`, `-V`) prints `dbee-cli <version>`. |
| `help` | available | `dbee-cli help` (also `--help`, `-h`, or no argument) prints the usage and exits 0. |
| `commands [--search X]` | planned | `dbee-cli commands --search export`: every command with id, label, menu path, shortcut, params doc and whether it is enabled. |
| `describe <id>` | planned | `dbee-cli describe query.run`: one command in full. |
| `exec <id> k=v…` | planned | `dbee-cli exec connection.open name=demo`: run one command. |
| `run <script.jsonl>` | planned | One command per line, `{"id":"…","params":{…}}`. The first failing step stops the run. |
| `query --connection <name> <sql>` | planned | `dbee-cli query --connection demo 'select 1'`: run SQL and print rows as JSON. |
| `export` | planned | Export a table or query result (CSV, JSON, SQL). |
| `send <port> <method> [json]` | planned | One control-channel request to a running app started with `dbee --control <port>` ([`control-protocol.md`](control-protocol.md)). |

## Output and exit codes

- The planned subcommands print JSON on stdout (`version` and `help` print plain text). Diagnostics go to stderr.
- Exit status: **0** ok, **1** the command failed, **2** usage error (unknown subcommand or option, missing argument).
- A non-UTF-8 argument never panics: its invalid bytes are read as U+FFFD, so today it is an unknown subcommand (exit 2).
- If the reader of stdout closes the pipe early (`dbee-cli commands | head`), the CLI drops the rest of its output but still finishes the work, and the exit status still reports failures.
- Programmatic calls never open dialogs: a command with empty params runs with defaults or returns an error.
- Passwords are never printed or put in the journal.
