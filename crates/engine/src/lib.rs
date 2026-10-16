//! dbee engine (layer L5): the session, the command registry (`cmd/*`), undo and the journal.
//!
//! Empty in the scaffold. Every user-visible action becomes a command here (AGENTS.md
//! "Everything is a command"); the UI, the control channel, the CLI and MCP reach them all.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
