//! Pause/resume/cancel control for a running download job.
//!
//! `DownloadControl` is a cheap-to-clone handle: the caller keeps one clone to
//! drive a job's lifecycle (`pause`/`resume`/`cancel`) and observe it
//! (`subscribe`), while another clone is threaded down into every chunk's
//! download future as the gate it must pass before doing any I/O.
//!
//! This is deliberately a *separate* channel from the job's existing
//! `mpsc::UnboundedSender<DownloadEvent>`/`DownloadJobEvent`/`RepairEvent`
//! progress reporting (see `events.rs`) — those enums keep meaning "download
//! work" only (bytes/chunks/errors); control state lives here instead.

use std::sync::{Arc, Mutex};

use tokio::sync::watch;

/// What the frontend observes via `DownloadControl::subscribe()`.
///
/// Pause and cancel are both two-phase: the transient `Pausing`/`Cancelling`
/// states hold while chunks already past the gate drain to completion, and
/// the settled `Paused`/`Cancelled` states are only reached once nothing is
/// in flight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobStatus {
    Running,
    /// Pause requested; one or more chunks are still draining.
    Pausing,
    /// Pause requested; no chunks are in flight.
    Paused,
    /// Cancel requested; one or more chunks are still draining.
    Cancelling,
    /// Cancel requested; no chunks are in flight. Terminal.
    Cancelled,
    /// The job's future resolved (successfully or with an error) without
    /// being cancelled. Terminal — `subscribe()` watchers should stop on
    /// this edge (or on `Cancelled`); no further transitions follow.
    Completed,
}

/// The gate's own instruction state. Kept separate from `JobStatus` because
/// the gate only ever needs to know "run / wait / stop," while `JobStatus`
/// additionally encodes whether anything is still draining.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Request {
    Run,
    Pause,
    Cancel,
}

struct St {
    request: Request,
    /// Number of chunk futures currently past the gate (claimed a slot via
    /// `wait_to_proceed` and haven't yet dropped their `ActiveGuard`).
    active: usize,
    /// Set once the job's future resolves (see `complete`). Terminal: it
    /// makes the published status `Completed` (unless a cancel was
    /// requested, which wins) and turns `pause`/`resume`/`cancel` into
    /// no-ops.
    completed: bool,
}

struct Inner {
    st: Mutex<St>,
    /// Wakes chunk futures parked in `wait_to_proceed`. Sent to only while
    /// `st`'s lock is held, so a gate that observes `Request::Pause` under
    /// the same lock can never miss the subsequent wakeup.
    request_tx: watch::Sender<Request>,
    /// What the frontend observes. Updated whenever `request` or `active`
    /// changes in a way that affects the externally-visible status.
    status_tx: watch::Sender<JobStatus>,
}

impl Inner {
    /// Recomputes `JobStatus` from the current `request`/`active` and
    /// publishes it. Called with `st` already locked.
    fn publish_status(&self, st: &St) {
        let status = match (st.request, st.completed, st.active) {
            // A requested cancel outranks completion: `cancel()` still
            // resolves the job `Ok(())`, so both can be true at once and
            // the caller-visible answer must stay "cancelled".
            (Request::Cancel, _, 0) => JobStatus::Cancelled,
            (Request::Cancel, _, _) => JobStatus::Cancelling,
            (_, true, _) => JobStatus::Completed,
            (Request::Run, _, _) => JobStatus::Running,
            (Request::Pause, _, 0) => JobStatus::Paused,
            (Request::Pause, _, _) => JobStatus::Pausing,
        };
        self.status_tx.send_replace(status);
    }
}

/// A cheap-to-clone handle for pausing, resuming, and cancelling one download
/// job, and for observing its resulting `JobStatus`.
///
/// The caller constructs one (`DownloadControl::new()`), passes a clone into
/// `GogDl::download_files`/`repair_files`, and keeps its own clone to drive
/// and/or `subscribe()` to it. The job's future keeps running to completion
/// in the caller's own task exactly as before — this handle only gates
/// per-chunk progress, it does not change who owns or awaits the future.
#[derive(Clone)]
pub struct DownloadControl {
    inner: Arc<Inner>,
}

impl DownloadControl {
    pub fn new() -> Self {
        let (request_tx, _) = watch::channel(Request::Run);
        let (status_tx, _) = watch::channel(JobStatus::Running);
        Self {
            inner: Arc::new(Inner {
                st: Mutex::new(St {
                    request: Request::Run,
                    active: 0,
                    completed: false,
                }),
                request_tx,
                status_tx,
            }),
        }
    }

    /// Requests a pause: no new chunk will start until `resume()`. Chunks
    /// already past the gate finish and flush normally (drain semantics). A
    /// no-op once the job has been cancelled or has completed.
    pub fn pause(&self) {
        let mut st = self.inner.st.lock().unwrap();
        if st.request == Request::Cancel || st.completed {
            return;
        }
        st.request = Request::Pause;
        // send_replace/send_if_modified never error on zero receivers, unlike
        // `send` — which matters here since receivers come and go between
        // per-stage channels (see `run_stage` in downloader.rs).
        self.inner.request_tx.send_replace(Request::Pause);
        self.inner.publish_status(&st);
    }

    /// Un-pauses the job, letting parked chunks proceed. A no-op once the job
    /// has been cancelled or has completed.
    pub fn resume(&self) {
        let mut st = self.inner.st.lock().unwrap();
        if st.request == Request::Cancel || st.completed {
            return;
        }
        st.request = Request::Run;
        self.inner.request_tx.send_replace(Request::Run);
        self.inner.publish_status(&st);
    }

    /// Requests cancellation: no new chunk will start (parked or not-yet-
    /// gated chunks return without downloading), and the job winds down once
    /// every already-in-flight chunk finishes draining. Terminal — `pause()`/
    /// `resume()` are no-ops afterward. A no-op once the job has completed.
    pub fn cancel(&self) {
        let mut st = self.inner.st.lock().unwrap();
        if st.completed {
            return;
        }
        st.request = Request::Cancel;
        self.inner.request_tx.send_replace(Request::Cancel);
        self.inner.publish_status(&st);
    }

    /// The current status, recomputed from the live request/in-flight state.
    pub fn status(&self) -> JobStatus {
        *self.inner.status_tx.borrow()
    }

    /// True once a pause has been requested (`Pausing` or `Paused`).
    pub fn is_paused(&self) -> bool {
        matches!(self.status(), JobStatus::Pausing | JobStatus::Paused)
    }

    /// True once a cancel has been requested (`Cancelling` or `Cancelled`).
    pub fn is_cancelled(&self) -> bool {
        matches!(self.status(), JobStatus::Cancelling | JobStatus::Cancelled)
    }

    /// A receiver that immediately yields the current status and every
    /// subsequent transition, with no missed edges — the mechanism a
    /// frontend should use to show "Pausing…" and then "Paused".
    pub fn subscribe(&self) -> watch::Receiver<JobStatus> {
        self.inner.status_tx.subscribe()
    }

    /// The pause/cancel checkpoint a chunk future awaits before doing any
    /// I/O. Returns `true` if the chunk should proceed (in which case it has
    /// claimed an "in-flight" slot and **must** eventually drop the returned
    /// guard via `active_guard`), or `false` if the job was cancelled (no
    /// slot claimed, no I/O should happen).
    pub(crate) async fn wait_to_proceed(&self) -> bool {
        let mut rx = self.inner.request_tx.subscribe();
        loop {
            {
                let mut st = self.inner.st.lock().unwrap();
                match st.request {
                    Request::Run => {
                        st.active += 1;
                        return true;
                    }
                    Request::Cancel => return false,
                    Request::Pause => {
                        // Mark the current value seen *while still holding
                        // `st`'s lock*: `request_tx.send_replace` is only ever
                        // called under the same lock (in pause/resume/
                        // cancel), so no transition can happen between this
                        // borrow and the `changed().await` below — no lost
                        // wakeup.
                        rx.borrow_and_update();
                    }
                }
            } // lock dropped before awaiting
            if rx.changed().await.is_err() {
                // All senders gone (the DownloadControl was dropped) — this
                // can't happen while the job holding this same control is
                // still running the gate, but proceed rather than hang if it
                // ever does.
                let mut st = self.inner.st.lock().unwrap();
                st.active += 1;
                return true;
            }
        }
    }

    /// Claims the RAII guard for an in-flight chunk that just passed
    /// `wait_to_proceed`. Dropping it (on any return path — success or
    /// error) decrements the in-flight count and, if it reaches zero, settles
    /// `Pausing -> Paused` or `Cancelling -> Cancelled`.
    pub(crate) fn active_guard(&self) -> ActiveGuard {
        ActiveGuard(self.clone())
    }

    fn finish_unit(&self) {
        let mut st = self.inner.st.lock().unwrap();
        st.active -= 1;
        self.inner.publish_status(&st);
    }

    /// Marks the job as finished, settling the published status to
    /// `Completed` (unless a cancel was requested first — `Cancelled` wins)
    /// so `subscribe()` watchers get a final edge to stop on. Without this,
    /// a successfully finished job would leave the status at `Running`
    /// forever and any status-watching task would park (and leak) on
    /// `changed().await`.
    pub(crate) fn complete(&self) {
        let mut st = self.inner.st.lock().unwrap();
        if st.completed {
            return;
        }
        st.completed = true;
        self.inner.publish_status(&st);
    }

    /// RAII guard the job's engine holds for the lifetime of its future;
    /// dropping it calls `complete()`, so completion is published on every
    /// exit path — success, early error return, or the caller dropping the
    /// job future outright.
    pub(crate) fn completion_guard(&self) -> CompletionGuard {
        CompletionGuard(self.clone())
    }
}

impl Default for DownloadControl {
    fn default() -> Self {
        Self::new()
    }
}

/// RAII guard returned by `DownloadControl::active_guard`; decrements the
/// job's in-flight count on drop.
pub(crate) struct ActiveGuard(DownloadControl);

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.finish_unit();
    }
}

/// RAII guard returned by `DownloadControl::completion_guard`; publishes the
/// job's terminal status on drop.
pub(crate) struct CompletionGuard(DownloadControl);

impl Drop for CompletionGuard {
    fn drop(&mut self) {
        self.0.complete();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pause_with_no_active_chunks_settles_immediately() {
        let control = DownloadControl::new();
        control.pause();
        assert_eq!(control.status(), JobStatus::Paused);
    }

    #[tokio::test]
    async fn pause_with_active_chunk_is_pausing_until_it_finishes() {
        let control = DownloadControl::new();
        assert!(control.wait_to_proceed().await);
        let guard = control.active_guard();

        control.pause();
        assert_eq!(control.status(), JobStatus::Pausing);

        drop(guard);
        assert_eq!(control.status(), JobStatus::Paused);
    }

    #[tokio::test]
    async fn resume_unparks_a_waiting_chunk() {
        let control = DownloadControl::new();
        control.pause();

        let waiter = control.clone();
        let handle = tokio::spawn(async move { waiter.wait_to_proceed().await });

        // Give the spawned task a chance to park in wait_to_proceed().
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;

        control.resume();
        assert!(handle.await.unwrap());
    }

    #[tokio::test]
    async fn cancel_unparks_a_waiting_chunk_with_false() {
        let control = DownloadControl::new();
        control.pause();

        let waiter = control.clone();
        let handle = tokio::spawn(async move { waiter.wait_to_proceed().await });

        tokio::task::yield_now().await;
        tokio::task::yield_now().await;

        control.cancel();
        assert!(!handle.await.unwrap());
        assert_eq!(control.status(), JobStatus::Cancelled);
    }

    #[tokio::test]
    async fn cancel_is_terminal_against_pause_and_resume() {
        let control = DownloadControl::new();
        control.cancel();
        assert_eq!(control.status(), JobStatus::Cancelled);

        control.pause();
        assert_eq!(control.status(), JobStatus::Cancelled);

        control.resume();
        assert_eq!(control.status(), JobStatus::Cancelled);
    }

    #[tokio::test]
    async fn cancel_while_chunk_in_flight_is_cancelling_until_drain() {
        let control = DownloadControl::new();
        assert!(control.wait_to_proceed().await);
        let guard = control.active_guard();

        control.cancel();
        assert_eq!(control.status(), JobStatus::Cancelling);

        // A brand-new chunk trying to start after cancel is rejected
        // immediately and claims no slot.
        assert!(!control.wait_to_proceed().await);

        drop(guard);
        assert_eq!(control.status(), JobStatus::Cancelled);
    }

    #[tokio::test]
    async fn completion_guard_drop_settles_status_to_completed() {
        let control = DownloadControl::new();
        let guard = control.completion_guard();
        assert_eq!(control.status(), JobStatus::Running);

        drop(guard);
        assert_eq!(control.status(), JobStatus::Completed);
    }

    #[tokio::test]
    async fn cancel_then_complete_stays_cancelled() {
        let control = DownloadControl::new();
        let guard = control.completion_guard();

        control.cancel();
        drop(guard);
        assert_eq!(control.status(), JobStatus::Cancelled);
        assert!(control.is_cancelled());
    }

    #[tokio::test]
    async fn complete_while_paused_settles_to_completed() {
        // A zero-chunk job (e.g. repairing an already-complete install) can
        // resolve while a pause request is standing; completion must win so
        // the control doesn't report a finished job as `Paused`.
        let control = DownloadControl::new();
        control.pause();
        assert_eq!(control.status(), JobStatus::Paused);

        control.complete();
        assert_eq!(control.status(), JobStatus::Completed);
        assert!(!control.is_paused());
    }

    #[tokio::test]
    async fn pause_resume_cancel_are_no_ops_after_completion() {
        let control = DownloadControl::new();
        control.complete();

        control.pause();
        assert_eq!(control.status(), JobStatus::Completed);

        control.resume();
        assert_eq!(control.status(), JobStatus::Completed);

        control.cancel();
        assert_eq!(control.status(), JobStatus::Completed);
    }

    #[tokio::test]
    async fn status_watcher_terminates_when_job_completes() {
        // Regression test for the leak this fixes: the api.md §5.3 watcher
        // pattern parked forever on `changed().await` after a successful
        // job because no terminal edge was ever published.
        let control = DownloadControl::new();
        let mut rx = control.subscribe();
        let watcher = tokio::spawn(async move {
            loop {
                let status = *rx.borrow_and_update();
                if matches!(status, JobStatus::Completed | JobStatus::Cancelled) {
                    return status;
                }
                if rx.changed().await.is_err() {
                    return *rx.borrow();
                }
            }
        });

        let guard = control.completion_guard();
        drop(guard);

        let joined = tokio::time::timeout(std::time::Duration::from_secs(1), watcher)
            .await
            .expect("status watcher should terminate once the job completes");
        assert_eq!(joined.unwrap(), JobStatus::Completed);
    }

    #[tokio::test]
    async fn subscribe_observes_pausing_then_paused_with_no_missed_edges() {
        let control = DownloadControl::new();
        assert!(control.wait_to_proceed().await);
        let guard = control.active_guard();

        let mut rx = control.subscribe();
        assert_eq!(*rx.borrow_and_update(), JobStatus::Running);

        control.pause();
        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow_and_update(), JobStatus::Pausing);

        drop(guard);
        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow_and_update(), JobStatus::Paused);

        control.resume();
        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow_and_update(), JobStatus::Running);
    }
}
