//! The update check's state in the session, and the session's side of it: when the automatic
//! check runs, what a check's answer does, and the install step's progress.
//!
//! The parts below the engine are `dbee-update`'s. Everything the check reads from the machine or
//! the network comes in through [`Updates`] ([`crate::SessionOptions::updates`]): with none, a
//! session never checks, which is how tests, isolated runs and development builds run.
//!
//! The state goes `none → available → updating → ready`, or `→ failed` and from there back to
//! `updating` on a retry. No install step exists yet ([`Updates::installer`] is `None` in the
//! app), so every offer's action opens the release's page and the state stops at `available`.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dbee_update::cache::{CACHE_FILE, MAX_CACHE};
use dbee_update::{Action, Cache, Fetch, Install, InstallKind, Manifest, Offer, Platform, Progress, RECHECK, Repo, Version, first_check_in};
use serde_json::json;

use crate::{Apply, EngineError, JobId, Json, Session};

/// The command that checks, and the label of a check's job.
pub(crate) const CHECK: &str = "app.checkForUpdates";
/// The command the update button runs, and the label of an install's job.
pub(crate) const UPDATE: &str = "app.update";

/// The progress cell's value while an install has no percent to show.
const NO_PERCENT: u16 = u16::MAX;

/// What a session needs to check for updates.
#[derive(Clone)]
pub struct Updates {
    /// Reads the newest release's manifest (the app's is `dbee_update::HttpFetch`).
    pub fetch: Arc<dyn Fetch>,
    /// The install step; `None` while dbee has none, and then every offer opens the release's page.
    pub installer: Option<Arc<dyn Install>>,
    /// This copy's version.
    pub current: Version,
    /// How this copy was installed.
    pub kind: InstallKind,
    pub platform: Platform,
    /// The repository whose releases are checked; release pages are built from it.
    pub repo: Repo,
    /// Whether the session checks by itself, while the settings leave the check on. Off, only
    /// `app.checkForUpdates` checks.
    pub automatic: bool,
    /// How long after the start the first automatic check runs, when no fresh answer is kept.
    pub first_check: Duration,
    /// Whether the last answer is kept in the settings folder, so a restart does not ask again
    /// while it is fresh.
    pub remember: bool,
}

impl std::fmt::Debug for Updates {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Updates")
            .field("installer", &self.installer.is_some())
            .field("current", &self.current)
            .field("kind", &self.kind)
            .field("platform", &self.platform)
            .field("repo", &self.repo)
            .field("automatic", &self.automatic)
            .field("remember", &self.remember)
            .finish_non_exhaustive()
    }
}

/// Where the update stands, in the session state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum UpdateState {
    /// No newer release is known.
    #[default]
    None,
    /// A newer release exists.
    Available(Offer),
    /// The install runs; its percent is [`Session::update_percent`].
    Updating(Offer),
    /// The release is installed, and the next start runs it.
    Ready(Offer),
    /// The install failed; a retry runs it again.
    Failed { offer: Offer, reason: String },
}

impl UpdateState {
    /// The offer the state is about.
    pub fn offer(&self) -> Option<&Offer> {
        match self {
            UpdateState::None => None,
            UpdateState::Available(offer) | UpdateState::Updating(offer) | UpdateState::Ready(offer) | UpdateState::Failed { offer, .. } => {
                Some(offer)
            }
        }
    }

    fn id(&self) -> &'static str {
        match self {
            UpdateState::None => "none",
            UpdateState::Available(_) => "available",
            UpdateState::Updating(_) => "updating",
            UpdateState::Ready(_) => "ready",
            UpdateState::Failed { .. } => "failed",
        }
    }
}

/// An action as JSON: `{type: "openPage", url}` or `{type: "install", asset}`.
pub(crate) fn action_json(action: &Action) -> Json {
    match action {
        Action::OpenPage { url } => json!({ "type": "openPage", "url": url }),
        Action::Install { asset } => json!({ "type": "install", "asset": asset }),
    }
}

/// The session's update checker: the options, the settings' switch and the clock of the
/// automatic check.
pub(crate) struct Updater {
    pub(crate) options: Updates,
    config_dir: Option<PathBuf>,
    /// When the next automatic check runs.
    due: Instant,
    /// The running install's percent, or [`NO_PERCENT`].
    progress: Arc<AtomicU16>,
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// `now + wait`, or `now` when that cannot be told.
fn after(wait: Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(wait).unwrap_or(now)
}

impl Updater {
    /// The checker and the state a session starts with: the kept answer's offer while it is
    /// fresh, and the first check after it no longer is.
    pub(crate) fn start(options: Updates, config_dir: Option<PathBuf>) -> (Updater, UpdateState) {
        let now = unix_now();
        let kept = (options.remember)
            .then(|| {
                config_dir.as_deref().and_then(|dir| dbee_config::read_small_file(dir, CACHE_FILE, u64::try_from(MAX_CACHE).unwrap_or(u64::MAX)))
            })
            .flatten()
            .and_then(|bytes| Cache::parse(&bytes))
            .filter(|cache| cache.is_fresh(now));
        let state = kept.as_ref().and_then(|cache| offer(&options, &cache.latest)).map_or(UpdateState::None, UpdateState::Available);
        let wait = if kept.is_some() { first_check_in(kept.as_ref(), now) } else { options.first_check };
        (Updater { options, config_dir, due: after(wait), progress: Arc::new(AtomicU16::new(NO_PERCENT)) }, state)
    }
}

/// The offer `options`' copy gets when the newest release is `latest`.
fn offer(options: &Updates, latest: &Version) -> Option<Offer> {
    let installer = options.installer.as_ref().is_some_and(|i| i.supports(options.kind));
    dbee_update::offer(&options.current, latest, options.kind, options.platform, &options.repo, installer)
}

fn update_error(e: impl std::fmt::Display) -> EngineError {
    EngineError::Update(e.to_string())
}

impl Session {
    /// The install's percent while one runs and has one.
    pub fn update_percent(&self) -> Option<u8> {
        let updater = self.updates.as_ref()?;
        matches!(self.state.update, UpdateState::Updating(_)).then(|| updater.progress.load(Ordering::Relaxed)).and_then(|p| u8::try_from(p).ok())
    }

    /// Whether the automatic check is on: this run checks at all, by itself, and the settings
    /// leave it on.
    pub fn update_check_automatic(&self) -> bool {
        self.update_check_enabled && self.updates.as_ref().is_some_and(|u| u.options.automatic)
    }

    /// How long until the next automatic check, for a window that sleeps meanwhile; `None` while
    /// none is coming.
    pub fn next_update_check(&self) -> Option<Duration> {
        let updater = self.updates.as_ref().filter(|_| self.update_check_automatic())?;
        Some(updater.due.saturating_duration_since(Instant::now()))
    }

    /// The update as `document.inspect` shows it.
    pub(crate) fn update_json(&self) -> Json {
        let st = &self.state.update;
        let checks = match &self.updates {
            None => "off",
            Some(_) if self.update_check_automatic() => "automatic",
            Some(_) => "manual",
        };
        let mut v = json!({ "state": st.id(), "checks": checks, "enabled": self.update_check_enabled });
        if let Some(o) = st.offer() {
            v["from"] = json!(o.from.to_string());
            v["to"] = json!(o.to.to_string());
            v["kind"] = json!(o.kind.id());
            v["action"] = action_json(&o.action);
        }
        if let UpdateState::Updating(_) = st {
            v["percent"] = json!(self.update_percent());
        }
        if let UpdateState::Failed { reason, .. } = st {
            v["reason"] = json!(reason);
        }
        v
    }

    /// Starts the automatic check once it is due, from [`Session::pump`]. Its failure is silent:
    /// a line in the debug log, and the state stays as it was.
    pub(crate) fn update_tick(&mut self) {
        if !self.update_check_automatic() || self.jobs.is_running_label(CHECK) {
            return;
        }
        let Some(updater) = self.updates.as_mut() else { return };
        if Instant::now() < updater.due {
            return;
        }
        updater.due = after(RECHECK);
        self.start_update_check(true);
    }

    /// Schedules the first automatic check anew, after the settings turned the check on.
    pub(crate) fn update_check_soon(&mut self) {
        if let Some(updater) = self.updates.as_mut() {
            updater.due = after(updater.options.first_check);
        }
    }

    /// Starts a check as a job: the fetch and the parse on a blocking thread, the answer kept in
    /// the settings folder, then applied to the state. `None` when this run does not check.
    pub(crate) fn start_update_check(&mut self, automatic: bool) -> Option<JobId> {
        let updater = self.updates.as_ref()?;
        let fetch = Arc::clone(&updater.options.fetch);
        let keep_in = updater.options.remember.then(|| updater.config_dir.clone()).flatten();
        let fut = async move {
            let read = tokio::task::spawn_blocking(move || {
                let manifest = Manifest::parse(&fetch.manifest()?)?;
                if let Some(dir) = keep_in {
                    let cache = Cache { checked_at: unix_now(), latest: manifest.version.clone() };
                    if dbee_config::write_small_file(&dir, CACHE_FILE, &cache.to_bytes()).is_err() {
                        log::debug!("update check: the answer could not be kept");
                    }
                }
                Ok::<Version, dbee_update::UpdateError>(manifest.version)
            })
            .await;
            let latest = match read {
                Ok(read) => read.map_err(update_error),
                Err(_) => Err(EngineError::Update("the check stopped before it had an answer".into())),
            };
            let apply: Apply = Box::new(move |s: &mut Session| s.apply_update_check(latest, automatic));
            apply
        };
        // No database work: a stop asked of every job (a disconnect) is none of its business, and
        // it must not count as a statement to cancel.
        Some(self.spawn_stoppable_job(CHECK, Arc::default(), fut))
    }

    /// What a check's answer does: a newer release becomes the offer, anything else takes an
    /// offer away; an install that runs, is done or failed keeps the state. An automatic check's
    /// failure is no error.
    fn apply_update_check(&mut self, latest: Result<Version, EngineError>, automatic: bool) -> Result<Json, EngineError> {
        let latest = match latest {
            Ok(latest) => latest,
            Err(e) if automatic => {
                log::debug!("update check: no answer ({})", e.code());
                return Ok(json!({ "checked": false }));
            }
            Err(e) => return Err(e),
        };
        let Some(updater) = self.updates.as_ref() else { return Ok(crate::cmd::ignored()) };
        let found = offer(&updater.options, &latest);
        let current = updater.options.current.to_string();
        if matches!(self.state.update, UpdateState::None | UpdateState::Available(_)) {
            self.state.update = found.clone().map_or(UpdateState::None, UpdateState::Available);
        }
        Ok(json!({ "checked": true, "current": current, "latest": latest.to_string(), "available": found.is_some(), "update": self.update_json() }))
    }

    /// Starts the install of `offer` as a job, the state `updating` meanwhile; its end makes it
    /// `ready` or `failed`. `None` when this run has no install step.
    pub(crate) fn start_update_install(&mut self, offer: Offer) -> Option<JobId> {
        let updater = self.updates.as_ref()?;
        let installer = Arc::clone(updater.options.installer.as_ref()?);
        let progress = Arc::clone(&updater.progress);
        progress.store(NO_PERCENT, Ordering::Relaxed);
        self.state.update = UpdateState::Updating(offer.clone());
        let fut = async move {
            let installing = offer.clone();
            let ran = tokio::task::spawn_blocking(move || {
                installer.install(&installing, &|p| {
                    let value = match p {
                        Progress::Percent(n) => u16::from(n.min(100)),
                        Progress::Working => NO_PERCENT,
                    };
                    progress.store(value, Ordering::Relaxed);
                })
            })
            .await;
            let result = match ran {
                Ok(result) => result.map_err(|e| e.to_string()),
                Err(_) => Err("the install stopped before it was done".to_string()),
            };
            let apply: Apply = Box::new(move |s: &mut Session| s.apply_update_install(offer, result));
            apply
        };
        Some(self.spawn_stoppable_job(UPDATE, Arc::default(), fut))
    }

    fn apply_update_install(&mut self, offer: Offer, result: Result<(), String>) -> Result<Json, EngineError> {
        match result {
            Ok(()) => {
                self.state.update = UpdateState::Ready(offer);
                Ok(json!({ "action": "install", "update": self.update_json() }))
            }
            Err(reason) => {
                // An `Err` puts nothing back (only a panic does), so the state says `failed` and
                // the job's caller reads why.
                self.state.update = UpdateState::Failed { offer, reason: reason.clone() };
                Err(EngineError::Update(reason))
            }
        }
    }
}
