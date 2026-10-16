//! The placeholder CLI: version, help, and usage errors that never panic.

use std::process::{Command, Output};

fn cli(args: &[&std::ffi::OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_dbee-cli")).args(args).output().unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn version_prints_the_workspace_version() {
    for flag in ["--version", "-V", "version"] {
        let out = cli(&[flag.as_ref()]);
        assert!(out.status.success(), "{flag}");
        assert_eq!(text(&out.stdout), format!("dbee-cli {}\n", env!("CARGO_PKG_VERSION")), "{flag}");
    }
}

#[test]
fn help_lists_the_commands() {
    for args in [vec![], vec!["help"], vec!["--help"], vec!["-h"]] {
        let args: Vec<&std::ffi::OsStr> = args.iter().map(|a| a.as_ref()).collect();
        let out = cli(&args);
        assert!(out.status.success());
        assert!(text(&out.stdout).contains("usage: dbee-cli"));
    }
}

#[test]
fn unknown_command_is_a_usage_error() {
    let out = cli(&["frobnicate".as_ref()]);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("unknown command `frobnicate`"));
}

#[cfg(unix)]
#[test]
fn non_utf8_argument_is_a_usage_error_not_a_panic() {
    use std::os::unix::ffi::OsStrExt;
    let out = cli(&[std::ffi::OsStr::from_bytes(&[0x66, 0xff])]);
    assert_eq!(out.status.code(), Some(2));
    assert!(!text(&out.stderr).contains("panicked"));
}
