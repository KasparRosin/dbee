//! dbee engine (layer L5): the [`Session`], the command registry ([`cmd`]), background jobs, the
//! panic guard and the journal.
//!
//! Everything is a command (AGENTS.md): the UI, the localhost control channel and tests act only
//! through [`Session::execute`]. A command runs on the caller's thread and either finishes
//! ([`Outcome::Done`]) or starts a job on the session's tokio runtime ([`Outcome::Started`]).
//! Database work runs in jobs; the UI thread applies their results with [`Session::pump`] each
//! frame, so the window never waits on the database.
//!
//! Logs name command ids, job labels, error codes and timings only: never params, secrets,
//! cell values or SQL text.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod allowed;
pub mod cmd;
pub mod edits;
pub mod fuzzy;
mod jobs;
pub mod state;
mod turns;
pub mod view;

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dbee_config::{ConnectionStore, DbKind, SecretStore};
use dbee_driver::Drivers;
use dbee_ssh::SshEnv;
use serde::Serialize;
use serde_json::{Map, json};

pub use cmd::selection::COPY_MAX_BYTES;
pub use cmd::{CommandInfo, CommandSpec};
pub use dbee_driver::{Meaning, TlsState, TransactionState, allowed_file};
pub use edits::{Edits, Failed};
pub(crate) use jobs::Apply;
pub use state::{
    Connected, CurrentDatabase, PageView, QueryError, QueryTab, Screen, Selection, State, StatementOutcome, Tab, TableTab, Total, Transaction, Which,
};
pub use view::{VALUE_VIEW_MAX_BYTES, ValueView};

/// Params, results and error data.
pub type Json = serde_json::Value;

/// A background job, unique within a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct JobId(pub u64);

/// A tab, unique within a session (ids are never reused).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct TabId(pub u64);

/// An open connection, unique within a session: each open gets a fresh one (ids are never
/// reused), and a reconnect keeps it. It is not the saved connection's id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct ConnectionId(pub u64);

/// What a command did.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// It finished, with this result.
    Done(Json),
    /// It started a job; the result comes from [`Session::pump`] or [`Session::wait`].
    Started(JobId),
}

/// What the caller must supply before the command can go on. The UI asks the user and runs the
/// command again with the answer in its params.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Need {
    /// The SSH server's host key is not in `known_hosts`: trust this one?
    HostKey { host: String, key_type: String, fingerprint: String },
    /// The passphrase of this SSH key file.
    Passphrase { key: String },
    /// The database password.
    Password,
    /// The SSH password.
    SshPassword,
    /// Something will be lost (`what`, e.g. `queryText`); run again with `force` to go ahead.
    Confirm { what: String },
}

/// Why a command or a job failed. [`EngineError::code`] is the stable id the UI and the control
/// channel use; [`EngineError::data`] carries the details as JSON.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineError {
    /// No command has this id.
    UnknownCommand(String),
    /// The command exists but cannot run now (no tab open, not connected, …).
    Disabled { id: String, why: String },
    /// The params are missing a key, have a wrong type, or name something that does not exist.
    BadParams { id: String, msg: String },
    /// The command needs an answer first.
    Needs { need: Need, message: String },
    /// The database or the tunnel failed; `code` is a `dbee_driver::DbError::code`.
    Db { code: &'static str, message: String, data: Json },
    /// Something is already running that this command would collide with.
    Busy(String),
    /// Gave up waiting: [`Session::wait`] or [`Session::run_blocking`] ran out of time (the job
    /// goes on), or a bounded step of a job took too long (a catalog load past 60 s, which was
    /// cancelled and its connection discarded).
    Timeout,
    /// A bug: the command, the job or its `Apply` panicked. The state and the UI requests were
    /// put back as they were before the call, and jobs the call started are orphaned: they run to
    /// their end, and their results are dropped (each ends as `Internal` too).
    Internal { id: String, msg: String },
    /// Settings or the connections file could not be used.
    Config(String),
}

impl EngineError {
    /// A stable id: `unknownCommand`, `disabled`, `badParams`, `needsHostKey`, `needsPassphrase`,
    /// `needsPassword`, `needsSshPassword`, `needsConfirm`, a database code (`server`,
    /// `connectionLost`, …), `busy`, `timeout`, `internal` or `config`.
    pub fn code(&self) -> &'static str {
        match self {
            EngineError::UnknownCommand(_) => "unknownCommand",
            EngineError::Disabled { .. } => "disabled",
            EngineError::BadParams { .. } => "badParams",
            EngineError::Needs { need, .. } => match need {
                Need::HostKey { .. } => "needsHostKey",
                Need::Passphrase { .. } => "needsPassphrase",
                Need::Password => "needsPassword",
                Need::SshPassword => "needsSshPassword",
                Need::Confirm { .. } => "needsConfirm",
            },
            EngineError::Db { code, .. } => code,
            EngineError::Busy(_) => "busy",
            EngineError::Timeout => "timeout",
            EngineError::Internal { .. } => "internal",
            EngineError::Config(_) => "config",
        }
    }

    /// The details as a JSON object (camelCase keys), e.g. `{fingerprint}` for a host key.
    pub fn data(&self) -> Json {
        match self {
            EngineError::UnknownCommand(id) | EngineError::BadParams { id, .. } | EngineError::Internal { id, .. } => {
                json!({ "command": id })
            }
            EngineError::Disabled { id, why } => json!({ "command": id, "why": why }),
            EngineError::Needs { need, .. } => match need {
                Need::HostKey { host, key_type, fingerprint } => json!({ "host": host, "keyType": key_type, "fingerprint": fingerprint }),
                Need::Passphrase { key } => json!({ "key": key }),
                Need::Confirm { what } => json!({ "what": what }),
                Need::Password | Need::SshPassword => json!({}),
            },
            EngineError::Db { data, .. } => data.clone(),
            EngineError::Busy(_) | EngineError::Timeout | EngineError::Config(_) => json!({}),
        }
    }
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::UnknownCommand(id) => write!(f, "there is no command `{id}`"),
            EngineError::Disabled { id, why } => write!(f, "`{id}` cannot run now: {why}"),
            EngineError::BadParams { id, msg } => write!(f, "bad params for `{id}`: {msg}"),
            EngineError::Needs { message, .. } | EngineError::Db { message, .. } => write!(f, "{message}"),
            EngineError::Busy(what) => write!(f, "busy: {what}"),
            EngineError::Timeout => write!(f, "gave up waiting: it took too long"),
            EngineError::Internal { id, msg } => write!(f, "internal error in `{id}` (the session was kept as it was): {msg}"),
            EngineError::Config(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for EngineError {}

/// Something only the UI can do, collected by [`Session::take_ui_requests`]. Programmatic calls
/// never open dialogs: only menu-style invocation asks for one.
#[derive(Clone, Debug, PartialEq)]
pub enum UiRequest {
    Dialog { name: &'static str, data: Json },
    Notice(String),
}

/// What a session reads from the machine; everything comes in here, so tests and isolated runs
/// never touch the real home, SSH agent or keyring.
#[derive(Clone, Default)]
pub struct SessionOptions {
    /// Where `connections.json` lives; `None` keeps connections in memory only.
    pub config_dir: Option<PathBuf>,
    /// Where passwords and passphrases are kept; `None` keeps none.
    pub secrets: Option<Arc<dyn SecretStore>>,
    pub ssh: SshEnv,
    /// Called from a worker thread after each job result, so the UI can wake up and pump.
    pub notify: Option<Arc<dyn Fn() + Send + Sync>>,
    /// The only `(host, port)` targets `connection.test`, `connection.listDatabases`,
    /// `connection.open` and `connection.reconnect` may reach (an isolated run's test servers);
    /// `None` is no limit.
    /// See [`allowed`].
    pub allowed_targets: Option<Vec<(String, u16)>>,
    /// The only folders a file database may lie in (an isolated run's home and `target/`); `None`
    /// is no limit. They go to the driver (`dbee_driver::Target::allowed_dirs`), which checks the
    /// file inside its open and keeps the connection's SQL from other files. See [`allowed`].
    pub allowed_dirs: Option<Vec<PathBuf>>,
    /// The drivers connections open with (the app's are `dbee_drivers::all()`). A database type
    /// none of them opens is refused with `disabled`.
    pub drivers: Drivers,
}

impl std::fmt::Debug for SessionOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionOptions")
            .field("config_dir", &self.config_dir)
            .field("secrets", &self.secrets.as_ref().map(|s| s.name()))
            .field("ssh", &self.ssh)
            .field("notify", &self.notify.is_some())
            .field("allowed_targets", &self.allowed_targets)
            .field("allowed_dirs", &self.allowed_dirs)
            .field("drivers", &self.drivers)
            .finish()
    }
}

/// Most journal entries kept; past it the oldest [`JOURNAL_DROP`] go.
const JOURNAL_MAX: usize = 10_000;
const JOURNAL_DROP: usize = 5_000;

/// Commands whose consecutive journal entries replace each other: each sets the whole value, so
/// only the last of a run matters for a replay (an editor sends one per keystroke).
const JOURNAL_COALESCED: &[&str] = &["query.setText"];

/// The filter bar's command, whose typing (`apply: false`) is coalesced too.
const SET_FILTERS: &str = "table.setFilters";

/// Whether the journal entry for `id` with `params` replaces `last` instead of following it: a
/// [`JOURNAL_COALESCED`] command after itself, or `table.setFilters` with `apply: false` after the
/// same with the same keys (typing in the filter bar: the newer sets everything the older did,
/// and neither loads a page).
pub(crate) fn journal_coalesces(last: &(String, Json), id: &str, params: &Json) -> bool {
    if last.0 != id {
        return false;
    }
    if JOURNAL_COALESCED.contains(&id) {
        return true;
    }
    let draft = |p: &Json| p.get("apply") == Some(&Json::Bool(false));
    let keys = |p: &Json| p.as_object().map(|m| m.keys().cloned().collect::<Vec<_>>());
    id == SET_FILTERS && draft(&last.1) && draft(params) && keys(&last.1) == keys(params)
}

/// One window's engine: the state, the command registry, the job runtime and the journal.
pub struct Session {
    pub(crate) state: State,
    pub(crate) jobs: jobs::Jobs,
    pub(crate) ui_requests: Vec<UiRequest>,
    pub(crate) secrets: Option<Arc<dyn SecretStore>>,
    pub(crate) ssh: SshEnv,
    pub(crate) allowed_targets: Option<Vec<(String, u16)>>,
    pub(crate) allowed_dirs: Option<Vec<PathBuf>>,
    pub(crate) drivers: Drivers,
    journal: Vec<(String, Json)>,
    value_cache: view::Cache,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session").field("tabs", &self.state.tabs.len()).field("busy", &self.busy()).finish_non_exhaustive()
    }
}

impl Session {
    /// A session on the connection screen, with its job runtime (2 worker threads named
    /// `dbee-db`). A damaged or unreadable connections file is not an error: the store is then
    /// empty and [`State::store_warning`] says why.
    pub fn new(o: SessionOptions) -> Result<Session, EngineError> {
        let jobs = jobs::Jobs::new(o.notify)?;
        let (store, store_warning) = ConnectionStore::load(o.config_dir.as_deref());
        if store_warning.is_some() {
            log::warn!("the connections file could not be used; connections are kept in memory or start empty");
        }
        Ok(Session {
            state: State::new(store, store_warning),
            jobs,
            ui_requests: Vec::new(),
            secrets: o.secrets,
            ssh: o.ssh,
            allowed_targets: o.allowed_targets,
            allowed_dirs: o.allowed_dirs,
            drivers: o.drivers,
            journal: Vec::new(),
            value_cache: view::Cache::default(),
        })
    }

    /// Runs command `id`. In order: find it, check `enabled`, check that `params` is an object
    /// (`null` counts as `{}`), then run it under the panic guard: the state is snapshot first,
    /// and a panic restores it and returns [`EngineError::Internal`]. A successful journaled
    /// command is recorded in the journal without its secret params.
    pub fn execute(&mut self, id: &str, params: Json) -> Result<Outcome, EngineError> {
        let started = Instant::now();
        let result = self.execute_guarded(id, params);
        // An unknown id is caller text and could be anything, so it is not logged.
        let logged = cmd::find(id).map_or("(unknown)", |c| c.id);
        match &result {
            Ok(Outcome::Done(_)) => log::debug!("command {logged}: done in {:?}", started.elapsed()),
            Ok(Outcome::Started(job)) => log::debug!("command {logged}: started job {} in {:?}", job.0, started.elapsed()),
            Err(e) => log::debug!("command {logged}: {} in {:?}", e.code(), started.elapsed()),
        }
        result
    }

    fn execute_guarded(&mut self, id: &str, params: Json) -> Result<Outcome, EngineError> {
        let spec = cmd::find(id).ok_or_else(|| EngineError::UnknownCommand(id.to_string()))?;
        match catch_unwind(AssertUnwindSafe(|| (spec.enabled)(self))) {
            Ok(Ok(())) => {}
            Ok(Err(why)) => return Err(EngineError::Disabled { id: id.to_string(), why }),
            Err(payload) => {
                log::error!("command {id}: its enabled check panicked");
                return Err(EngineError::Internal { id: id.to_string(), msg: panic_text(payload.as_ref()) });
            }
        }
        let params = match params {
            Json::Null => Json::Object(Map::new()),
            p @ Json::Object(_) => p,
            _ => return Err(EngineError::BadParams { id: id.to_string(), msg: "params must be a JSON object".into() }),
        };
        let mut result = self.guarded(id, |s| (spec.run)(s, &params));
        if let Err(EngineError::BadParams { id: named, .. } | EngineError::Disabled { id: named, .. }) = &mut result
            && named.is_empty()
        {
            // The params helpers do not know the command.
            *named = id.to_string();
        }
        if result.is_ok() && spec.journal {
            self.record(id, &params);
        }
        result
    }

    /// Runs `f` under the panic guard: on a panic the state and the UI requests are put back as
    /// they were, the jobs `f` started are orphaned (their results will be dropped, since they
    /// were meant for the state that was thrown away), and the result is `Internal` naming
    /// `what`.
    pub(crate) fn guarded<T>(&mut self, what: &str, f: impl FnOnce(&mut Session) -> Result<T, EngineError>) -> Result<T, EngineError> {
        let snapshot = self.state.clone();
        let requests = self.ui_requests.len();
        let jobs_before = self.jobs.last_id();
        match catch_unwind(AssertUnwindSafe(|| f(self))) {
            Ok(r) => r,
            Err(payload) => {
                self.state = snapshot;
                self.ui_requests.truncate(requests);
                let orphans = self.jobs.orphan_since(jobs_before);
                log::error!("{what} panicked; the session was restored and {orphans} job(s) it started were orphaned");
                Err(EngineError::Internal { id: what.to_string(), msg: panic_text(payload.as_ref()) })
            }
        }
    }

    fn record(&mut self, id: &str, params: &Json) {
        let entry = (id.to_string(), cmd::journal_params(params));
        match self.journal.last_mut() {
            Some(last) if journal_coalesces(last, id, &entry.1) => *last = entry,
            _ => self.journal.push(entry),
        }
        if self.journal.len() > JOURNAL_MAX {
            self.journal.drain(..JOURNAL_DROP);
        }
    }

    /// Runs a command and, when it starts a job, waits up to `timeout` for the job's result.
    pub fn run_blocking(&mut self, id: &str, params: Json, timeout: Duration) -> Result<Json, EngineError> {
        match self.execute(id, params)? {
            Outcome::Done(v) => Ok(v),
            Outcome::Started(job) => self.wait(job, timeout),
        }
    }

    /// Applies the results of finished jobs to the state (each under the panic guard) and
    /// returns them, oldest first, including those applied while [`Session::wait`] or
    /// [`Session::wait_idle`] waited. Never blocks; the UI calls it every frame.
    pub fn pump(&mut self) -> Vec<(JobId, Result<Json, EngineError>)> {
        let mut out = self.jobs.take_delivered();
        while let Some(done) = self.jobs.try_next() {
            out.push(self.apply(done));
        }
        out
    }

    /// Waits up to `timeout` for job `job` and returns its result. Other jobs' results that
    /// arrive meanwhile are applied and kept for the next [`Session::pump`]. A timeout leaves the
    /// job running. A job that is not running and whose result was already returned is
    /// `BadParams`.
    pub fn wait(&mut self, job: JobId, timeout: Duration) -> Result<Json, EngineError> {
        if let Some(result) = self.jobs.take_delivered_job(job) {
            return result;
        }
        if !self.jobs.is_running(job) {
            return Err(EngineError::BadParams { id: "wait".into(), msg: format!("job {} is not running", job.0) });
        }
        let deadline = jobs::Deadline::after(timeout);
        loop {
            let Some(done) = self.jobs.next_before(&deadline) else { return Err(EngineError::Timeout) };
            let (id, result) = self.apply(done);
            if id == job {
                return result;
            }
            self.jobs.deliver(id, result);
        }
    }

    /// Waits up to `timeout` until no job is running; `false` when time ran out. Results are
    /// applied as they arrive and kept for the next [`Session::pump`].
    pub fn wait_idle(&mut self, timeout: Duration) -> bool {
        let deadline = jobs::Deadline::after(timeout);
        loop {
            while let Some(done) = self.jobs.try_next() {
                let (id, result) = self.apply(done);
                self.jobs.deliver(id, result);
            }
            if !self.busy() {
                return true;
            }
            let Some(done) = self.jobs.next_before(&deadline) else { return !self.busy() };
            let (id, result) = self.apply(done);
            self.jobs.deliver(id, result);
        }
    }

    /// A job is running: started and its result not yet applied.
    pub fn busy(&self) -> bool {
        self.jobs.busy()
    }

    /// A job that command `command` started is running (a job is labelled with its command's
    /// id), e.g. `connection.reconnect`. Its result may still be ignored when it arrives (see
    /// [`cmd`]); a tab's own jobs are named by the tab's markers instead.
    pub fn running(&self, command: &str) -> bool {
        self.jobs.is_running_label(command)
    }

    /// Every registered command with whether it can run now. A panicking `enabled` check shows
    /// as disabled.
    pub fn commands(&self) -> Vec<CommandInfo> {
        cmd::specs()
            .map(|spec| {
                let enabled = catch_unwind(AssertUnwindSafe(|| (spec.enabled)(self))).unwrap_or_else(|_| {
                    log::error!("command {}: its enabled check panicked", spec.id);
                    Err("internal error in its enabled check".into())
                });
                CommandInfo::new(spec, enabled)
            })
            .collect()
    }

    /// The only folders this run may touch files in, an isolated run's; `None` is no limit
    /// ([`SessionOptions::allowed_dirs`]).
    pub fn allowed_dirs(&self) -> Option<&[PathBuf]> {
        self.allowed_dirs.as_deref()
    }

    /// Successful journaled commands, oldest first, with their params minus secrets. Replaying
    /// them in order on a new session repeats the work (secrets must then come from the store).
    pub fn journal(&self) -> &[(String, Json)] {
        &self.journal
    }

    /// Dialogs and notices for the UI, since the last call.
    pub fn take_ui_requests(&mut self) -> Vec<UiRequest> {
        std::mem::take(&mut self.ui_requests)
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    /// The database types this session can open: those a driver opens, in [`DbKind::ALL`]
    /// order.
    pub fn kinds(&self) -> Vec<DbKind> {
        self.drivers.kinds()
    }

    /// The value panel's content: the focus cell of the active tab's selection, with staged
    /// values, pretty-printed when it is JSON and cut to [`VALUE_VIEW_MAX_BYTES`]. `None` without
    /// a selection. Cheap to call every frame: the view is built once per tab, cell, page and
    /// staged-edit state (see [`view`]); this copies the text, [`Session::value_view_shared`]
    /// does not.
    pub fn value_view(&self) -> Option<ValueView> {
        self.value_view_shared().map(|v| (*v).clone())
    }

    /// [`Session::value_view`] without the copy of the text.
    pub fn value_view_shared(&self) -> Option<Arc<ValueView>> {
        self.value_cache.get(self.state.active_tab()?)
    }

    /// How many operations wait for their turn on the browse connection (for tests of the queue).
    #[doc(hidden)]
    pub fn browse_waiting(&self) -> usize {
        self.state.connection(Which::Active).map_or(0, |c| c.browse.waiting())
    }

    /// How many value views this session has built (for tests of the caching).
    #[doc(hidden)]
    pub fn value_view_computations(&self) -> usize {
        self.value_cache.built()
    }

    /// A JSON summary for agents and tests (`document.inspect`): the screen, the database types
    /// the session can open (`kinds`, their ids), the active tab's connection and every open one,
    /// the tabs and the active one in detail, and the running jobs. Never contains a secret.
    pub fn inspect(&self) -> Json {
        let st = &self.state;
        let summary = |c: &Connected| c.summary(st.store.is_saved(&c.config.id));
        let connection = st.connection(Which::Active).map(summary);
        let active = st.active_tab();
        let query = match active {
            Some(Tab::Query(q)) => cmd::query::summary(q, st.connection(Which::Tab(q.id))),
            _ => Json::Null,
        };
        let table = match active {
            Some(Tab::Table(t)) => cmd::table::summary(self, t),
            _ => Json::Null,
        };
        json!({
            "screen": st.screen,
            "kinds": self.kinds(),
            "connection": connection,
            "connections": st.connections.iter().map(summary).collect::<Vec<_>>(),
            "storeWarning": st.store_warning,
            "secretStore": self.secrets.as_ref().map(|s| s.name()),
            "tabs": st.tabs.iter().map(Tab::summary).collect::<Vec<_>>(),
            "active": st.active,
            "query": query,
            "table": table,
            "selection": view::selection_json(active),
            "value": self.value_view_shared().map_or(Json::Null, |v| view::summary(&v)),
            "jobs": self.jobs.running().map(|(job, label)| json!({ "job": job, "label": label })).collect::<Vec<_>>(),
        })
    }

    /// Starts `fut` on the job runtime and returns its id at once. When the future finishes, its
    /// [`Apply`] runs on the caller's thread in [`Session::pump`] (or a wait) under the panic
    /// guard; a panic in the future becomes `Internal` naming `label`. A job is never aborted:
    /// it runs to its end, and an `Apply` whose tab is gone must ignore its result. A job started
    /// by a call that then panics is orphaned: its `Apply` is dropped.
    pub(crate) fn spawn_job(&mut self, label: &'static str, fut: impl Future<Output = Apply> + Send + 'static) -> JobId {
        self.jobs.spawn(label, fut)
    }

    /// [`Session::spawn_job`] for a job that `stop` stops (`Jobs::stop`).
    pub(crate) fn spawn_stoppable_job(
        &mut self,
        label: &'static str,
        stop: Arc<jobs::Stop>,
        fut: impl Future<Output = Apply> + Send + 'static,
    ) -> JobId {
        self.jobs.spawn_stoppable(label, stop, fut)
    }

    /// [`Session::spawn_job`] for a connection attempt, made by `attempt` with the `Stop` that
    /// `connection.cancel` sets.
    pub(crate) fn spawn_attempt<F>(&mut self, label: &'static str, attempt: impl FnOnce(Arc<jobs::Stop>) -> F) -> JobId
    where
        F: Future<Output = Apply> + Send + 'static,
    {
        let stop = Arc::new(jobs::Stop::default());
        let fut = attempt(Arc::clone(&stop));
        self.jobs.spawn_attempt(label, stop, fut)
    }

    /// The job whose result is being applied, inside an [`Apply`]; `None` elsewhere.
    pub(crate) fn applying_job(&self) -> Option<JobId> {
        self.jobs.applying()
    }

    fn apply(&mut self, done: jobs::Finished) -> (JobId, Result<Json, EngineError>) {
        let jobs::Finished { job, label, outcome, took } = done;
        let orphan = self.jobs.finish(job);
        let result = match outcome {
            Ok(apply) if orphan => {
                drop(apply);
                Err(EngineError::Internal { id: label.to_string(), msg: "the call that started this job panicked, so its result was dropped".into() })
            }
            Ok(apply) => {
                self.jobs.set_applying(Some(job));
                let r = self.guarded(label, apply);
                self.jobs.set_applying(None);
                r
            }
            Err(e) => Err(e),
        };
        self.job_ended(job);
        match &result {
            Ok(_) => log::debug!("job {} ({label}): done in {took:?}", job.0),
            Err(e) => log::debug!("job {} ({label}): {} in {took:?}", job.0, e.code()),
        }
        (job, result)
    }

    /// No marker outlives its job: once `job`'s result was applied, or dropped because the job's
    /// future or its `Apply` panicked (and the state was put back with the markers it had), every
    /// marker that still names it is cleared. A query run (or a commit or rollback) whose marker
    /// is still there was not applied, so where it left the query connection, and its transaction,
    /// are not known.
    fn job_ended(&mut self, job: JobId) {
        for c in &mut self.state.connections {
            if c.query_run.as_ref().is_some_and(|run| run.job == job) {
                c.query_run = None;
                c.query_database.known = false;
                c.transaction = Some(Transaction { state: dbee_driver::TransactionState::Unknown, tab: None });
            }
        }
        for tab in &mut self.state.tabs {
            match tab {
                Tab::Table(t) => t.job_ended(job),
                Tab::Query(q) => {
                    if q.running == Some(job) {
                        q.running = None;
                    }
                }
            }
        }
    }
}

/// The text of a panic, cut to a few hundred characters. It is returned to the caller, never
/// logged: it can quote data.
pub(crate) fn panic_text(payload: &(dyn Any + Send)) -> String {
    let text = payload.downcast_ref::<&str>().copied().or_else(|| payload.downcast_ref::<String>().map(String::as_str)).unwrap_or("a panic");
    const MAX: usize = 500;
    if text.chars().count() > MAX { format!("{}…", text.chars().take(MAX).collect::<String>()) } else { text.to_string() }
}

#[cfg(test)]
mod tests;
