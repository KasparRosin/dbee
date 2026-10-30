//! Workspace tooling: `cargo xtask <command>` (or `cargo run -p xtask -- <command>`).
//!
//! Pure Rust (std + serde_json). External tools (`cargo`, `git`) are invoked through
//! `std::process::Command`.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod assets;
mod contributors;
mod icns;
mod ico;
mod isolated;
mod layers;
mod licenses;
mod provenance;
mod stats;
mod testdb;
mod version;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const USAGE: &str = "\
usage: cargo xtask <command>

commands:
  ci              fmt --check, clippy -D warnings, testdb up (when `docker info` answers), test, assets, layers,
                  provenance (stops at the first failure); after testdb up the tests run with
                  DBEE_REQUIRE_ORACLES=1, so a driver test that skips fails
  assets          check that every icon/image/font/sample file is attributed in ATTRIBUTION.md
  layers          enforce the crate dependency layering (AGENTS.md \"Layering\")
  licenses        list dependencies outside the licence allowlist (docs/standards/licensing.md); not a ci step
  provenance      fail on banned terms in tracked files: the built-in word list, and the brand names in the
                  local plan/provenance-terms.txt when it exists (xtask/src/provenance.rs)
  version [set X.Y.Z[-pre]]
                  print or set the workspace version ([workspace.package] in Cargo.toml)
  testdb up|down|status
                  the Docker test servers (Postgres, MySQL, MariaDB, sshd); `up` writes target/testdb/env.json
  stats [--exact] count tests and lines per crate (--exact: ask the test harness via `-- --list`)
  ico <out.ico> <in.png>...
                  pack square PNGs (<= 256 px) into a Windows .ico (see packaging/icons.sh)
  icns <out.icns> <in.png>...
                  pack square PNGs (16-1024 px) into a macOS .icns, on any OS (see packaging/icons.sh)
  contributors    write contributors/contributors.json from git history and contributors/people.toml
  run-isolated [--release] [--port N] [-- args]
                  build and run dbee on a throwaway home (target/isolated/<port>/home) with the control
                  channel on 127.0.0.1:<port> (default 7979) and in-memory secrets; no SSH agent on
                  Linux and macOS (on Windows the OpenSSH agent pipe is still reachable)
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
    let rest: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
    let result = match args.first().map(String::as_str) {
        Some("assets") => assets::run(&root()),
        Some("layers") => cmd_layers(),
        Some("licenses") => licenses::run(&root()),
        Some("provenance") => provenance::run(&root()),
        Some("ci") => cmd_ci(),
        Some("stats") => stats::run(&root(), rest.contains(&"--exact")),
        Some("testdb") => testdb::run(&root(), &rest),
        Some("ico") => ico::run(&rest),
        Some("icns") => icns::run(&rest),
        Some("version") => version::run(&root(), &rest),
        Some("contributors") => contributors::run(&root()),
        Some("run-isolated") => isolated::run(&root(), &rest),
        Some("-h" | "--help" | "help") | None => {
            print!("{USAGE}");
            Ok(())
        }
        Some(other) => Err(format!("unknown command `{other}`\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Workspace root (parent of the xtask crate).
pub fn root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().unwrap_or(manifest).to_path_buf()
}

pub fn cargo() -> Command {
    let mut c = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    c.current_dir(root());
    c
}

fn run(mut cmd: Command, what: &str) -> Result<(), String> {
    eprintln!("$ {what}");
    let status = cmd.status().map_err(|e| format!("{what}: failed to spawn: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("{what}: exited with {status}")) }
}

pub fn metadata() -> Result<serde_json::Value, String> {
    let out = cargo().args(["metadata", "--format-version", "1", "--no-deps"]).output().map_err(|e| format!("cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!("cargo metadata failed:\n{}", String::from_utf8_lossy(&out.stderr)));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata: bad JSON: {e}"))
}

fn cmd_layers() -> Result<(), String> {
    let crates = layers::from_metadata(&metadata()?)?;
    println!("Dependency layering (AGENTS.md \"Layering\")\n");
    println!("{:<28} {:<12} workspace deps", "crate", "layer");
    for c in &crates {
        let ws: Vec<String> = c
            .deps
            .iter()
            .filter(|d| d.workspace)
            .map(|d| {
                let k = match d.kind {
                    layers::DepKind::Normal => "",
                    layers::DepKind::Dev => " (dev)",
                    layers::DepKind::Build => " (build)",
                };
                format!("{}{k}", layers::short_name(&d.name))
            })
            .collect();
        println!("{:<28} {:<12} {}", c.name, layers::describe(layers::classify(&c.name)), ws.join(", "));
    }
    let violations = layers::check(&crates);
    println!();
    if violations.is_empty() {
        println!("OK: {} crates, no layering violations.", crates.len());
        Ok(())
    } else {
        println!("{} violation(s):", violations.len());
        for v in &violations {
            println!("  - {v}");
        }
        Err(format!("{} layering violation(s)", violations.len()))
    }
}

/// The `cargo test` step and the command line it prints. With the test servers up (the testdb
/// step ran) it sets `DBEE_REQUIRE_ORACLES=1`, so a driver test that skips fails instead: that
/// catches a server that stopped answering, or an env file the tests read differently from how
/// `testdb up` wrote it.
fn test_step(servers_up: bool) -> (Command, String) {
    let mut c = cargo();
    c.args(["test", "--workspace"]);
    if servers_up {
        c.env("DBEE_REQUIRE_ORACLES", "1");
        (c, "DBEE_REQUIRE_ORACLES=1 cargo test --workspace".into())
    } else {
        (c, "cargo test --workspace".into())
    }
}

fn cmd_ci() -> Result<(), String> {
    type Step = (&'static str, Box<dyn Fn() -> Result<(), String>>);
    // Driver tests need the Docker test servers; without Docker they skip, which
    // DBEE_REQUIRE_ORACLES=1 turns into a failure (docs/testing.md).
    let docker = testdb::docker_available();
    let required = std::env::var("DBEE_REQUIRE_ORACLES").is_ok_and(|v| v == "1");
    let testdb_step: Option<Step> = match (docker, required) {
        (true, _) => Some(("testdb", Box::new(|| testdb::up(&root()).map(|_| ())))),
        (false, true) => Some(("testdb", Box::new(|| Err("Docker is not available and DBEE_REQUIRE_ORACLES=1".into())))),
        (false, false) => None,
    };
    let skipped: &[&str] = if testdb_step.is_none() { &["driver tests (no Docker)"] } else { &[] };
    // The testdb step runs before `test` and ci stops when it fails, so once `test` runs with it the
    // servers are up and a driver test that skips is a failure.
    let servers_up = testdb_step.is_some();
    let mut steps: Vec<Step> = vec![
        (
            "fmt",
            Box::new(|| {
                let mut c = cargo();
                c.args(["fmt", "--all", "--", "--check"]);
                run(c, "cargo fmt --all -- --check")
            }),
        ),
        (
            "clippy",
            Box::new(|| {
                let mut c = cargo();
                c.args(["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"]);
                run(c, "cargo clippy --workspace --all-targets -- -D warnings")
            }),
        ),
        (
            "test",
            Box::new(move || {
                let (c, what) = test_step(servers_up);
                run(c, &what)
            }),
        ),
        ("assets", Box::new(|| assets::run(&root()))),
        ("layers", Box::new(cmd_layers)),
        ("provenance", Box::new(|| provenance::run(&root()))),
    ];
    if let Some(step) = testdb_step {
        // Before `test`, so the driver tests find the servers.
        let at = steps.iter().position(|(n, _)| *n == "test").unwrap_or(steps.len());
        steps.insert(at, step);
    }
    let mut done = Vec::new();
    for (name, f) in &steps {
        eprintln!("\n=== ci: {name} ===");
        if let Err(e) = f() {
            println!("\nCI summary:");
            for d in &done {
                println!("  ok    {d}");
            }
            println!("  FAIL  {name}: {e}");
            for (n, _) in steps.iter().skip(done.len() + 1) {
                println!("  skip  {n}");
            }
            for s in skipped {
                println!("  skip  {s}");
            }
            return Err(format!("ci failed at `{name}`"));
        }
        done.push(*name);
    }
    println!("\nCI summary: all {} steps passed ({})", done.len(), done.join(", "));
    for s in skipped {
        println!("  skip  {s}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::test_step;

    #[test]
    fn the_test_step_requires_the_servers_once_the_testdb_step_brought_them_up() {
        let env = |c: &std::process::Command| c.get_envs().map(|(k, v)| (k.to_owned(), v.map(|v| v.to_owned()))).collect::<Vec<_>>();
        let (c, what) = test_step(true);
        assert_eq!(what, "DBEE_REQUIRE_ORACLES=1 cargo test --workspace");
        assert_eq!(env(&c), [("DBEE_REQUIRE_ORACLES".into(), Some("1".into()))]);
        assert_eq!(c.get_args().collect::<Vec<_>>(), ["test", "--workspace"]);
        // Without Docker the driver tests skip (or, with DBEE_REQUIRE_ORACLES=1 set by the caller,
        // the testdb step has already failed).
        let (c, what) = test_step(false);
        assert_eq!(what, "cargo test --workspace");
        assert!(env(&c).is_empty(), "{:?}", env(&c));
    }
}
