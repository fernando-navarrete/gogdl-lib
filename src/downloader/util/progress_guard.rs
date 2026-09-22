use tokio::sync::mpsc;

use crate::downloader::DownloadEvent;

/// Tracks the bytes one chunk-download attempt has reported via
/// [`DownloadEvent::Progress`] and takes them back with
/// [`DownloadEvent::ProgressRegression`] unless the attempt reaches
/// [`commit`](Self::commit) — including when the future is dropped
/// mid-transfer because another unit aborted the batch. This makes "every
/// exit cancels what it reported" hold unconditionally, without a manual
/// `ProgressRegression` send at every early-return site.
pub struct ProgressGuard {
    tx: mpsc::UnboundedSender<DownloadEvent>,
    reported_bytes: usize,
}

impl ProgressGuard {
    /// Starts a guard with nothing reported yet.
    pub fn new(tx: mpsc::UnboundedSender<DownloadEvent>) -> Self {
        Self {
            tx,
            reported_bytes: 0,
        }
    }

    /// Sends `Progress(len)` and remembers `len` as bytes this guard would
    /// take back if the attempt doesn't reach `commit()`.
    pub fn report(&mut self, len: usize) {
        self.reported_bytes += len;
        self.tx.send(DownloadEvent::Progress(len)).ok();
    }

    /// The attempt succeeded — keep the reported bytes and disarm the
    /// take-back.
    pub fn commit(mut self) {
        self.reported_bytes = 0;
        // Drop runs after this, but with reported_bytes at 0 it is a no-op.
    }
}

impl Drop for ProgressGuard {
    fn drop(&mut self) {
        if self.reported_bytes > 0 {
            self.tx
                .send(DownloadEvent::ProgressRegression(self.reported_bytes))
                .ok();
        }
    }
}
