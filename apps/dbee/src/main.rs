//! dbee desktop app: reads the command line and the environment, builds the session's options,
//! and runs the window ([`dbee_ui_egui::DbeeApp`]).
//!
//! - `--version` (`-V`) prints the version.
//! - `--control <port>` (or `DBEE_CONTROL_PORT`) serves the control channel on
//!   `127.0.0.1:<port>` (`docs/control-protocol.md`). It listens before the window opens, so a
//!   port in use stops the start: an agent must never drive another dbee by mistake.
//! - `DBEE_SECRET_STORE=memory` keeps passwords in memory only; otherwise the OS keyring, and
//!   without one none (a warning is logged and passwords are asked each time).
//! - `DBEE_CLIPBOARD=memory` keeps every copy and cut in the app (a text field's too), never on
//!   the system clipboard; the control channel's `document.inspect` shows the start of the last
//!   as `clipboard` ([`Clipboard::Memory`]; set by `cargo xtask run-isolated`).
//! - `DBEE_NO_PREFS` keeps no connections file and no log file.
//! - `DBEE_ALLOW_TARGETS` (`host:port,…`, set by `cargo xtask run-isolated`) limits connections
//!   to those targets ([`SessionOptions::allowed_targets`]).
//! - `DBEE_ALLOW_DIRS` (folders joined as the system joins paths, set by `cargo xtask
//!   run-isolated`) limits database files to those folders, and keeps SQL from opening other files
//!   ([`SessionOptions::allowed_dirs`]). With `DBEE_ALLOW_TARGETS` set and this not, no folder is
//!   allowed.
//! - SSH uses `~/.ssh` and the OS agent ([`SshEnv::from_os`]).
//! - The session opens connections with every driver dbee ships (`dbee_drivers::all()`).
//!
//! The logger (`logging.rs`) and the Windows graphics start-up (`graphics.rs`) are set up first.
#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

#[cfg(any(target_os = "windows", test))]
mod graphics;
mod logging;

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, OnceLock};

use dbee_config::{MemoryStore, SecretStore, keyring_store};
use dbee_engine::{Session, SessionOptions};
use dbee_ssh::SshEnv;
use dbee_ui_egui::{Clipboard, DbeeApp};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const APP_ID: &str = "dev.rosin.dbee";

fn config_dir() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support/dbee"))
    } else if cfg!(windows) {
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("dbee"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .map(|c| c.join("dbee"))
    }
}

/// Runs without preferences (`DBEE_NO_PREFS`, agents' test runs) keep no log file and no
/// connections file.
fn prefs_enabled() -> bool {
    std::env::var_os("DBEE_NO_PREFS").is_none()
}

fn icon() -> Option<egui::IconData> {
    #[cfg(target_os = "macos")]
    let bytes = include_bytes!("../../../assets/app-icon/dbee-macos-512.png");
    #[cfg(not(target_os = "macos"))]
    let bytes = include_bytes!("../../../assets/app-icon/hicolor/256x256/apps/dev.rosin.dbee.png");
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    Some(egui::IconData { width: img.width(), height: img.height(), rgba: img.into_raw() })
}

/// The control port: `--control <port>` or `--control=<port>`, else `env` (`DBEE_CONTROL_PORT`;
/// empty is unset). Other arguments are left alone.
fn control_port(args: &[OsString], env: Option<OsString>) -> Result<Option<u16>, String> {
    let parse = |v: &str| match v.parse::<u16>() {
        Ok(p) if p > 0 => Ok(p),
        _ => Err(format!("the control port must be a number from 1 to 65535, not `{v}`")),
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let a = a.to_string_lossy();
        if a == "--control" {
            let v = it.next().ok_or("--control needs a port: --control 7979")?;
            return parse(&v.to_string_lossy()).map(Some);
        }
        if let Some(v) = a.strip_prefix("--control=") {
            return parse(v).map(Some);
        }
    }
    match env.map(|v| v.to_string_lossy().into_owned()).filter(|v| !v.is_empty()) {
        Some(v) => parse(&v).map(Some).map_err(|e| format!("DBEE_CONTROL_PORT: {e}")),
        None => Ok(None),
    }
}

/// `DBEE_SECRET_STORE=memory`.
fn memory_secrets(env: Option<OsString>) -> bool {
    env.is_some_and(|v| v == "memory")
}

/// `DBEE_CLIPBOARD=memory`: copies and cuts stay in the app. Anything else is the system clipboard.
fn clipboard(env: Option<OsString>) -> Clipboard {
    if env.is_some_and(|v| v == "memory") { Clipboard::Memory } else { Clipboard::System }
}

fn secret_store() -> Option<Arc<dyn SecretStore>> {
    if memory_secrets(std::env::var_os("DBEE_SECRET_STORE")) {
        log::info!("secrets are kept in memory for this run (DBEE_SECRET_STORE=memory)");
        return Some(Arc::new(MemoryStore::default()));
    }
    match keyring_store() {
        Ok(k) => Some(Arc::new(k)),
        Err(e) => {
            log::warn!("no keyring ({e}); passwords are asked each time and not saved");
            None
        }
    }
}

/// `DBEE_ALLOW_TARGETS` (`host:port,host:port`, set by `cargo xtask run-isolated`): the only
/// targets this run may connect to. Unset is no limit. Set, it fails closed: an empty value or a
/// malformed entry allows nothing. Hosts are lowercased.
fn allowed_targets(env: Option<OsString>) -> Option<Vec<(String, u16)>> {
    let text = env?.to_string_lossy().into_owned();
    let entry = |e: &str| {
        let (host, port) = e.trim().rsplit_once(':')?;
        let port = port.parse::<u16>().ok().filter(|p| *p > 0)?;
        (!host.is_empty()).then(|| (host.to_lowercase(), port))
    };
    Some(text.split(',').filter_map(entry).collect())
}

/// `DBEE_ALLOW_DIRS` (folders joined as the system joins paths, set by `cargo xtask
/// run-isolated`): the only folders this run's database files may lie in. Unset is no limit,
/// unless the run limits its targets (`isolated`): then no folder. Set, it fails closed: an empty
/// value allows no folder.
fn allowed_dirs(env: Option<OsString>, isolated: bool) -> Option<Vec<PathBuf>> {
    match env {
        Some(dirs) => Some(std::env::split_paths(&dirs).filter(|p| !p.as_os_str().is_empty()).collect()),
        None => isolated.then(Vec::new),
    }
}

fn session_options(notify: Arc<dyn Fn() + Send + Sync>) -> SessionOptions {
    let allowed_targets = allowed_targets(std::env::var_os("DBEE_ALLOW_TARGETS"));
    if let Some(list) = &allowed_targets {
        log::info!("this run may connect only to {} allowed target(s) (DBEE_ALLOW_TARGETS)", list.len());
    }
    let allowed_dirs = allowed_dirs(std::env::var_os("DBEE_ALLOW_DIRS"), allowed_targets.is_some());
    if let Some(list) = &allowed_dirs {
        log::info!("this run may open files only in {} allowed folder(s) (DBEE_ALLOW_DIRS)", list.len());
    }
    SessionOptions {
        config_dir: config_dir().filter(|_| prefs_enabled()),
        secrets: secret_store(),
        ssh: SshEnv::from_os(),
        notify: Some(notify),
        allowed_targets,
        allowed_dirs,
        drivers: dbee_drivers::all(),
    }
}

/// What the panic hook logs: where, not the message, which can quote cell values or SQL (the
/// command that panicked returns it to its caller instead).
fn panic_line(location: Option<&std::panic::Location<'_>>, thread: Option<&str>) -> String {
    let at = location.map_or_else(|| "an unknown place".to_string(), |l| format!("{}:{}:{}", l.file(), l.line(), l.column()));
    format!("internal error: a panic on thread {} at {at}", thread.unwrap_or("?"))
}

fn main() -> ExitCode {
    let logger = logging::install();
    if let Some(logger) = logger {
        match config_dir().filter(|_| prefs_enabled()) {
            Some(dir) => {
                if let Err(e) = logger.attach_dir(&dir.join("logs")) {
                    logger.no_file();
                    log::warn!("log file: {e}");
                }
            }
            None => logger.no_file(),
        }
    }
    std::panic::set_hook(Box::new(|info| {
        let line = panic_line(info.location(), std::thread::current().name());
        if log::log_enabled!(log::Level::Error) {
            log::error!("{line}");
        } else {
            let _ = std::io::Write::write_fmt(&mut std::io::stderr(), format_args!("dbee: {line}\n"));
        }
    }));
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        let _ = std::io::Write::write_fmt(&mut std::io::stdout(), format_args!("dbee {VERSION}\n"));
        return ExitCode::SUCCESS;
    }
    let port = match control_port(&args, std::env::var_os("DBEE_CONTROL_PORT")) {
        Ok(p) => p,
        Err(e) => {
            let _ = std::io::Write::write_fmt(&mut std::io::stderr(), format_args!("dbee: {e}\n"));
            return ExitCode::from(2);
        }
    };
    // Listening before the window exists: requests wait in the queue until the first frame, and
    // wake the window once it has a context.
    let window: Arc<OnceLock<egui::Context>> = Arc::new(OnceLock::new());
    let control = match port {
        None => None,
        Some(port) => {
            let w = Arc::clone(&window);
            let wake = Arc::new(move || {
                if let Some(ctx) = w.get() {
                    ctx.request_repaint();
                }
            });
            match dbee_ui_egui::control_server::listen(port, wake) {
                Ok((_, rx)) => Some(rx),
                Err(e) => {
                    log::error!("control channel: cannot listen on 127.0.0.1:{port}: {e}");
                    let _ = std::io::Write::write_fmt(
                        &mut std::io::stderr(),
                        format_args!("dbee: control channel: cannot listen on 127.0.0.1:{port}: {e}\n"),
                    );
                    return ExitCode::FAILURE;
                }
            }
        }
    };
    let control = std::cell::Cell::new(control);
    let clipboard = clipboard(std::env::var_os("DBEE_CLIPBOARD"));
    if clipboard == Clipboard::Memory {
        log::info!("copies and cuts stay in the app for this run (DBEE_CLIPBOARD=memory)");
    }
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(format!("dbee {VERSION}"))
        .with_inner_size([1100.0, 720.0])
        .with_min_inner_size([640.0, 420.0])
        .with_app_id(APP_ID);
    if let Some(i) = icon() {
        viewport = viewport.with_icon(i);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    // Before eframe creates the wgpu instance: Windows starts on DirectX 12, or OpenGL when
    // DirectX 12 has no GPU driver, and never touches Vulkan unless asked (see graphics.rs).
    #[cfg(target_os = "windows")]
    let backend_override = eframe::wgpu::Backends::from_env();
    #[cfg(target_os = "windows")]
    let (options, first_backends) = {
        let mut options = options;
        let backends = graphics::configure(&mut options, backend_override);
        (options, backends)
    };
    #[cfg(target_os = "windows")]
    let retry_options = options.clone();
    let created = std::cell::Cell::new(false);
    let run = |options: eframe::NativeOptions| {
        eframe::run_native(
            "dbee",
            options,
            Box::new(|cc| {
                created.set(true);
                let _ = window.set(cc.egui_ctx.clone());
                let ctx = cc.egui_ctx.clone();
                let session = Session::new(session_options(Arc::new(move || ctx.request_repaint())))?;
                Ok(Box::new(DbeeApp::new(session, control.take()).with_clipboard(clipboard)))
            }),
        )
    };
    let result = run(options);
    // A graphics error before the window existed: start once more with the other backend.
    #[cfg(target_os = "windows")]
    let result = match result {
        Err(eframe::Error::Wgpu(e)) if !created.get() => match graphics::fallback_backends(backend_override, first_backends) {
            Some(backends) => {
                log::error!("graphics: the renderer didn't start on {first_backends:?} ({e}); trying {backends:?}");
                let mut options = retry_options;
                graphics::configure_backends(&mut options, backends);
                run(options)
            }
            None => Err(eframe::Error::Wgpu(e)),
        },
        other => other,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            log::error!("the window could not run: {e}");
            let _ = std::io::Write::write_fmt(&mut std::io::stderr(), format_args!("dbee: {e}\n"));
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod start_tests {
    use std::ffi::OsString;

    use super::*;

    fn args(a: &[&str]) -> Vec<OsString> {
        a.iter().map(OsString::from).collect()
    }

    #[test]
    fn the_control_port_comes_from_the_flag_or_the_environment() {
        assert_eq!(control_port(&args(&[]), None), Ok(None));
        assert_eq!(control_port(&args(&["--control", "7979"]), None), Ok(Some(7979)));
        assert_eq!(control_port(&args(&["--control=8001"]), None), Ok(Some(8001)));
        assert_eq!(control_port(&args(&[]), Some("7000".into())), Ok(Some(7000)));
        // The flag wins over the environment.
        assert_eq!(control_port(&args(&["--control", "7979"]), Some("7000".into())), Ok(Some(7979)));
        // An empty variable is unset.
        assert_eq!(control_port(&args(&[]), Some("".into())), Ok(None));
        // Other arguments (a file the desktop asks dbee to open) are left alone.
        assert_eq!(control_port(&args(&["/tmp/a.sqlite"]), None), Ok(None));
        for bad in [&["--control"][..], &["--control", "x"], &["--control", "70000"], &["--control", "0"], &["--control="], &["--control", "-1"]] {
            assert!(control_port(&args(bad), None).is_err(), "{bad:?}");
        }
        assert!(control_port(&args(&[]), Some("seven".into())).is_err());
    }

    #[test]
    fn allowed_targets_come_from_the_environment_and_fail_closed() {
        let t = |h: &str, p: u16| (h.to_string(), p);
        assert_eq!(allowed_targets(None), None, "a normal run has no limit");
        assert_eq!(allowed_targets(Some("127.0.0.1:1,localhost:32780".into())), Some(vec![t("127.0.0.1", 1), t("localhost", 32780)]));
        // Hosts are compared lowercased; spaces around entries are ignored.
        assert_eq!(allowed_targets(Some(" LocalHost:5 , 127.0.0.1:6 ".into())), Some(vec![t("localhost", 5), t("127.0.0.1", 6)]));
        // Set but empty or malformed: nothing is allowed, never everything.
        assert_eq!(allowed_targets(Some("".into())), Some(vec![]));
        assert_eq!(allowed_targets(Some("x,h:0,h:99999,:5,h:p,127.0.0.1:1".into())), Some(vec![t("127.0.0.1", 1)]));
    }

    #[test]
    fn allowed_dirs_come_from_the_environment_and_fail_closed() {
        assert_eq!(allowed_dirs(None, false), None, "a normal run has no limit");
        let joined = std::env::join_paths(["/repo/target", "/repo/target/isolated/7979/home"]).unwrap();
        let both = Some(vec![PathBuf::from("/repo/target"), PathBuf::from("/repo/target/isolated/7979/home")]);
        assert_eq!(allowed_dirs(Some(joined.clone()), false), both);
        assert_eq!(allowed_dirs(Some(joined), true), both);
        // Set but empty, or a run that limits its targets without naming folders: no folder is
        // allowed, never every one.
        assert_eq!(allowed_dirs(Some("".into()), false), Some(vec![]));
        assert_eq!(allowed_dirs(None, true), Some(vec![]));
    }

    #[test]
    fn memory_secrets_only_when_asked() {
        assert!(memory_secrets(Some("memory".into())));
        assert!(!memory_secrets(Some("keyring".into())));
        assert!(!memory_secrets(Some("".into())));
        assert!(!memory_secrets(None));
    }

    #[test]
    fn the_memory_clipboard_only_when_asked() {
        assert_eq!(clipboard(Some("memory".into())), Clipboard::Memory);
        for other in [Some("system".into()), Some("".into()), Some("Memory".into()), None] {
            assert_eq!(clipboard(other.clone()), Clipboard::System, "{other:?}");
        }
    }

    #[test]
    fn the_panic_line_names_the_place_not_the_message() {
        let here = std::panic::Location::caller();
        let line = panic_line(Some(here), Some("dbee-db"));
        assert!(line.starts_with("internal error: a panic on thread dbee-db at "), "{line}");
        assert!(line.contains("main.rs:"), "{line}");
        assert_eq!(panic_line(None, None), "internal error: a panic on thread ? at an unknown place");
    }
}

#[cfg(test)]
mod packaging_tests {
    /// No platform offers dbee for files while no engine opens one. The `.sqlite` and `.sqlite3`
    /// associations come back with the SQLite engine (docs/roadmap.md).
    #[test]
    fn no_platform_offers_dbee_for_files_yet() {
        let plist = include_str!("../../../packaging/macos/Info.plist.in");
        let wxs = include_str!("../../../packaging/windows/dbee.wxs");
        let desktop = include_str!("../../../packaging/linux/dev.rosin.dbee.desktop");
        let metainfo = include_str!("../../../packaging/linux/dev.rosin.dbee.metainfo.xml.in");
        assert!(!plist.contains("CFBundleDocumentTypes"), "Info.plist.in declares document types");
        assert!(!wxs.contains("OpenWithProgids") && !wxs.contains("FileAssociations"), "dbee.wxs declares file associations");
        assert!(!desktop.lines().any(|l| l.starts_with("MimeType=")), "the desktop entry declares MIME types");
        assert!(!metainfo.contains("<mediatype>"), "the AppStream metadata declares media types");
    }
}
