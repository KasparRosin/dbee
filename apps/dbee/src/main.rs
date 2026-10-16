//! dbee desktop app. Placeholder: an empty window titled `dbee <version>`.
//!
//! The scaffold ships this window so packaging and the release pipeline can be proven before any
//! feature exists. The logger (`logging.rs`) and the Windows graphics start-up (`graphics.rs`)
//! are the real thing and stay.
#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

#[cfg(any(target_os = "windows", test))]
mod graphics;
mod logging;

use std::path::PathBuf;

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

/// Runs without preferences (`DBEE_NO_PREFS`, agents' test runs) keep no log file.
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

struct Placeholder;

impl eframe::App for Placeholder {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.centered_and_justified(|ui| {
            ui.heading(format!("dbee {VERSION}"));
        });
    }
}

fn main() -> eframe::Result<()> {
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
        if log::log_enabled!(log::Level::Error) {
            log::error!("internal error: {info}");
        } else {
            let _ = std::io::Write::write_fmt(&mut std::io::stderr(), format_args!("dbee: internal error: {info}\n"));
        }
    }));
    if std::env::args_os().skip(1).any(|a| a == "--version" || a == "-V") {
        let _ = std::io::Write::write_fmt(&mut std::io::stdout(), format_args!("dbee {VERSION}\n"));
        return Ok(());
    }
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(format!("dbee {VERSION}"))
        .with_inner_size([960.0, 600.0])
        .with_min_inner_size([480.0, 320.0])
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
            Box::new(|_cc| {
                created.set(true);
                Ok(Box::new(Placeholder))
            }),
        )
    };
    let result = run(options);
    // A graphics error before the window existed: start once more with the other backend.
    #[cfg(target_os = "windows")]
    if let Err(eframe::Error::Wgpu(e)) = &result
        && !created.get()
        && let Some(backends) = graphics::fallback_backends(backend_override, first_backends)
    {
        log::error!("graphics: the renderer didn't start on {first_backends:?} ({e}); trying {backends:?}");
        let mut options = retry_options;
        graphics::configure_backends(&mut options, backends);
        return run(options);
    }
    result
}

#[cfg(test)]
mod packaging_tests {
    /// The text between `start` and `end` that follows each `marker` in `text`, sorted.
    fn extensions(text: &str, marker: &str, start: &str, end: char) -> Vec<String> {
        let mut v: Vec<String> = text.split(marker).skip(1).filter_map(|s| s.split_once(start)?.1.split(end).next()).map(str::to_string).collect();
        v.sort();
        v.dedup();
        v
    }

    /// macOS Open With, Windows Open with and the Linux MIME globs offer dbee for the same files.
    #[test]
    fn all_platforms_offer_the_same_file_types() {
        let plist = include_str!("../../../packaging/macos/Info.plist.in");
        let wxs = include_str!("../../../packaging/windows/dbee.wxs");
        let mime = include_str!("../../../packaging/linux/dev.rosin.dbee.mime.xml");
        // One array holds both extensions, so read every <string> up to its closing tag.
        let mut mac: Vec<String> = plist
            .split("<key>CFBundleTypeExtensions</key>")
            .skip(1)
            .filter_map(|s| s.split("</array>").next())
            .flat_map(|a| a.split("<string>").skip(1).filter_map(|x| x.split('<').next()).map(str::to_string))
            .collect();
        mac.sort();
        mac.dedup();
        let win = extensions(wxs, "Key=\"Software\\Classes\\.", "", '\\');
        let linux = extensions(mime, "<glob pattern=\"*.", "", '"');
        assert_eq!(mac, vec!["sqlite".to_string(), "sqlite3".into()]);
        assert_eq!(win, mac);
        assert_eq!(linux, mac);
    }
}
