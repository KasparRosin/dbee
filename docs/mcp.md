# dbee MCP server

> **Last reviewed:** 2026-10-15 · **Last updated:** 2026-10-15 · **Change:** major (scaffold)

**All of this is planned.** `dbee-cli mcp` will expose dbee to AI agents over the [Model Context Protocol](https://modelcontextprotocol.io): newline-delimited JSON-RPC 2.0 on stdin/stdout. Logs go to stderr.

## Start it

| Mode | Command | What it drives |
|---|---|---|
| Headless | `dbee-cli mcp --demo` | An in-process engine session, no window, with a seeded in-memory SQLite database. |
| Bridge | `dbee-cli mcp --bridge HOST:PORT` | The running desktop app started with `dbee --control PORT` ([`control-protocol.md`](control-protocol.md)). |

Any other argument is an error (exit 2).

## Claude Code setup

```sh
claude mcp add dbee-headless -- /abs/path/dbee-cli mcp --demo
claude mcp add dbee -- /abs/path/dbee-cli mcp --bridge 127.0.0.1:9876
```

`.mcp.json`:

```json
{
  "mcpServers": {
    "dbee": { "command": "/abs/path/dbee-cli", "args": ["mcp", "--bridge", "127.0.0.1:9876"] },
    "dbee-headless": { "command": "/abs/path/dbee-cli", "args": ["mcp", "--demo"] }
  }
}
```

## Tools

| Tool | Modes | Purpose |
|---|---|---|
| `command_list` | both | Every command: id, label, menu, shortcut, params, enabled now (`filter`, `enabled_only`). |
| `command_run` | both | Run a command `{id, params}`; edits are undoable. |
| `command_batch` | both | Run several in order `{steps: [{id, params}], stop_on_error}`. |
| `doc_inspect` | both | Session state: connections, open tabs, selection. |
| `render_preview` | both | PNG of the current view. |
| `connection_list` | both | Saved connections (never passwords). |
| `schema_inspect` | both | Databases, tables, columns, keys and indexes of a connection. |
| `query_run` | both | Run SQL on a connection; rows capped, with a `truncated` flag. |
| `table_read` | both | Read a page of rows from a table. |
| `ui_*`, `screenshot` | bridge only | Inspect, click, key, type, screenshot the live window. |

Every tool carries a `title` and the four hints `readOnlyHint`, `destructiveHint`, `idempotentHint` and `openWorldHint`, so a client can let read-only tools run without asking. `query_run` is not read-only: it can run any SQL.

Errors:

- An unknown tool argument is JSON-RPC `-32602`, naming the accepted keys (`unknown argument "filtr" for command_list; expected: enabled_only, filter`).
- A line that is not JSON gets `-32700` and the server keeps serving.
- A failing command is an `isError` result with the engine's message.

## Resources

- `dbee://session`: the session as `doc_inspect` returns it.
- `dbee://commands`: the command catalog as `command_list` returns it.

## Prompts

`prompts/list` returns workflow prompts (explore a schema, write and check a query). The list is empty until the first one is written.

## Example session

```text
→ {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}
→ {"jsonrpc":"2.0","method":"notifications/initialized"}
→ {"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"query_run","arguments":{"connection":"demo","sql":"select 1 as one"}}}
← {"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"{\"columns\":[\"one\"],\"rows\":[[1]]}"}],"isError":false}}
```
