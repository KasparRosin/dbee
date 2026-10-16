//! `dbee-cli`: dbee without a window. Placeholder: `version` and `help` only.
//!
//! Packaging smoke-tests the binary with `--version`. The commands in `docs/cli.md` (`commands`,
//! `describe`, `exec`, `run`, `mcp`, …) arrive with the engine. Exit codes: 0 ok, 1 command
//! failed, 2 usage error.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::io::Write;
use std::process::ExitCode;

const USAGE: &str = "\
usage: dbee-cli <command>

commands:
  version | --version | -V   print the version
  help | --help | -h         print this help
";

/// Writes to stdout; a closed pipe (`dbee-cli help | head -0`) is not an error worth a panic.
fn print(text: &str) -> ExitCode {
    let _ = std::io::stdout().write_all(text.as_bytes());
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
    match args.first().map(String::as_str) {
        Some("--version" | "-V" | "version") => print(&format!("dbee-cli {}\n", env!("CARGO_PKG_VERSION"))),
        Some("help" | "--help" | "-h") | None => print(USAGE),
        Some(other) => {
            let _ = writeln!(std::io::stderr(), "dbee-cli: unknown command `{other}`\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}
