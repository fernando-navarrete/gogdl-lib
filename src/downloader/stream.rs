//! The low-level byte-streaming transport helper used by the download
//! stage: pulls decoded chunk bytes off `HttpClient::fetch_chunk_stream`
//! and writes them into an already-open file at the right offset.

use std::io::SeekFrom;
use std::time::Duration;

use tokio::{
    fs,
    io::{AsyncSeekExt, AsyncWriteExt},
    sync::mpsc,
};

use crate::{
    client::{ClientError, HttpClient},
    downloader::{adaptive::ThroughputMeter, events::DownloadEvent},
};

/// Decoded bytes are buffered up to this size before being flushed to disk
/// as one `write_all`, instead of issuing one (often small) write per
/// network-level read. Each write crosses into tokio's blocking-file
/// threadpool, so batching cuts down on both syscall count and async/blocking
/// hops under high chunk concurrency.
const WRITE_BUFFER_THRESHOLD: usize = 256 * 1024;

/// The outcome of a single fetch-and-write attempt for one chunk, made by
/// `stream_unit_to_file`. Distinguishes a CDN/stream failure (worth retrying
/// against the redist store) from a local disk failure (not worth retrying,
/// since the same disk is used for any fallback attempt too).
pub(crate) enum UnitAttemptError {
    Download(ClientError),
    Write(std::io::Error),
}

impl std::fmt::Display for UnitAttemptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UnitAttemptError::Download(e) => write!(f, "{e}"),
            UnitAttemptError::Write(e) => write!(f, "{e}"),
        }
    }
}

/// Streams `url`'s (decoded) bytes into `file` starting at `offset`, emitting
/// a `DownloadEvent::Progress` for each byte range not yet credited.
///
/// `Progress` is emitted at the same cadence as disk writes -- i.e. every
/// `WRITE_BUFFER_THRESHOLD`, not every network-level read -- rather than
/// once per (often small) decoded chunk of a stream that can complete
/// thousands of reads per second across many concurrent chunks. A consumer
/// wiring `Progress` straight into a per-event UI/IPC push (as
/// `gogdl_flutter`'s drain loops do) sees an event rate this way that's
/// bounded by throughput / `WRITE_BUFFER_THRESHOLD` instead of by read
/// syscall size, which is what let a fast connection flood an unbounded
/// downstream channel faster than it could be drained.
///
/// `counted` is a per-unit high-water-mark of how many decoded bytes have
/// already been credited via `Progress` for this chunk, shared across a
/// primary attempt and a possible redist-fallback retry: if a first attempt
/// streams `w` bytes before failing, the retry only emits `Progress` for
/// bytes beyond `w`, so the two attempts together credit exactly the chunk's
/// size once, never double-counting the re-streamed prefix. This still holds
/// with `Progress` moved to the flush boundary: a failed attempt's
/// not-yet-flushed tail is simply re-decoded (and then credited) by the
/// retry, since both attempts always re-seek to the same `offset`.
///
/// `meter` is credited with every decoded byte as it's read (not just at the
/// flush boundary), so the adaptive concurrency controller's throughput
/// measurement (see `downloader::adaptive`) is unaffected by this batching.
/// `response_timeout`/`idle_timeout` are forwarded to
/// `HttpClient::fetch_chunk_stream` to bound how long a stalled CDN
/// connection is tolerated before it's reported as a (retryable) error.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn stream_unit_to_file(
    client: &HttpClient,
    url: &str,
    access_token: String,
    file: &mut fs::File,
    offset: u64,
    counted: &mut u64,
    tx: &mpsc::UnboundedSender<DownloadEvent>,
    meter: &ThroughputMeter,
    response_timeout: Duration,
    idle_timeout: Duration,
) -> Result<(), UnitAttemptError> {
    file.seek(SeekFrom::Start(offset))
        .await
        .map_err(UnitAttemptError::Write)?;

    let mut rx = client.fetch_chunk_stream(url, access_token, response_timeout, idle_timeout);
    let mut pos: u64 = 0;
    let mut write_buffer: Vec<u8> = Vec::with_capacity(WRITE_BUFFER_THRESHOLD);

    // Credits `pos - *counted` bytes via `Progress`, if any are outstanding,
    // and advances `*counted` to `pos`. Called at every point the buffer is
    // written out, so `Progress` tracks disk writes rather than reads.
    let credit_progress = |pos: u64, counted: &mut u64| {
        if pos > *counted {
            tx.send(DownloadEvent::Progress {
                bytes: pos - *counted,
            })
            .ok();
            *counted = pos;
        }
    };

    loop {
        match rx.recv().await {
            Some(Ok(bytes)) => {
                write_buffer.extend_from_slice(&bytes);
                pos += bytes.len() as u64;
                meter.add_bytes(bytes.len() as u64);
                if write_buffer.len() >= WRITE_BUFFER_THRESHOLD {
                    file.write_all(&write_buffer)
                        .await
                        .map_err(UnitAttemptError::Write)?;
                    write_buffer.clear();
                    credit_progress(pos, counted);
                }
            }
            Some(Err(err)) => {
                // Flush whatever was already decoded before surfacing the
                // error: a retry resumes at `offset`, i.e. rewrites this
                // range, but losing an already-decoded, not-yet-written tail
                // here would still leave stale bytes on disk if the retry
                // itself fails before reaching this far.
                if !write_buffer.is_empty() {
                    file.write_all(&write_buffer).await.ok();
                    credit_progress(pos, counted);
                }
                return Err(UnitAttemptError::Download(err));
            }
            None => break,
        }
    }
    if !write_buffer.is_empty() {
        file.write_all(&write_buffer)
            .await
            .map_err(UnitAttemptError::Write)?;
    }
    credit_progress(pos, counted);
    file.flush().await.ok();
    Ok(())
}
