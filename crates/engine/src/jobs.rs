//! The job runtime: database work runs as futures on a small tokio runtime, and each future
//! ends in an [`Apply`] that changes the state on the caller's thread.
//!
//! A job is never aborted. Its task is detached (the join handle is dropped, which does not
//! cancel it) and runs to its end, so a Postgres apply is never abandoned mid-transaction; work
//! is stopped cooperatively, through `dbee_pg::Canceller`. A result whose tab has gone is
//! ignored by its `Apply`.

use std::collections::{BTreeMap, VecDeque};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use futures_util::FutureExt as _;

use crate::{EngineError, JobId, Json, Session, panic_text};

/// What a finished job does to the session, run on the caller's thread by [`Session::pump`].
pub(crate) type Apply = Box<dyn FnOnce(&mut Session) -> Result<Json, EngineError> + Send>;

/// Worker threads of the job runtime.
const WORKERS: usize = 2;
/// Results kept for the next pump at most (a session that only ever waits for single jobs).
const DELIVERED_MAX: usize = 10_000;
/// The longest single wait on the channel; longer waits loop.
const WAIT_SLICE: Duration = Duration::from_secs(60);

/// A job's future has finished.
pub(crate) struct Finished {
    pub job: JobId,
    pub label: &'static str,
    /// The `Apply`, or `Internal` when the future panicked.
    pub outcome: Result<Apply, EngineError>,
    /// How long the future ran.
    pub took: Duration,
}

/// When a wait gives up. `None` is later than an `Instant` can say: never.
pub(crate) struct Deadline(Option<Instant>);

impl Deadline {
    pub fn after(timeout: Duration) -> Deadline {
        Deadline(Instant::now().checked_add(timeout))
    }

    /// The time left, at most [`WAIT_SLICE`]; `None` once the deadline has passed.
    fn left(&self) -> Option<Duration> {
        match self.0 {
            None => Some(WAIT_SLICE),
            Some(at) => {
                let left = at.saturating_duration_since(Instant::now());
                if left.is_zero() { None } else { Some(left.min(WAIT_SLICE)) }
            }
        }
    }
}

pub(crate) struct Jobs {
    runtime: tokio::runtime::Runtime,
    tx: mpsc::Sender<Finished>,
    rx: mpsc::Receiver<Finished>,
    /// Started and not yet applied, with their labels.
    running: BTreeMap<JobId, &'static str>,
    /// Applied while a wait was waiting for something else; the next pump returns them.
    delivered: VecDeque<(JobId, Result<Json, EngineError>)>,
    last: u64,
    applying: Option<JobId>,
    notify: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Jobs {
    pub fn new(notify: Option<Arc<dyn Fn() + Send + Sync>>) -> Result<Jobs, EngineError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(WORKERS)
            .thread_name("dbee-db")
            .enable_all()
            .build()
            .map_err(|e| EngineError::Internal { id: "session".into(), msg: format!("could not start the job runtime: {e}") })?;
        let (tx, rx) = mpsc::channel();
        Ok(Jobs { runtime, tx, rx, running: BTreeMap::new(), delivered: VecDeque::new(), last: 0, applying: None, notify })
    }

    pub fn spawn(&mut self, label: &'static str, fut: impl Future<Output = Apply> + Send + 'static) -> JobId {
        self.last = self.last.saturating_add(1);
        let job = JobId(self.last);
        self.running.insert(job, label);
        let tx = self.tx.clone();
        let notify = self.notify.clone();
        log::debug!("job {} ({label}): started", job.0);
        let task = self.runtime.spawn(async move {
            let start = Instant::now();
            let outcome = AssertUnwindSafe(fut).catch_unwind().await.map_err(|payload| {
                log::error!("job {} ({label}) panicked", job.0);
                EngineError::Internal { id: label.to_string(), msg: panic_text(payload.as_ref()) }
            });
            // Fails only when the session is gone, and then nobody wants the result.
            let _ = tx.send(Finished { job, label, outcome, took: start.elapsed() });
            if let Some(notify) = notify {
                notify();
            }
        });
        // Detach: dropping the handle does not cancel the task.
        drop(task);
        job
    }

    pub fn busy(&self) -> bool {
        !self.running.is_empty()
    }

    pub fn is_running(&self, job: JobId) -> bool {
        self.running.contains_key(&job)
    }

    pub fn running(&self) -> impl Iterator<Item = (JobId, &'static str)> + '_ {
        self.running.iter().map(|(j, l)| (*j, *l))
    }

    /// The job's result is being applied: it no longer counts as running.
    pub fn finish(&mut self, job: JobId) {
        self.running.remove(&job);
    }

    pub fn applying(&self) -> Option<JobId> {
        self.applying
    }

    pub fn set_applying(&mut self, job: Option<JobId>) {
        self.applying = job;
    }

    /// A finished job, without waiting.
    pub fn try_next(&mut self) -> Option<Finished> {
        self.rx.try_recv().ok()
    }

    /// The next finished job, waiting until `deadline`.
    pub fn next_before(&mut self, deadline: &Deadline) -> Option<Finished> {
        loop {
            if let Ok(done) = self.rx.try_recv() {
                return Some(done);
            }
            let left = deadline.left()?;
            match self.rx.recv_timeout(left) {
                Ok(done) => return Some(done),
                Err(RecvTimeoutError::Timeout) => {}
                // `self.tx` keeps the channel open; nothing can arrive if it ever closes.
                Err(RecvTimeoutError::Disconnected) => return None,
            }
        }
    }

    /// Keeps an applied result for the next pump.
    pub fn deliver(&mut self, job: JobId, result: Result<Json, EngineError>) {
        if self.delivered.len() >= DELIVERED_MAX {
            log::warn!("dropping the oldest unpumped job result");
            self.delivered.pop_front();
        }
        self.delivered.push_back((job, result));
    }

    pub fn take_delivered(&mut self) -> Vec<(JobId, Result<Json, EngineError>)> {
        self.delivered.drain(..).collect()
    }

    pub fn take_delivered_job(&mut self, job: JobId) -> Option<Result<Json, EngineError>> {
        let at = self.delivered.iter().position(|(j, _)| *j == job)?;
        self.delivered.remove(at).map(|(_, r)| r)
    }
}
