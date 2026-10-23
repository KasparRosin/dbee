//! The file pickers of the connection form ("Change…" for the SSH key, "Choose…" for the CA
//! file): rfd runs on a thread of its own and sends the chosen path back over a channel, so the
//! UI thread never waits for it. A picker that cannot open leaves a note in its row.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use crate::conn_form::Auth;
use crate::connections::ConnScreen;

/// A picker that answers "nothing" faster than this did not show (no portal, no zenity).
const PICKER_GAVE_UP: Duration = Duration::from_millis(800);

/// Which field a file picker fills.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pick {
    Key,
    Ca,
}

/// A file picker running on its own thread.
#[derive(Debug)]
pub(crate) struct Picker {
    pub target: Pick,
    rx: Receiver<Option<PathBuf>>,
    started: Instant,
}

/// Notes the file pickers leave in their rows.
#[derive(Debug, Default)]
pub(crate) struct Notes {
    pub key: Option<String>,
    pub ca: Option<String>,
}

/// Takes the running picker's answer, if it has one, into the form or the row's note.
pub(crate) fn poll(conn: &mut ConnScreen, ctx: &egui::Context) {
    let Some(p) = &conn.picker else { return };
    let target = p.target;
    let answer = p.rx.try_recv();
    let quick = p.started.elapsed() < PICKER_GAVE_UP;
    let note = match answer {
        Err(TryRecvError::Empty) => {
            ctx.request_repaint_after(Duration::from_millis(250));
            return;
        }
        Ok(Some(path)) => {
            match target {
                Pick::Key => {
                    conn.form.ssh_key = Some(path);
                    conn.form.ssh_auth = Auth::Key;
                }
                Pick::Ca => conn.form.ca_file = Some(path),
            }
            None
        }
        // Linux: rfd answers "nothing" at once when neither the desktop portal nor zenity could
        // show a picker; a person takes longer than that to cancel one.
        Ok(None) if quick && cfg!(not(any(target_os = "macos", target_os = "windows"))) => {
            Some("No file picker opened: it needs the desktop portal (xdg-desktop-portal) or zenity.".to_string())
        }
        Ok(None) => None,
        Err(TryRecvError::Disconnected) => Some("The file picker stopped without an answer.".to_string()),
    };
    conn.picker = None;
    match target {
        Pick::Key => conn.notes.key = note,
        Pick::Ca => conn.notes.ca = note,
    }
}

/// Starts a file picker for `target` on its own thread; it never runs on the UI thread.
pub(crate) fn start(conn: &mut ConnScreen, target: Pick, ctx: &egui::Context) {
    if conn.picker.is_some() {
        return;
    }
    let ssh_dir = dbee_config::home_dir().map(|h| h.join(".ssh"));
    let (title, dir) = match target {
        // ~/.ssh, or the folder of the key the login uses when there is no ~/.ssh.
        Pick::Key => {
            ("Choose an SSH key", ssh_dir.filter(|d| d.is_dir()).or_else(|| conn.key.path.as_deref().and_then(Path::parent).map(Path::to_path_buf)))
        }
        Pick::Ca => {
            ("Choose a CA certificate", conn.form.ca_file.as_deref().and_then(Path::parent).map(Path::to_path_buf).or_else(dbee_config::home_dir))
        }
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let repaint = ctx.clone();
    let spawned = std::thread::Builder::new().name("dbee-file-picker".into()).spawn(move || {
        let mut dialog = rfd::FileDialog::new().set_title(title);
        if let Some(dir) = dir.filter(|d| d.is_dir()) {
            dialog = dialog.set_directory(dir);
        }
        let _ = tx.send(dialog.pick_file());
        repaint.request_repaint();
    });
    let note = match spawned {
        Ok(_) => {
            conn.picker = Some(Picker { target, rx, started: Instant::now() });
            None
        }
        Err(e) => Some(format!("The file picker could not be started: {e}")),
    };
    match target {
        Pick::Key => conn.notes.key = note,
        Pick::Ca => conn.notes.ca = note,
    }
}
