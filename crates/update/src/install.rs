//! How this copy was installed, which of a release's files is its own, and what a click on the
//! update button does.
//!
//! The install kind is found in this order: Flatpak (`/.flatpak-info` exists), AppImage
//! (`$APPIMAGE` is set), the one-word marker file the packaging writes (`deb`, `rpm`, `tar`, `msi`
//! or `portable`: `install-kind` beside the binary on Windows, `../share/dbee/install-kind` from
//! the binary's folder elsewhere), a macOS app bundle whose `Info.plist` names dbee's bundle id,
//! else unknown. Everything read from the machine comes in through [`Probe`].

use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};

use semver::Version;

use crate::UpdateError;
use crate::fetch::{Repo, release_page};

/// The marker file's name.
pub const MARKER_FILE: &str = "install-kind";

/// The macOS bundle id.
pub const BUNDLE_ID: &str = "dev.rosin.dbee";

/// The most a marker file may hold.
const MAX_MARKER: usize = 32;

/// The most of an `Info.plist` that is read.
const MAX_PLIST: usize = 64 * 1024;

/// An operating system releases are built for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Linux,
    Windows,
    MacOs,
    FreeBsd,
    /// Any other: no release file is its own.
    Other,
}

/// What a binary was built for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Platform {
    pub os: Os,
    /// The binary's `target_arch`: `x86_64`, `aarch64`, `x86`, `riscv64`.
    pub arch: &'static str,
}

impl Platform {
    /// This binary's.
    pub fn current() -> Platform {
        let os = if cfg!(target_os = "linux") {
            Os::Linux
        } else if cfg!(target_os = "windows") {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else if cfg!(target_os = "freebsd") {
            Os::FreeBsd
        } else {
            Os::Other
        };
        Platform { os, arch: std::env::consts::ARCH }
    }

    /// The architecture as this platform's asset names write it.
    fn asset_arch(self) -> Option<&'static str> {
        match (self.os, self.arch) {
            (Os::MacOs, _) => Some("universal"),
            (Os::Windows, "x86_64") => Some("x64"),
            (Os::Windows, "aarch64") => Some("arm64"),
            (Os::Windows, "x86") => Some("x86"),
            (Os::Linux, "x86_64") | (Os::FreeBsd, "x86_64") => Some("x86_64"),
            (Os::Linux, "aarch64") => Some("aarch64"),
            (Os::Linux, "riscv64") => Some("riscv64"),
            _ => None,
        }
    }

    fn asset_os(self) -> Option<&'static str> {
        match self.os {
            Os::Linux => Some("linux"),
            Os::Windows => Some("windows"),
            Os::MacOs => Some("macos"),
            Os::FreeBsd => Some("freebsd"),
            Os::Other => None,
        }
    }
}

/// How a copy of dbee was installed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallKind {
    Flatpak,
    AppImage,
    Deb,
    Rpm,
    /// The `.tar.gz` tree, unpacked anywhere.
    Tar,
    Msi,
    /// The Windows portable zip.
    Portable,
    /// The macOS app bundle.
    MacApp,
    /// A build from source, a distribution's own package, or anything else.
    Unknown,
}

impl InstallKind {
    pub const ALL: [InstallKind; 9] = [
        InstallKind::Flatpak,
        InstallKind::AppImage,
        InstallKind::Deb,
        InstallKind::Rpm,
        InstallKind::Tar,
        InstallKind::Msi,
        InstallKind::Portable,
        InstallKind::MacApp,
        InstallKind::Unknown,
    ];

    /// A stable id (`deb`, `appImage`, `macApp`, `unknown`).
    pub fn id(self) -> &'static str {
        match self {
            InstallKind::Flatpak => "flatpak",
            InstallKind::AppImage => "appImage",
            InstallKind::Deb => "deb",
            InstallKind::Rpm => "rpm",
            InstallKind::Tar => "tar",
            InstallKind::Msi => "msi",
            InstallKind::Portable => "portable",
            InstallKind::MacApp => "macApp",
            InstallKind::Unknown => "unknown",
        }
    }

    /// The kind a marker file's word names on `os`; a word that is no marker, or one of another
    /// system's formats, names none.
    fn from_marker(word: &str, os: Os) -> Option<InstallKind> {
        match (word, os) {
            ("deb", Os::Linux) => Some(InstallKind::Deb),
            ("rpm", Os::Linux) => Some(InstallKind::Rpm),
            ("tar", Os::Linux | Os::FreeBsd) => Some(InstallKind::Tar),
            ("msi", Os::Windows) => Some(InstallKind::Msi),
            ("portable", Os::Windows) => Some(InstallKind::Portable),
            _ => None,
        }
    }

    /// How the copy that [`Probe`] describes was installed.
    pub fn detect(probe: &Probe<'_>) -> InstallKind {
        if probe.os == Os::Linux {
            if (probe.read)(probe.flatpak_info, 1).is_some() {
                return InstallKind::Flatpak;
            }
            if probe.appimage.is_some_and(|path| !path.is_empty()) {
                return InstallKind::AppImage;
            }
        }
        let Some(dir) = probe.exe.parent() else { return InstallKind::Unknown };
        let marker = if probe.os == Os::Windows { dir.join(MARKER_FILE) } else { share_of(dir).join("dbee").join(MARKER_FILE) };
        let marked = (probe.read)(&marker, MAX_MARKER)
            .filter(|bytes| bytes.len() <= MAX_MARKER)
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .and_then(|text| InstallKind::from_marker(text.strip_suffix('\n').unwrap_or(&text), probe.os));
        if let Some(kind) = marked {
            return kind;
        }
        if probe.os == Os::MacOs && in_dbee_bundle(probe, dir) {
            return InstallKind::MacApp;
        }
        InstallKind::Unknown
    }

    /// How this process was installed, read from the machine.
    pub fn of_this_process() -> InstallKind {
        let Ok(exe) = std::env::current_exe() else { return InstallKind::Unknown };
        // Through links, so `/usr/local/bin/dbee -> /opt/dbee/bin/dbee` finds its own tree.
        let exe = exe.canonicalize().unwrap_or(exe);
        let appimage = std::env::var_os("APPIMAGE");
        InstallKind::detect(&Probe {
            os: Platform::current().os,
            exe: &exe,
            flatpak_info: Path::new("/.flatpak-info"),
            appimage: appimage.as_deref(),
            read: &read_start,
        })
    }
}

/// `<prefix>/share` for a binary in `<prefix>/bin`.
fn share_of(bin: &Path) -> PathBuf {
    bin.parent().map_or_else(|| PathBuf::from("share"), |prefix| prefix.join("share"))
}

/// Whether `dir`, the binary's folder, is `<name>.app/Contents/MacOS` of a bundle whose
/// `Info.plist` gives dbee's bundle id.
fn in_dbee_bundle(probe: &Probe<'_>, dir: &Path) -> bool {
    let Some(contents) = dir.parent() else { return false };
    let Some(bundle) = contents.parent() else { return false };
    let named = |p: &Path, name: &str| p.file_name().is_some_and(|n| n == name);
    if !named(dir, "MacOS") || !named(contents, "Contents") || bundle.extension().is_none_or(|e| e != "app") {
        return false;
    }
    let Some(plist) = (probe.read)(&contents.join("Info.plist"), MAX_PLIST) else { return false };
    let Ok(text) = String::from_utf8(plist) else { return false };
    let Some((_, after)) = text.split_once("<key>CFBundleIdentifier</key>") else { return false };
    let value = after.trim_start().strip_prefix("<string>").and_then(|rest| rest.split_once("</string>")).map(|(id, _)| id);
    value == Some(BUNDLE_ID)
}

/// What [`InstallKind::detect`] reads from the machine.
pub struct Probe<'a> {
    pub os: Os,
    /// The running binary, links resolved.
    pub exe: &'a Path,
    /// `/.flatpak-info`.
    pub flatpak_info: &'a Path,
    /// `$APPIMAGE`.
    pub appimage: Option<&'a OsStr>,
    /// The first bytes of a file, at most the number given and one more; `None` when it cannot be
    /// read.
    pub read: &'a dyn Fn(&Path, usize) -> Option<Vec<u8>>,
}

/// The first `max + 1` bytes of the file at `path`, so a caller sees when it holds more.
fn read_start(path: &Path, max: usize) -> Option<Vec<u8>> {
    let limit = u64::try_from(max).ok()?.checked_add(1)?;
    let mut bytes = Vec::new();
    std::fs::File::open(path).ok()?.take(limit).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// Whether the binary at `exe` runs from a cargo target folder, so is a development build: a
/// folder above it holds cargo's `.rustc_info.json`.
pub fn is_development_build(exe: &Path, exists: &dyn Fn(&Path) -> bool) -> bool {
    exe.ancestors().skip(1).take(6).any(|dir| exists(&dir.join(".rustc_info.json")))
}

/// The name of the release file a copy installed as `kind` on `platform` updates from, for
/// release `version`; `None` when no file is its own.
pub fn asset_name(kind: InstallKind, platform: Platform, version: &Version) -> Option<String> {
    let (os, arch) = (platform.asset_os()?, platform.asset_arch()?);
    let ext = match (kind, platform.os) {
        (InstallKind::Flatpak, Os::Linux) if platform.arch != "riscv64" => "flatpak",
        (InstallKind::AppImage, Os::Linux) if platform.arch != "riscv64" => "AppImage",
        (InstallKind::Deb, Os::Linux) => "deb",
        (InstallKind::Rpm, Os::Linux) => "rpm",
        (InstallKind::Tar, Os::Linux | Os::FreeBsd) => "tar.gz",
        (InstallKind::Msi, Os::Windows) => "msi",
        (InstallKind::Portable, Os::Windows) => return Some(format!("dbee-{version}-{os}-{arch}-portable.zip")),
        (InstallKind::MacApp, Os::MacOs) => "dmg",
        _ => return None,
    };
    Some(format!("dbee-{version}-{os}-{arch}.{ext}"))
}

/// What a click on the update button does, as data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// dbee installs the release itself, from this file of it.
    Install { asset: String },
    /// dbee opens the release's page, for a copy that cannot update itself.
    OpenPage { url: String },
}

/// A newer release offered to this copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    pub from: Version,
    pub to: Version,
    pub kind: InstallKind,
    pub action: Action,
}

/// The offer a copy at `current`, installed as `kind` on `platform`, gets when the newest release
/// is `latest`: none unless `latest` is newer ([`crate::is_newer`]). The action is
/// [`Action::Install`] only with `installer` on, for a kind that has a file of its own and is not
/// kept up to date by something else (Flatpak, an unknown install); every other copy opens the
/// release's page.
pub fn offer(current: &Version, latest: &Version, kind: InstallKind, platform: Platform, repo: &Repo, installer: bool) -> Option<Offer> {
    if !crate::is_newer(current, latest) {
        return None;
    }
    let own = match kind {
        InstallKind::Flatpak | InstallKind::Unknown => None,
        _ => asset_name(kind, platform, latest),
    };
    let action = match own {
        Some(asset) if installer => Action::Install { asset },
        _ => Action::OpenPage { url: release_page(repo, latest) },
    };
    Some(Offer { from: current.clone(), to: latest.clone(), kind, action })
}

/// The step that installs a release. None exists yet: the engine takes one through its options,
/// and without one every offer opens the release's page.
pub trait Install: Send + Sync {
    /// Whether it can update a copy installed as `kind`.
    fn supports(&self, kind: InstallKind) -> bool;

    /// Installs `offer`'s release, calling `progress` as it goes. It runs on a blocking thread.
    fn install(&self, offer: &Offer, progress: &dyn Fn(Progress)) -> Result<(), UpdateError>;
}

/// How far an install is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    /// This many hundredths of the download are in (0 to 100).
    Percent(u8),
    /// A step with no measure, such as the system's own installer running.
    Working,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::manifest::Manifest;
    use crate::manifest::tests::{manifest_text, release_names};

    /// A pretend machine: its files by path.
    #[derive(Default)]
    struct Tree(HashMap<PathBuf, Vec<u8>>);

    impl Tree {
        fn with(mut self, path: &str, content: &str) -> Tree {
            self.0.insert(PathBuf::from(path), content.as_bytes().to_vec());
            self
        }

        fn kind(&self, os: Os, exe: &str, appimage: Option<&str>) -> InstallKind {
            let read = |path: &Path, max: usize| self.0.get(path).map(|bytes| bytes.iter().copied().take(max.saturating_add(1)).collect());
            InstallKind::detect(&Probe {
                os,
                exe: Path::new(exe),
                flatpak_info: Path::new("/.flatpak-info"),
                appimage: appimage.map(OsStr::new),
                read: &read,
            })
        }
    }

    const PLIST: &str = "<plist version=\"1.0\">\n<dict>\n  <key>CFBundleIdentifier</key>\n  <string>dev.rosin.dbee</string>\n</dict>\n</plist>\n";

    #[test]
    fn each_package_is_known_by_its_marker() {
        for (word, kind) in [("deb", InstallKind::Deb), ("rpm", InstallKind::Rpm), ("tar", InstallKind::Tar)] {
            let tree = Tree::default().with("/usr/share/dbee/install-kind", &format!("{word}\n"));
            assert_eq!(tree.kind(Os::Linux, "/usr/bin/dbee", None), kind);
        }
        let tree = Tree::default().with("/home/u/dbee-0.5.0-linux-x86_64/share/dbee/install-kind", "tar\n");
        assert_eq!(tree.kind(Os::Linux, "/home/u/dbee-0.5.0-linux-x86_64/bin/dbee", None), InstallKind::Tar);
        assert_eq!(tree.kind(Os::FreeBsd, "/home/u/dbee-0.5.0-linux-x86_64/bin/dbee", None), InstallKind::Tar);
        for (word, kind) in [("msi", InstallKind::Msi), ("portable", InstallKind::Portable)] {
            let tree = Tree::default().with("C:/Program Files/dbee/install-kind", word);
            assert_eq!(tree.kind(Os::Windows, "C:/Program Files/dbee/dbee.exe", None), kind);
        }
    }

    #[test]
    fn flatpak_comes_first_then_appimage_then_the_marker() {
        let tree = Tree::default().with("/.flatpak-info", "[Application]\n").with("/app/share/dbee/install-kind", "deb\n");
        assert_eq!(tree.kind(Os::Linux, "/app/bin/dbee", Some("/x/dbee.AppImage")), InstallKind::Flatpak);
        let tree = Tree::default().with("/tmp/.mount_dbee/usr/share/dbee/install-kind", "deb\n");
        assert_eq!(tree.kind(Os::Linux, "/tmp/.mount_dbee/usr/bin/dbee", Some("/home/u/dbee.AppImage")), InstallKind::AppImage);
        assert_eq!(tree.kind(Os::Linux, "/tmp/.mount_dbee/usr/bin/dbee", Some("")), InstallKind::Deb, "an empty $APPIMAGE names no AppImage");
        assert_eq!(tree.kind(Os::Linux, "/tmp/.mount_dbee/usr/bin/dbee", None), InstallKind::Deb);
    }

    #[test]
    fn an_empty_flatpak_info_still_means_flatpak() {
        let tree = Tree::default().with("/.flatpak-info", "");
        assert_eq!(tree.kind(Os::Linux, "/app/bin/dbee", None), InstallKind::Flatpak);
        assert_eq!(tree.kind(Os::Windows, "C:/dbee/dbee.exe", None), InstallKind::Unknown, "only Linux has Flatpak");
    }

    #[test]
    fn a_marker_that_is_no_marker_means_unknown() {
        for content in [
            "",
            "\n",
            "deb \n",
            " deb\n",
            "DEB\n",
            "deb\n\n",
            "deb\r\n",
            "snap\n",
            "deb rpm\n",
            "msi\n",
            "portable\n",
            "../../etc\n",
            "debdebdebdebdebdebdebdebdebdebdebdebdeb\n",
        ] {
            let tree = Tree::default().with("/usr/share/dbee/install-kind", content);
            assert_eq!(tree.kind(Os::Linux, "/usr/bin/dbee", None), InstallKind::Unknown, "{content:?}");
        }
        let tree = Tree(HashMap::from([(PathBuf::from("/usr/share/dbee/install-kind"), vec![0xff, 0xfe])]));
        assert_eq!(tree.kind(Os::Linux, "/usr/bin/dbee", None), InstallKind::Unknown);
        // A Windows word on Linux and a Linux word on Windows name nothing.
        let tree = Tree::default().with("C:/dbee/install-kind", "deb");
        assert_eq!(tree.kind(Os::Windows, "C:/dbee/dbee.exe", None), InstallKind::Unknown);
    }

    #[test]
    fn no_marker_means_unknown() {
        let tree = Tree::default();
        assert_eq!(tree.kind(Os::Linux, "/usr/bin/dbee", None), InstallKind::Unknown);
        assert_eq!(tree.kind(Os::Linux, "/home/u/dbee/target/release/dbee", None), InstallKind::Unknown);
        assert_eq!(tree.kind(Os::Linux, "dbee", None), InstallKind::Unknown);
        assert_eq!(tree.kind(Os::Linux, "/", None), InstallKind::Unknown);
        // The marker beside the binary is Windows's place, not Linux's.
        let tree = Tree::default().with("/usr/bin/install-kind", "deb\n");
        assert_eq!(tree.kind(Os::Linux, "/usr/bin/dbee", None), InstallKind::Unknown);
    }

    #[test]
    fn a_macos_bundle_is_known_by_its_bundle_id() {
        let tree = Tree::default().with("/Applications/dbee.app/Contents/Info.plist", PLIST);
        assert_eq!(tree.kind(Os::MacOs, "/Applications/dbee.app/Contents/MacOS/dbee", None), InstallKind::MacApp);
        let renamed = Tree::default().with("/Users/u/Apps/dbee 2.app/Contents/Info.plist", PLIST);
        assert_eq!(renamed.kind(Os::MacOs, "/Users/u/Apps/dbee 2.app/Contents/MacOS/dbee", None), InstallKind::MacApp);
        // Not a bundle, another app's bundle, a plist without the id, or another system.
        assert_eq!(tree.kind(Os::MacOs, "/usr/local/bin/dbee", None), InstallKind::Unknown);
        assert_eq!(tree.kind(Os::MacOs, "/Applications/dbee.app/Contents/Helpers/dbee", None), InstallKind::Unknown);
        assert_eq!(tree.kind(Os::Linux, "/Applications/dbee.app/Contents/MacOS/dbee", None), InstallKind::Unknown);
        let other = Tree::default().with("/Applications/other.app/Contents/Info.plist", &PLIST.replace("dev.rosin.dbee", "com.example.other"));
        assert_eq!(other.kind(Os::MacOs, "/Applications/other.app/Contents/MacOS/dbee", None), InstallKind::Unknown);
        let elsewhere = Tree::default().with("/A/x.app/Contents/Info.plist", "<key>CFBundleName</key><string>dev.rosin.dbee</string>");
        assert_eq!(elsewhere.kind(Os::MacOs, "/A/x.app/Contents/MacOS/dbee", None), InstallKind::Unknown);
        let folder = Tree::default().with("/A/dbee/Contents/Info.plist", PLIST);
        assert_eq!(folder.kind(Os::MacOs, "/A/dbee/Contents/MacOS/dbee", None), InstallKind::Unknown);
    }

    #[test]
    fn a_binary_under_a_cargo_target_folder_is_a_development_build() {
        let has = |paths: &'static [&'static str]| move |p: &Path| paths.iter().any(|x| Path::new(x) == p);
        assert!(is_development_build(Path::new("/src/dbee/target/debug/dbee"), &has(&["/src/dbee/target/.rustc_info.json"])));
        assert!(is_development_build(
            Path::new("/src/dbee/target/x86_64-unknown-linux-gnu/release/dbee"),
            &has(&["/src/dbee/target/.rustc_info.json"])
        ));
        assert!(is_development_build(Path::new("/src/dbee/target/debug/deps/m1_day-0123"), &has(&["/src/dbee/target/.rustc_info.json"])));
        assert!(!is_development_build(Path::new("/usr/bin/dbee"), &has(&[])));
        assert!(!is_development_build(Path::new("/usr/bin/dbee"), &has(&["/usr/bin/dbee/.rustc_info.json"])), "the binary itself is no folder");
        assert!(!is_development_build(Path::new("dbee"), &has(&[])));
    }

    const ARCHES: [&str; 4] = ["x86_64", "aarch64", "x86", "riscv64"];

    fn platforms() -> Vec<Platform> {
        let mut all = Vec::new();
        for os in [Os::Linux, Os::Windows, Os::MacOs, Os::FreeBsd, Os::Other] {
            for arch in ARCHES {
                all.push(Platform { os, arch });
            }
        }
        all
    }

    /// Every file of a release that is an app to install is some copy's own, the names the picks
    /// make are all a release's, and the files no copy picks are the ones that install nothing.
    #[test]
    fn the_picks_cover_the_releases_files() {
        let v = Version::new(0, 5, 0);
        let names = release_names("0.5.0");
        let manifest = Manifest::parse(manifest_text("0.5.0").as_bytes()).unwrap();
        let mut picked: Vec<String> = Vec::new();
        for platform in platforms() {
            for kind in InstallKind::ALL {
                if let Some(name) = asset_name(kind, platform, &v) {
                    // The only names a pick makes that no release has: riscv64 builds exist for Linux alone.
                    assert!(manifest.asset(&name).is_some(), "{kind:?} on {platform:?} picks {name}, which no release has");
                    if !picked.contains(&name) {
                        picked.push(name);
                    }
                }
            }
        }
        let unpicked: Vec<&String> = names.iter().filter(|n| !picked.contains(n)).collect();
        assert_eq!(
            unpicked,
            ["dbee-cli-0.5.0-macos-universal.zip", "dbee-0.5.0-linux-x86_64.AppImage.zsync", "dbee-0.5.0-linux-aarch64.AppImage.zsync"]
        );
    }

    #[test]
    fn each_copy_picks_its_own_file() {
        let v = Version::parse("0.5.0").unwrap();
        let on = |os, arch| Platform { os, arch };
        for (kind, platform, name) in [
            (InstallKind::Deb, on(Os::Linux, "x86_64"), Some("dbee-0.5.0-linux-x86_64.deb")),
            (InstallKind::Deb, on(Os::Linux, "aarch64"), Some("dbee-0.5.0-linux-aarch64.deb")),
            (InstallKind::Deb, on(Os::Linux, "riscv64"), Some("dbee-0.5.0-linux-riscv64.deb")),
            (InstallKind::Rpm, on(Os::Linux, "x86_64"), Some("dbee-0.5.0-linux-x86_64.rpm")),
            (InstallKind::Rpm, on(Os::Linux, "riscv64"), Some("dbee-0.5.0-linux-riscv64.rpm")),
            (InstallKind::Tar, on(Os::Linux, "aarch64"), Some("dbee-0.5.0-linux-aarch64.tar.gz")),
            (InstallKind::Tar, on(Os::Linux, "riscv64"), Some("dbee-0.5.0-linux-riscv64.tar.gz")),
            (InstallKind::Tar, on(Os::FreeBsd, "x86_64"), Some("dbee-0.5.0-freebsd-x86_64.tar.gz")),
            (InstallKind::AppImage, on(Os::Linux, "x86_64"), Some("dbee-0.5.0-linux-x86_64.AppImage")),
            (InstallKind::AppImage, on(Os::Linux, "aarch64"), Some("dbee-0.5.0-linux-aarch64.AppImage")),
            (InstallKind::Flatpak, on(Os::Linux, "x86_64"), Some("dbee-0.5.0-linux-x86_64.flatpak")),
            (InstallKind::Msi, on(Os::Windows, "x86_64"), Some("dbee-0.5.0-windows-x64.msi")),
            (InstallKind::Msi, on(Os::Windows, "aarch64"), Some("dbee-0.5.0-windows-arm64.msi")),
            (InstallKind::Msi, on(Os::Windows, "x86"), Some("dbee-0.5.0-windows-x86.msi")),
            (InstallKind::Portable, on(Os::Windows, "x86_64"), Some("dbee-0.5.0-windows-x64-portable.zip")),
            (InstallKind::Portable, on(Os::Windows, "aarch64"), Some("dbee-0.5.0-windows-arm64-portable.zip")),
            (InstallKind::Portable, on(Os::Windows, "x86"), Some("dbee-0.5.0-windows-x86-portable.zip")),
            (InstallKind::MacApp, on(Os::MacOs, "aarch64"), Some("dbee-0.5.0-macos-universal.dmg")),
            (InstallKind::MacApp, on(Os::MacOs, "x86_64"), Some("dbee-0.5.0-macos-universal.dmg")),
            // No file of its own.
            (InstallKind::Unknown, on(Os::Linux, "x86_64"), None),
            (InstallKind::AppImage, on(Os::Linux, "riscv64"), None),
            (InstallKind::Flatpak, on(Os::Linux, "riscv64"), None),
            (InstallKind::Tar, on(Os::FreeBsd, "aarch64"), None),
            (InstallKind::Deb, on(Os::Windows, "x86_64"), None),
            (InstallKind::Msi, on(Os::Linux, "x86_64"), None),
            (InstallKind::Msi, on(Os::Windows, "riscv64"), None),
            (InstallKind::MacApp, on(Os::Linux, "x86_64"), None),
            (InstallKind::Tar, on(Os::Other, "x86_64"), None),
        ] {
            assert_eq!(asset_name(kind, platform, &v).as_deref(), name, "{kind:?} on {platform:?}");
        }
        let pre = Version::parse("0.6.0-rc.1").unwrap();
        assert_eq!(asset_name(InstallKind::Deb, on(Os::Linux, "x86_64"), &pre).unwrap(), "dbee-0.6.0-rc.1-linux-x86_64.deb");
    }

    #[test]
    fn every_pick_is_an_asset_name() {
        let v = Version::parse("0.6.0-rc.1").unwrap();
        for platform in platforms() {
            for kind in InstallKind::ALL {
                if let Some(name) = asset_name(kind, platform, &v) {
                    assert_eq!(crate::manifest::asset_version(&name), Some(v.clone()), "{name}");
                }
            }
        }
    }

    #[test]
    fn without_an_installer_every_offer_opens_the_page() {
        let repo = Repo::default();
        let (from, to) = (Version::new(0, 3, 0), Version::new(0, 3, 1));
        let linux = Platform { os: Os::Linux, arch: "x86_64" };
        for kind in InstallKind::ALL {
            let o = offer(&from, &to, kind, linux, &repo, false).unwrap();
            assert_eq!(o.action, Action::OpenPage { url: "https://github.com/KasparRosin/dbee/releases/tag/v0.3.1".into() }, "{kind:?}");
            assert_eq!((o.from.to_string(), o.to.to_string(), o.kind), ("0.3.0".into(), "0.3.1".into(), kind));
        }
        assert_eq!(offer(&to, &to, InstallKind::Deb, linux, &repo, false), None);
        assert_eq!(offer(&to, &from, InstallKind::Deb, linux, &repo, false), None);
    }

    #[test]
    fn with_an_installer_a_copy_with_a_file_of_its_own_installs() {
        let repo = Repo::default();
        let (from, to) = (Version::new(0, 3, 0), Version::new(0, 4, 0));
        let linux = Platform { os: Os::Linux, arch: "x86_64" };
        let action = |kind, platform| offer(&from, &to, kind, platform, &repo, true).unwrap().action;
        assert_eq!(action(InstallKind::Deb, linux), Action::Install { asset: "dbee-0.4.0-linux-x86_64.deb".into() });
        assert_eq!(action(InstallKind::AppImage, linux), Action::Install { asset: "dbee-0.4.0-linux-x86_64.AppImage".into() });
        // Flatpak and an unknown install are kept up to date by something else; riscv64 has no AppImage.
        for (kind, platform) in
            [(InstallKind::Flatpak, linux), (InstallKind::Unknown, linux), (InstallKind::AppImage, Platform { os: Os::Linux, arch: "riscv64" })]
        {
            assert!(matches!(action(kind, platform), Action::OpenPage { .. }), "{kind:?}");
        }
    }
}
