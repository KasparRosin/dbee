//! The file pickers of the connection form ("Change…" for the SSH key, "Choose…" for the CA
//! file, "Choose…" and "New file…" for a database file): rfd runs on a thread of its own and sends
//! the chosen path back over a channel, so the UI thread never waits for it. A database file's
//! pickers list the type's extensions first, then all files; New file… is a save dialog, and the
//! file it names is made by the next test or connect.
//!
//! One picker at a time: while its thread runs, the form starts no other. "Stop" only stops
//! waiting in the form; the picker's answer still lands when it comes, unless the form shows
//! another connection by then. Notes about a picker (one is already open, none could open, it
//! stopped without an answer) go to the form's message slot, so the rows keep showing what they
//! show.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use dbee_config::Settings;

use crate::conn_form::Auth;
use crate::connections::{ConnScreen, Kind, Message};

/// A picker that answers "nothing" faster than this most likely never showed (no portal, no
/// zenity): a person takes longer to cancel one.
const PICKER_GAVE_UP: Duration = Duration::from_millis(800);

/// What the form says when a picker gave nothing back that fast (Linux and the BSDs).
pub(crate) const NO_PICKER: &str = "No file chosen. If no file picker appeared, dbee needs xdg-desktop-portal or zenity to show one.";

/// Which field a file picker fills.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pick {
    Key,
    Ca,
    /// A database file to open.
    File,
    /// A database file to make.
    NewFile,
}

/// The filter that lists every file.
const ALL_FILES: &[&str] = &["*"];

/// What a picker shows.
struct Ask {
    title: &'static str,
    /// The folder it starts in.
    dir: Option<PathBuf>,
    /// Named extension lists, the first shown first.
    filters: Vec<(String, &'static [&'static str])>,
    /// A save dialog (a file to make), else an open one.
    save: bool,
}

/// A file picker running on its own thread.
#[derive(Debug)]
pub(crate) struct Picker {
    pub target: Pick,
    /// The form's generation when it opened: its answer is for that connection only.
    pub generation: u64,
    /// The user stopped waiting; the answer still lands if it comes.
    pub stopped: bool,
    rx: Receiver<Option<PathBuf>>,
    started: Instant,
}

/// A picker's answer, as far as there is one.
enum Answer {
    Waiting,
    Picked(PathBuf),
    /// Nothing chosen; `quick` when it came back too fast for a person to have cancelled.
    Nothing {
        quick: bool,
    },
    /// The thread ended without answering.
    Failed,
}

impl Picker {
    /// Opens the picker `ask` describes on its own thread.
    fn spawn(target: Pick, generation: u64, ask: Ask, ctx: &egui::Context) -> Result<Picker, String> {
        let (tx, rx) = std::sync::mpsc::channel();
        let repaint = ctx.clone();
        std::thread::Builder::new()
            .name("dbee-file-picker".into())
            .spawn(move || {
                let mut dialog = rfd::FileDialog::new().set_title(ask.title);
                if let Some(dir) = ask.dir.filter(|d| d.is_dir()) {
                    dialog = dialog.set_directory(dir);
                }
                for (name, extensions) in &ask.filters {
                    dialog = dialog.add_filter(name, extensions);
                }
                let _ = tx.send(if ask.save { dialog.save_file() } else { dialog.pick_file() });
                repaint.request_repaint();
            })
            .map_err(|e| e.to_string())?;
        Ok(Picker { target, generation, stopped: false, rx, started: Instant::now() })
    }

    /// A picker whose answer the test sends.
    #[cfg(test)]
    pub(crate) fn for_test(target: Pick, generation: u64) -> (std::sync::mpsc::Sender<Option<PathBuf>>, Picker) {
        let (tx, rx) = std::sync::mpsc::channel();
        (tx, Picker { target, generation, stopped: false, rx, started: Instant::now() })
    }

    fn answer(&self) -> Answer {
        match self.rx.try_recv() {
            Err(TryRecvError::Empty) => Answer::Waiting,
            Ok(Some(path)) => Answer::Picked(path),
            Ok(None) => Answer::Nothing { quick: self.started.elapsed() < PICKER_GAVE_UP },
            Err(TryRecvError::Disconnected) => Answer::Failed,
        }
    }
}

fn note(text: impl Into<String>) -> Option<Message> {
    Some(Message { kind: Kind::Note, text: text.into() })
}

/// Takes the running picker's answer, if it has one, into the form (step 5 of the frame, before
/// drawing). While it waits, the form polls it a few times a second.
pub(crate) fn poll(conn: &mut ConnScreen, ctx: &egui::Context) {
    let Some(p) = &conn.picker else { return };
    let (target, generation, stopped) = (p.target, p.generation, p.stopped);
    let answer = p.answer();
    if let Answer::Waiting = answer {
        ctx.request_repaint_after(Duration::from_millis(250));
        return;
    }
    conn.picker = None;
    // An answer for a connection the form no longer shows is dropped.
    if generation != conn.generation {
        return;
    }
    match answer {
        Answer::Waiting => {}
        Answer::Picked(path) => match target {
            Pick::Key => {
                conn.form.ssh_key = Some(path);
                conn.form.ssh_auth = Auth::Key;
            }
            Pick::Ca => conn.form.ca_file = Some(path),
            Pick::File | Pick::NewFile => {
                conn.form.file = Some(path);
                conn.form.create_file = target == Pick::NewFile;
            }
        },
        // rfd says "nothing" for a cancel too, so this claims no failure.
        Answer::Nothing { quick } if quick && !stopped && cfg!(not(any(target_os = "macos", target_os = "windows"))) => {
            conn.message = note(NO_PICKER)
        }
        Answer::Nothing { .. } => {}
        Answer::Failed if !stopped => conn.message = Some(Message { kind: Kind::Error, text: "The file picker stopped without an answer.".into() }),
        Answer::Failed => {}
    }
}

/// Opens a file picker for `target`, unless one is open already (then the form says so).
pub(crate) fn start(conn: &mut ConnScreen, target: Pick, ctx: &egui::Context) {
    if conn.picker.is_some() {
        conn.message = note("A file picker is already open: choose a file there, or close it first.");
        return;
    }
    let ssh_dir = dbee_config::home_dir().map(|h| h.join(".ssh"));
    let folder_of = |path: Option<&Path>| path.and_then(Path::parent).map(Path::to_path_buf).or_else(dbee_config::home_dir);
    let open = |title, dir| Ask { title, dir, filters: Vec::new(), save: false };
    let ask = match target {
        // ~/.ssh, or the folder of the key the login uses when there is no ~/.ssh.
        Pick::Key => open(
            "Choose an SSH key",
            ssh_dir.filter(|d| d.is_dir()).or_else(|| conn.key.path.as_deref().and_then(Path::parent).map(Path::to_path_buf)),
        ),
        Pick::Ca => open("Choose a CA certificate", folder_of(conn.form.ca_file.as_deref())),
        Pick::File | Pick::NewFile => {
            let kind = conn.form.kind;
            let mut filters = Vec::new();
            if let Settings::File { extensions } = kind.info().settings {
                filters.push((format!("{} files", kind.label()), extensions));
            }
            filters.push(("All files".to_string(), ALL_FILES));
            let save = target == Pick::NewFile;
            let title = if save { "New database file" } else { "Choose a database file" };
            Ask { title, dir: folder_of(conn.form.file.as_deref()), filters, save }
        }
    };
    match Picker::spawn(target, conn.generation, ask, ctx) {
        Ok(p) => conn.picker = Some(p),
        Err(e) => conn.message = Some(Message { kind: Kind::Error, text: format!("The file picker could not be started: {e}") }),
    }
}

/// Stops waiting for the picker in the form; its answer still lands when it comes.
pub(crate) fn stop(conn: &mut ConnScreen) {
    if let Some(p) = conn.picker.as_mut() {
        p.stopped = true;
    }
}
