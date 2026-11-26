//! Opening a page in the browser.
//!
//! The desktop's own opener is started with the page's address as its one argument, through no
//! shell: `xdg-open` on Linux and FreeBSD, found on `PATH` (inside a Flatpak it asks the desktop's
//! portal), `/usr/bin/open` on macOS, and on Windows `rundll32 url.dll,FileProtocolHandler`.
//! Nothing is linked in for it: egui's own way (`Context::open_url`, eframe's `links` feature)
//! brings a URL library and its Unicode tables, 28 crates, and a page that does not open is one
//! line in its log. dbee was built without that feature, so 0.3.1 and 0.3.2 opened nothing on a
//! click.
//!
//! Only a plain web page's address is passed on ([`fits`]): `https://`, then letters, digits and
//! `-`, `.`, `_`, `/`, which is what a release's page is made of (the one caller hands in the
//! repository's address with a version the manifest was read for). No shell reads the address,
//! and with this set an opener has nothing to read into it either: no pattern, no `%`, no `@`,
//! no query. On Linux, FreeBSD and macOS the opener is started in `/`, not in the folder dbee was
//! started in: some openers (KDE's) take a file there of the address's own name for what to
//! open. Windows needs none of it, where no file can have such a name. There too it is in a
//! process group of its own, so what ends dbee from its terminal does not end a browser the
//! opener stayed with.
//!
//! A click that opens nothing has to say so. A failure to start the opener is the answer of
//! [`open`]. One that shows only when the opener ends (no browser to hand the page to) is told
//! through `failed`, from a thread that waits for the opener, so it is not left behind as a
//! zombie; an opener that ends in failure later than [`TOLD_WITHIN`] is only logged, since some
//! stay for as long as the browser they started. What is not told: an opener that ends well and
//! opened nothing, which is every failure on Windows, where `rundll32` ends well whatever
//! happened.

use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Where a failure that shows only after [`Browser`] answered is told: at most once, from another
/// thread.
pub type Failed = Box<dyn FnOnce(String) + Send>;

/// What opens a page for the app ([`crate::DbeeApp::with_browser`]): the page's address, and
/// where a late failure goes. `Err` says why nothing was started.
pub type Browser = Arc<dyn Fn(&str, Failed) -> Result<(), String> + Send + Sync>;

/// The longest address passed on.
pub const MAX_ADDRESS: usize = 2048;

/// How long after its start an opener's failure is still told to the user.
pub const TOLD_WITHIN: Duration = Duration::from_secs(10);

/// The desktop's opener ([`open`]).
pub fn system() -> Browser {
    Arc::new(open)
}

/// The opener of this system, and what it takes before the address.
fn opener() -> (&'static str, &'static [&'static str]) {
    if cfg!(target_os = "macos") {
        // Its place is fixed, so `PATH` is not asked.
        ("/usr/bin/open", &[])
    } else if cfg!(windows) {
        ("rundll32", &["url.dll,FileProtocolHandler"])
    } else {
        ("xdg-open", &[])
    }
}

/// Whether `url` is a web page's address an opener may be handed: `https://` and then, to at most
/// [`MAX_ADDRESS`] in all, ASCII letters, digits and `-`, `.`, `_`, `/`. A release's page needs no
/// more, and every other mark means something to one opener or another (see the module docs); a
/// caller that needs one adds it here with the reason it is safe.
pub fn fits(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    !rest.is_empty() && url.len() <= MAX_ADDRESS && rest.chars().all(|c| c.is_ascii_alphanumeric() || "-._/".contains(c))
}

/// Opens `url` in the browser with the desktop's opener. `Err` says why the opener was not
/// started; a failure it ends with is told to `failed` (see the module docs).
pub fn open(url: &str, failed: Failed) -> Result<(), String> {
    let (program, before) = opener();
    start(program, before, url, failed)
}

/// Starts `program` with `before` and then `url` as its arguments, and waits for it on a thread
/// of its own.
fn start(program: &str, before: &[&str], url: &str, failed: Failed) -> Result<(), String> {
    if !fits(url) {
        return Err("it is not a web page's address".into());
    }
    let mut command = Command::new(program);
    command.args(before).arg(url).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // Away from the folder dbee was started in, where a file of the address's name could be
    // taken for the page, and out of dbee's process group (see the module docs).
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.current_dir("/").process_group(0);
    }
    let mut child = command.spawn().map_err(|e| format!("`{program}` did not start: {e}"))?;
    let (program, started) = (program.to_owned(), Instant::now());
    let waiting = std::thread::Builder::new().name("dbee-open-page".into()).spawn(move || {
        let why = match child.wait() {
            Ok(status) if status.success() => return,
            Ok(status) => format!("`{program}` ended with {status}"),
            // How it ended is not known (the system took it, where dbee's starter left child
            // exits ignored): no reason to say the page did not open.
            Err(e) => return log::warn!("opening a page: `{program}` could not be waited for: {e}"),
        };
        log::warn!("opening a page: {why}");
        if started.elapsed() <= TOLD_WITHIN {
            failed(why);
        }
    });
    if let Err(e) = waiting {
        // The opener runs; only nobody waits for it.
        log::warn!("opening a page: no thread to wait for the opener: {e}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    const PAGE: &str = "https://github.com/KasparRosin/dbee/releases/tag/v0.3.2";

    #[test]
    fn only_a_web_pages_address_fits() {
        for url in [PAGE, "https://github.com/Some-One/a.b_c/releases/tag/v0.4.0-rc.1", "https://example.org"] {
            assert!(fits(url), "{url}");
        }
        let long = format!("https://example.org/{}", "a".repeat(MAX_ADDRESS));
        for url in [
            "",
            "https://",
            "http://example.org/",
            "https://example.org/?q=1",
            "https://example.org/a*",
            "https://example.org/%55",
            "https://example.org/#top",
            "https://github.com@example.org/",
            "https://example.org:8443/",
            "https://example.org/v0.3.3+a-word-from-the-manifest",
            "https://example.org/a;b",
            "https://example.org/a&b",
            "https://example.org/~x",
            "https://example.org/a,b=c!d",
            "file:///etc/passwd",
            "-https://example.org/",
            " https://example.org/",
            "https://example.org/a b",
            "https://example.org/\"",
            "https://example.org/'x'",
            "https://example.org/a\\b",
            "https://example.org/a\nb",
            "https://example.org/a\0b",
            "https://example.org/`id`",
            "https://example.org/$(id)|x",
            "https://example.org/<x>",
            "https://example.org/^x",
            "https://example.org/ü",
            long.as_str(),
        ] {
            assert!(!fits(url), "{url:?}");
        }
        assert!(fits(&long[..MAX_ADDRESS]));
    }

    /// A callback that hands what it is told to the receiver.
    fn told() -> (Failed, mpsc::Receiver<String>) {
        let (tx, rx) = mpsc::channel();
        (
            Box::new(move |why| {
                let _ = tx.send(why);
            }),
            rx,
        )
    }

    #[test]
    fn an_address_that_does_not_fit_starts_nothing() {
        let (failed, rx) = told();
        // `true` would start anywhere: the address is refused before it.
        assert_eq!(start("true", &[], "http://example.org/", failed), Err("it is not a web page's address".into()));
        assert!(rx.recv().is_err(), "nothing is told later: the callback was dropped");
    }

    #[test]
    fn an_opener_that_is_not_there_says_so_at_once() {
        let (failed, rx) = told();
        let why = start("dbee-no-such-opener", &[], PAGE, failed).unwrap_err();
        assert!(why.starts_with("`dbee-no-such-opener` did not start: "), "{why}");
        assert!(rx.recv().is_err());
    }

    /// The opener here is the shell with a script of the test's (`sh -c <script> <how it ends>
    /// <address>`), so no file is written and then run.
    #[cfg(unix)]
    #[test]
    fn an_opener_gets_the_address_as_its_one_argument_in_the_root_folder_and_its_failure_is_told() {
        let dir = dbee_testkit::temp_home(None, None);
        let seen = dir.home.join("seen");
        // What it was started with, one argument a line, then where; it ends as `$0` says.
        let script = format!("{{ for a in \"$@\"; do echo \"$a\"; done; pwd; }} > '{}'; exit \"$0\"", seen.display());

        let (failed, rx) = told();
        assert_eq!(start("sh", &["-c", &script, "0"], PAGE, failed), Ok(()));
        assert!(rx.recv().is_err(), "an opener that ends well tells nothing");
        assert_eq!(std::fs::read_to_string(&seen).unwrap(), format!("{PAGE}\n/\n"), "the address alone, and not in this test's folder");

        let (failed, rx) = told();
        assert_eq!(start("sh", &["-c", &script, "3"], PAGE, failed), Ok(()));
        let why = rx.recv().unwrap();
        assert!(why.starts_with("`sh` ended with ") && why.ends_with("exit status: 3"), "{why}");
    }
}
