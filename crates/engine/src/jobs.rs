//! The job runtime: database work runs as futures on a small tokio runtime, and each future
//! ends in an [`Apply`] that changes the state on the caller's thread.
//!
//! A job is never aborted. Its task is detached (the join handle is dropped, which does not
//! cancel it) and runs to its end, so an apply is never abandoned mid-transaction; work is
//! stopped cooperatively, through the connection's `dbee_driver::Cancel`. A result whose tab has
//! gone is ignored by its `Apply`.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use dbee_driver::{Cancel, DbError};
use futures_util::FutureExt as _;
use tokio::sync::Notify;

use crate::{EngineError, JobId, Json, Session, panic_text};

/// What a finished job does to the session, run on the caller's thread by [`Session::pump`].
pub(crate) type Apply = Box<dyn FnOnce(&mut Session) -> Result<Json, EngineError> + Send>;

/// Asks a running job to stop, cooperatively: set once and for good. The job looks at it between
/// steps and waits on it while a step runs (a query run, a page load or a count: before each
/// statement, and while one runs, when it sends the cancel request itself, through
/// [`stoppable`]).
#[derive(Default)]
pub(crate) struct Stop {
    set: AtomicBool,
    notify: Notify,
}

impl Stop {
    /// Sets it; false when it was set already.
    pub(crate) fn stop(&self) -> bool {
        let was = self.set.swap(true, Ordering::AcqRel);
        self.notify.notify_waiters();
        !was
    }

    pub(crate) fn is_set(&self) -> bool {
        self.set.load(Ordering::Acquire)
    }

    /// Returns once [`Stop::stop`] was called, also when that was before.
    pub(crate) async fn stopped(&self) {
        loop {
            let notified = self.notify.notified();
            let mut notified = std::pin::pin!(notified);
            // Waiting from here on, so a stop after the check below wakes it.
            notified.as_mut().enable();
            if self.is_set() {
                return;
            }
            notified.await;
        }
    }
}

impl std::fmt::Debug for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stop").field("set", &self.is_set()).finish()
    }
}

/// Set once a job's result is on its way to the session ([`Jobs::sent`]). A job that waits for it
/// before it ends sends its own result after that one, so the session applies the two in that
/// order.
#[derive(Clone, Default)]
pub(crate) struct Sent(Arc<Stop>);

impl Sent {
    /// Returns once the result was sent, also when that was before.
    pub(crate) async fn wait(&self) {
        self.0.stopped().await;
    }

    fn set(&self) {
        self.0.stop();
    }
}

/// How long a stopped statement's first cancel request has to stop it before the request is sent
/// again, and then how often it is sent while the statement has not ended.
const CANCEL_AGAIN_AFTER: Duration = Duration::from_secs(1);
const CANCEL_AGAIN_EVERY: Duration = Duration::from_secs(2);

/// Runs `statement`, a statement on the connection `cancel` stops, to its end. Once `stop` is set
/// the cancel request goes out, and again after [`CANCEL_AGAIN_AFTER`], then every
/// [`CANCEL_AGAIN_EVERY`], while the statement has not ended: a request that reaches the server
/// before the statement does is lost. Each request is waited for before the next is sent and
/// before this returns, so none is still on its way when the connection's next statement starts
/// (the drivers then start none either). Stopped before it started, the statement is not run.
///
/// Whose stop it was is decided here. The statement ends `Cancelled` when it was stopped before
/// it started, or when it ends in the server's interrupt once a request went out for it. An
/// interrupt before that (a timeout, another session's cancel) stays the server's error, also
/// when the stop comes after it, and a statement that ended any other way keeps its end.
pub(crate) async fn stoppable<T>(cancel: Arc<dyn Cancel>, statement: impl Future<Output = Result<T, DbError>>, stop: &Stop) -> Result<T, DbError> {
    stoppable_with(cancel, statement, stop, CANCEL_AGAIN_AFTER, CANCEL_AGAIN_EVERY).await
}

async fn stoppable_with<T>(
    cancel: Arc<dyn Cancel>,
    statement: impl Future<Output = Result<T, DbError>>,
    stop: &Stop,
    after: Duration,
    every: Duration,
) -> Result<T, DbError> {
    if stop.is_set() {
        return Err(DbError::Cancelled);
    }
    let mut statement = std::pin::pin!(statement);
    tokio::select! {
        biased;
        ended = &mut statement => return ended,
        () = stop.stopped() => {}
    }
    let mut wait = after;
    loop {
        let mut sending = std::pin::pin!(cancel.cancel());
        let mut ended = None;
        let sent = loop {
            tokio::select! {
                sent = &mut sending => break sent,
                done = &mut statement, if ended.is_none() => ended = Some(done),
            }
        };
        if sent.is_err() {
            log::info!("a cancel request was not delivered");
        }
        if let Some(ended) = ended {
            return cancelled_by_dbee(ended);
        }
        if let Ok(ended) = tokio::time::timeout(wait, &mut statement).await {
            return cancelled_by_dbee(ended);
        }
        wait = every;
    }
}

/// `work`, or `None` once `stop` is set (also when both are ready): `work` is then dropped where it
/// stands, which closes what it holds, such as a connection attempt's socket. For work no cancel
/// request reaches, as a connect; a statement stops through [`stoppable`].
pub(crate) async fn unless_stopped<T>(stop: &Stop, work: impl Future<Output = T>) -> Option<T> {
    tokio::select! {
        biased;
        () = stop.stopped() => None,
        done = work => Some(done),
    }
}

/// The end of a statement dbee sent a cancel request for: the server's interrupt is that cancel.
fn cancelled_by_dbee<T>(ended: Result<T, DbError>) -> Result<T, DbError> {
    ended.map_err(|e| if e.is_interrupted() { DbError::Cancelled } else { e })
}

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
    /// Owned for the shutdown on drop; always `Some` until then.
    runtime: Option<tokio::runtime::Runtime>,
    handle: tokio::runtime::Handle,
    tx: mpsc::Sender<Finished>,
    rx: mpsc::Receiver<Finished>,
    /// Started and not yet applied, with their labels.
    running: BTreeMap<JobId, &'static str>,
    /// Started inside a call that panicked: they run to their end, and their `Apply` is dropped.
    orphaned: BTreeSet<JobId>,
    /// Applied while a wait was waiting for something else; the next pump returns them.
    delivered: VecDeque<(JobId, Result<Json, EngineError>)>,
    /// How to stop the running jobs that can be stopped ([`Jobs::spawn_stoppable`]).
    stops: BTreeMap<JobId, Arc<Stop>>,
    /// How to stop the running connection attempts ([`Jobs::spawn_attempt`]); kept apart from
    /// `stops`, as closing or reconnecting a connection stops what runs on it, never an attempt.
    attempts: BTreeMap<JobId, Arc<Stop>>,
    /// Each running job's [`Sent`].
    sent: BTreeMap<JobId, Sent>,
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
        let handle = runtime.handle().clone();
        let (tx, rx) = mpsc::channel();
        Ok(Jobs {
            runtime: Some(runtime),
            handle,
            tx,
            rx,
            running: BTreeMap::new(),
            orphaned: BTreeSet::new(),
            delivered: VecDeque::new(),
            stops: BTreeMap::new(),
            attempts: BTreeMap::new(),
            sent: BTreeMap::new(),
            last: 0,
            applying: None,
            notify,
        })
    }

    pub fn spawn(&mut self, label: &'static str, fut: impl Future<Output = Apply> + Send + 'static) -> JobId {
        self.last = self.last.saturating_add(1);
        let job = JobId(self.last);
        self.running.insert(job, label);
        let sent = Sent::default();
        self.sent.insert(job, sent.clone());
        let tx = self.tx.clone();
        let notify = self.notify.clone();
        log::debug!("job {} ({label}): started", job.0);
        let task = self.handle.spawn(async move {
            let start = Instant::now();
            let outcome = AssertUnwindSafe(fut).catch_unwind().await.map_err(|payload| {
                log::error!("job {} ({label}) panicked", job.0);
                EngineError::Internal { id: label.to_string(), msg: panic_text(payload.as_ref()) }
            });
            // Fails only when the session is gone, and then nobody wants the result.
            let _ = tx.send(Finished { job, label, outcome, took: start.elapsed() });
            sent.set();
            if let Some(notify) = notify {
                notify();
            }
        });
        // Detach: dropping the handle does not cancel the task.
        drop(task);
        job
    }

    /// [`Jobs::spawn`] for a job that `stop` stops ([`Jobs::stop`]).
    pub fn spawn_stoppable(&mut self, label: &'static str, stop: Arc<Stop>, fut: impl Future<Output = Apply> + Send + 'static) -> JobId {
        let job = self.spawn(label, fut);
        self.stops.insert(job, stop);
        job
    }

    /// The [`Sent`] of job `job`, while its result was not applied; `None` once it was, when
    /// there is nothing to wait for.
    pub fn sent(&self, job: JobId) -> Option<Sent> {
        self.sent.get(&job).cloned()
    }

    /// Stops the running job `job`: `Some(true)` when this stopped it, `Some(false)` when it was
    /// stopping already, `None` when no such job runs that can be stopped.
    pub fn stop(&self, job: JobId) -> Option<bool> {
        self.stops.get(&job).map(|stop| stop.stop())
    }

    /// [`Jobs::spawn`] for a connection attempt that `stop` stops ([`Jobs::stop_attempts`]).
    pub fn spawn_attempt(&mut self, label: &'static str, stop: Arc<Stop>, fut: impl Future<Output = Apply> + Send + 'static) -> JobId {
        let job = self.spawn(label, fut);
        self.attempts.insert(job, stop);
        job
    }

    /// Stops every running connection attempt: how many this stopped, and how many were stopping
    /// already.
    pub fn stop_attempts(&self) -> (usize, usize) {
        let (mut stopped, mut stopping) = (0usize, 0usize);
        for stop in self.attempts.values() {
            if stop.stop() {
                stopped = stopped.saturating_add(1);
            } else {
                stopping = stopping.saturating_add(1);
            }
        }
        (stopped, stopping)
    }

    /// Stops every running job that can be stopped, except the connection attempts.
    pub fn stop_all(&self) {
        for stop in self.stops.values() {
            stop.stop();
        }
    }

    /// A job runs that cannot be stopped through [`Jobs::stop`] (an Apply, a catalog load).
    pub fn unstoppable_running(&self) -> bool {
        self.running.keys().any(|job| !self.stops.contains_key(job))
    }

    /// Runs `fut` on the runtime with no result and no tracking: it never counts as a running
    /// job (cancel requests sent while disconnecting). A panic in it ends only that task.
    pub fn spawn_detached(&self, fut: impl Future<Output = ()> + Send + 'static) {
        // Detach: dropping the handle does not cancel the task.
        drop(self.handle.spawn(fut));
    }

    /// A job with this label is running.
    pub fn is_running_label(&self, label: &str) -> bool {
        self.running.values().any(|l| *l == label)
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

    /// The id of the newest job so far; jobs started later have larger ids.
    pub fn last_id(&self) -> u64 {
        self.last
    }

    /// Orphans every running job started after [`Jobs::last_id`] was `last`; returns how many.
    pub fn orphan_since(&mut self, last: u64) -> usize {
        let since: Vec<JobId> = self.running.keys().copied().filter(|j| j.0 > last).collect();
        let n = since.len();
        self.orphaned.extend(since);
        n
    }

    /// The job's result is being applied: it no longer counts as running. `true` when it is an
    /// orphan, whose `Apply` must be dropped.
    pub fn finish(&mut self, job: JobId) -> bool {
        self.running.remove(&job);
        self.stops.remove(&job);
        self.attempts.remove(&job);
        self.sent.remove(&job);
        self.orphaned.remove(&job)
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

impl Drop for Jobs {
    /// Shuts the runtime down without waiting for its threads, so a session can be dropped
    /// anywhere, async code included. Jobs still running are dropped with it (at quit), and an
    /// open transaction ends with its connection, rolled back by the server.
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn stop_is_seen_before_and_while_waiting() {
        let stop = Arc::new(Stop::default());
        assert!(!stop.is_set());
        let waiting = tokio::spawn({
            let stop = Arc::clone(&stop);
            async move { stop.stopped().await }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        stop.stop();
        tokio::time::timeout(Duration::from_secs(5), waiting).await.unwrap().unwrap();
        // Once set, it stays set, and waiting returns at once.
        assert!(stop.is_set());
        tokio::time::timeout(Duration::from_secs(5), stop.stopped()).await.unwrap();
        assert!(!stop.stop(), "set already");
    }

    /// A job that waits for another's [`Sent`] before it ends comes in after it, also when it
    /// would finish first; waiting for a job whose result is already in returns at once.
    #[test]
    fn a_job_waiting_for_anothers_result_comes_in_after_it() {
        let mut jobs = Jobs::new(None).unwrap();
        let release = Arc::new(Stop::default());
        let held = Arc::clone(&release);
        let first = jobs.spawn("first", async move {
            held.stopped().await;
            let apply: Apply = Box::new(|_| Ok(Json::Null));
            apply
        });
        let sent = jobs.sent(first).unwrap();
        let second = jobs.spawn("second", async move {
            sent.wait().await;
            let apply: Apply = Box::new(|_| Ok(Json::Null));
            apply
        });
        release.stop();
        let deadline = Deadline::after(Duration::from_secs(10));
        let order: Vec<JobId> = (0..2).filter_map(|_| jobs.next_before(&deadline).map(|done| done.job)).collect();
        assert_eq!(order, [first, second]);
        assert!(jobs.sent(first).is_some(), "until its result is applied");
        jobs.finish(first);
        assert!(jobs.sent(first).is_none(), "nothing to wait for once applied");
    }

    /// A statement that ends only once a cancel request reaches it, and a canceller whose first
    /// `lost` requests reach nothing (they arrive before the statement does). Every request takes
    /// a moment on its way; one is never sent while another is.
    #[derive(Default)]
    struct Remote {
        lost: usize,
        sent: std::sync::atomic::AtomicUsize,
        on_its_way: AtomicBool,
        reached: Notify,
        stopped: AtomicBool,
    }

    impl Cancel for Remote {
        fn cancel(&self) -> dbee_driver::DbFuture<'_, ()> {
            Box::pin(async move {
                assert!(!self.on_its_way.swap(true, Ordering::SeqCst), "two cancel requests on their way at once");
                tokio::task::yield_now().await;
                if self.sent.fetch_add(1, Ordering::SeqCst) >= self.lost {
                    self.stopped.store(true, Ordering::SeqCst);
                    self.reached.notify_waiters();
                }
                tokio::task::yield_now().await;
                self.on_its_way.store(false, Ordering::SeqCst);
                Ok(())
            })
        }
    }

    impl Remote {
        /// Ends with the server's interrupt once a request reached it, as a Postgres or MySQL
        /// statement does.
        async fn statement(&self) -> Result<u64, DbError> {
            loop {
                let reached = self.reached.notified();
                if self.stopped.load(Ordering::SeqCst) {
                    return Err(interrupt());
                }
                reached.await;
            }
        }
    }

    /// The server's interrupt: a cancel request's, or a timeout's with the same code.
    fn interrupt() -> DbError {
        DbError::Server(Box::new(dbee_driver::ServerError {
            code: "57014".into(),
            code_label: Some("SQLSTATE"),
            message: "canceling statement due to statement timeout".into(),
            detail: None,
            hint: None,
            position: None,
            meaning: Some(dbee_driver::Meaning::Interrupted),
        }))
    }

    const SOON: Duration = Duration::from_millis(10);

    #[tokio::test]
    async fn a_lost_cancel_is_sent_again_until_the_statement_ends() {
        for lost in [0, 1, 3] {
            let remote = Arc::new(Remote { lost, ..Remote::default() });
            let stop = Stop::default();
            let running = stoppable_with(Arc::clone(&remote) as Arc<dyn Cancel>, remote.statement(), &stop, SOON, SOON);
            let stopping = async {
                tokio::task::yield_now().await;
                stop.stop();
            };
            let (ended, ()) = tokio::time::timeout(Duration::from_secs(10), async { tokio::join!(running, stopping) }).await.unwrap();
            assert_eq!(ended, Err(DbError::Cancelled), "{lost} lost");
            assert_eq!(remote.sent.load(Ordering::SeqCst), lost + 1, "sent until one reached it, and no more");
            assert!(!remote.on_its_way.load(Ordering::SeqCst), "the last request had arrived");
        }
    }

    #[tokio::test]
    async fn a_statement_that_ends_meanwhile_waits_for_the_request_on_its_way() {
        let remote = Arc::new(Remote::default());
        let stop = Stop::default();
        stop.stop();
        // Stopped before it started: nothing is sent, and the statement does not run.
        let never = async { panic!("the statement ran") };
        assert_eq!(stoppable_with::<u64>(Arc::clone(&remote) as Arc<dyn Cancel>, never, &stop, SOON, SOON).await, Err(DbError::Cancelled));
        assert_eq!(remote.sent.load(Ordering::SeqCst), 0);
        // It ends by itself while the request is on its way: it returns once that has arrived.
        let stop = Stop::default();
        let ends = async {
            stop.stopped().await;
            tokio::task::yield_now().await;
            Ok(7)
        };
        let stopping = async {
            tokio::task::yield_now().await;
            stop.stop();
        };
        let (ended, ()) = tokio::join!(stoppable_with(Arc::clone(&remote) as Arc<dyn Cancel>, ends, &stop, SOON, SOON), stopping);
        assert_eq!(ended, Ok(7));
        assert!(!remote.on_its_way.load(Ordering::SeqCst), "the request had arrived before it returned");
        // Not stopped: the statement's own end, and no request.
        let quiet = Arc::new(Remote::default());
        assert_eq!(stoppable_with(Arc::clone(&quiet) as Arc<dyn Cancel>, async { Ok(1) }, &Stop::default(), SOON, SOON).await, Ok(1));
        assert_eq!(quiet.sent.load(Ordering::SeqCst), 0);
    }

    /// The server's interrupt is dbee's cancel only once dbee sent one for the statement: before
    /// that (a timeout, another session's cancel) it stays the server's error, also when the stop
    /// comes after the statement ended; once a request went out, an interrupt that ends the
    /// statement while it is on its way is dbee's. dbee's own `Cancelled` and other errors pass.
    #[tokio::test]
    async fn only_an_interrupt_after_dbees_cancel_reads_as_cancelled() {
        let remote = Arc::new(Remote::default());
        let quiet = || Arc::clone(&remote) as Arc<dyn Cancel>;
        let server = async { Err::<u64, _>(interrupt()) };
        assert_eq!(stoppable_with(quiet(), server, &Stop::default(), SOON, SOON).await, Err(interrupt()));
        // It ended by itself before the stop: the stop sends nothing and changes nothing.
        let stop = Stop::default();
        let ended_first = async {
            tokio::task::yield_now().await;
            Err::<u64, _>(interrupt())
        };
        let stopping = async {
            for _ in 0..3 {
                tokio::task::yield_now().await;
            }
            stop.stop();
        };
        let (ended, ()) = tokio::join!(stoppable_with(quiet(), ended_first, &stop, SOON, SOON), stopping);
        assert_eq!(ended, Err(interrupt()));
        assert_eq!(remote.sent.load(Ordering::SeqCst), 0);
        // A server's timeout that lands while dbee's request is on its way: the request went out.
        let stop = Stop::default();
        let meanwhile = async {
            stop.stopped().await;
            tokio::task::yield_now().await;
            Err::<u64, _>(interrupt())
        };
        let stopping = async {
            tokio::task::yield_now().await;
            stop.stop();
        };
        let (ended, ()) = tokio::join!(stoppable_with(quiet(), meanwhile, &stop, SOON, SOON), stopping);
        assert_eq!(ended, Err(DbError::Cancelled));
        assert_eq!(remote.sent.load(Ordering::SeqCst), 1);
        // Other ends of a stopped statement are kept as they are.
        for other in [DbError::Cancelled, DbError::ConnectionLost, DbError::Other("a broken answer".into())] {
            let stop = Stop::default();
            let ends = async {
                stop.stopped().await;
                tokio::task::yield_now().await;
                Err::<u64, _>(other.clone())
            };
            let stopping = async {
                tokio::task::yield_now().await;
                stop.stop();
            };
            let (ended, ()) = tokio::join!(stoppable_with(quiet(), ends, &stop, SOON, SOON), stopping);
            assert_eq!(ended, Err(other));
        }
    }
}
