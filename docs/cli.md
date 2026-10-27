# dbee-cli

> **Last reviewed:** 2026-10-26 · **Last updated:** 2026-10-26 · **Change:** minor (still planned after M1; the control channel meanwhile)

The command-line tool for dbee: run commands, run queries and serve MCP without a window. Today only `version` and `help` exist; everything else is **(planned)** and documented first. M1 (the daily Postgres workflow) left the CLI's commands out on purpose. Until they land, agents and scripts drive dbee through the desktop app's control channel ([`control-protocol.md`](control-protocol.md)), which reaches every command.

```sh
cargo run -p dbee-cli -- <subcommand>
```

## Subcommands

| Subcommand | Status | Example |
|---|---|---|
| `version` | available | `dbee-cli version` prints `dbee-cli <version>`. |
| `help` | available | `dbee-cli help` (also `--help`, `-h`) prints the reference and exits 0. |
| `commands [--search X]` | planned | `dbee-cli commands --search export`: every command with id, label, menu path, shortcut, params doc and whether it is enabled. |
| `describe <id>` | planned | `dbee-cli describe query.run`: one command in full. |
| `exec <id> k=v…` | planned | `dbee-cli exec connection.open name=demo`: run one command. |
| `run <script.jsonl>` | planned | One command per line, `{"id":"…","params":{…}}`. The first failing step stops the run. |
| `query --connection <name> <sql>` | planned | `dbee-cli query --connection demo 'select 1'`: run SQL and print rows as JSON. |
| `export` | planned | Export a table or query result (CSV, JSON, SQL). |
| `mcp` | planned | MCP server on stdio ([`mcp.md`](mcp.md)). |
| `send <port> <method> [json]` | planned | One control-channel request to a running app started with `dbee --control <port>` ([`control-protocol.md`](control-protocol.md)). |

## Output and exit codes

- Output is JSON on stdout. Diagnostics go to stderr.
- Exit status: **0** ok, **1** the command failed, **2** usage error (unknown subcommand or option, missing argument).
- Options may appear anywhere. A non-UTF-8 argument is a usage error, never a panic.
- If the reader of stdout closes the pipe early (`dbee-cli commands | head`), the CLI drops the rest of its output but still finishes the work, and the exit status still reports failures.
- Programmatic calls never open dialogs: a command with empty params runs with defaults or returns an error.
- Passwords are never printed or put in the journal.
