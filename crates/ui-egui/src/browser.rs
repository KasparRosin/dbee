//! Opening a page in the browser.
//!
//! The desktop's own opener is started with the page's address as its one argument, through no
//! shell: `xdg-open` on Linux and FreeBSD, found on `PATH` (inside a Flatpak it asks the desktop's
//! portal), `/usr/bin/open` on macOS, and on Windows `rundll32 url.dll,FileProtocolHandler`. Not
//! through egui's `Context::open_url`: `docs/decisions.md`, 2026-11-26.
//!
//! Only a plain web page's address is passed on ([`plain_page`]). No shell reads it, and an
//! opener has nothing to read into it either: no pattern, no `%`, no `@`, no query. On Linux,
//! FreeBSD and macOS the opener is started in `/`, not in the folder dbee was started in: some
//! openers (KDE's) take a file there of the address's own name for what to open. Windows needs
//! none of it, where no file can have such a name. On those three it is also in a process group
//! of its own, so what ends dbee from its terminal does not end a browser the opener stayed with.
//!
//! A click that opens nothing has to say so. A failure to start the opener is the [`Browser`]'s
//! answer. One that shows only when the opener ends (no browser to hand the page to) is told
//! through its [`LateFailure`], from a thread that waits for the opener, so it is not left behind
//! as a zombie; an opener that ends in failure later than [`TOLD_WITHIN`] is only logged, since
//! some stay for as long as the browser they started. What is not told: an opener that ends well
//! and opened nothing, which is every failure on Windows, where `rundll32` ends well whatever
//! happened.

use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Where a failure that shows only after a [`Browser`] answered is told: at most once, from
/// another thread.
pub type LateFailure = Box<dyn FnOnce(String) + Send>;

/// What opens a page for the app ([`crate::DbeeApp::with_browser`]): the page's address, and
/// where a late failure goes. `Err` says why nothing was started.
pub type Browser = Arc<dyn Fn(&str, LateFailure) -> Result<(), String> + Send + Sync>;

/// The longest address passed on.
const MAX_ADDRESS: usize = 2048;

/// How long after its start an opener's failure is still told, and how long after it was told
/// the app still acts on it. The app answers a failure by putting the page's address on the
/// clipboard, which by later may hold something newer.
pub(crate) const TOLD_WITHIN: Duration = Duration::from_secs(3);

/// The desktop's opener.
pub fn system() -> Browser {
    Arc::new(|url, failed| {
        let (program, before): (&str, &[&str]) = if cfg!(target_os = "macos") {
            // Its place is fixed, so `PATH` is not asked.
            ("/usr/bin/open", &[])
        } else if cfg!(windows) {
            ("rundll32", &["url.dll,FileProtocolHandler"])
        } else {
            ("xdg-open", &[])
        };
        start(program, before, url, failed)
    })
}

/// Whether `url` is a plain web page's address, one an opener may be handed: `https://` and then,
/// to at most [`MAX_ADDRESS`] in all, ASCII letters, digits and `-`, `.`, `_`, `/`. A release's
/// page needs no more, and every other mark means something to one opener or another; a caller
/// that needs one adds it here with the reason it is safe.
pub(crate) fn plain_page(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    !rest.is_empty() && url.len() <= MAX_ADDRESS && rest.chars().all(|c| c.is_ascii_alphanumeric() || "-._/".contains(c))
}

/// Starts `program` with `before` and then `url` as its arguments, and waits for it on a thread
/// of its own.
fn start(program: &str, before: &[&str], url: &str, failed: LateFailure) -> Result<(), String> {
    if !plain_page(url) {
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
    let mut child = command.spawn().map_err(|e| format!("`{program}` did not start: {}", words(&e)))?;
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

/// The system's words for `e`, without their number and the full stop Windows ends them with:
/// `No such file or directory (os error 2)`, `Access is denied. (os error 5)`.
fn words(e: &std::io::Error) -> String {
    e.to_string().split(" (os error").next().unwrap_or_default().trim_end_matches('.').to_owned()
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    const PAGE: &str = "https://github.com/KasparRosin/dbee/releases/tag/v0.3.2";

    #[test]
    fn only_a_plain_web_pages_address_is_one() {
        for url in [PAGE, "https://github.com/Some-One/a.b_c/releases/tag/v0.4.0-rc.1", "https://example.org"] {
            assert!(plain_page(url), "{url}");
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
            assert!(!plain_page(url), "{url:?}");
        }
        assert!(plain_page(&long[..MAX_ADDRESS]));
    }

    /// A callback that hands what it is told to the receiver.
    fn told() -> (LateFailure, mpsc::Receiver<String>) {
        let (tx, rx) = mpsc::channel();
        (
            Box::new(move |why| {
                let _ = tx.send(why);
            }),
            rx,
        )
    }

    #[test]
    fn an_address_that_is_no_plain_page_starts_nothing() {
        let (failed, rx) = told();
        // `true` would start anywhere: the address is refused before it.
        assert_eq!(start("true", &[], "http://example.org/", failed), Err("it is not a web page's address".into()));
        assert!(rx.recv().is_err(), "nothing is told later: the callback was dropped");
    }

    #[test]
    fn an_opener_that_is_not_there_says_so_at_once() {
        let (failed, rx) = told();
        assert_eq!(words(&std::io::Error::other("Access is denied. (os error 5)")), "Access is denied");
        let why = start("dbee-no-such-opener", &[], PAGE, failed).unwrap_err();
        assert!(
            why.starts_with("`dbee-no-such-opener` did not start: ") && !why.contains("os error"),
            "the system's words, without their number: {why}"
        );
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
