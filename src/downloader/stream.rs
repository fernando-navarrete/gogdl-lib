//! The low-level byte-streaming transport helper used by the download
//! stage: pulls decoded chunk bytes off `HttpClient::fetch_chunk_stream`
//! and writes them into an already-open file at the right offset.

use std::io::SeekFrom;

use tokio::{
    fs,
    io::{AsyncSeekExt, AsyncWriteExt},
    sync::mpsc,
};

use crate::{
    client::{ClientError, HttpClient},
    downloader::events::DownloadEvent,
};

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
/// `counted` is a per-unit high-water-mark of how many decoded bytes have
/// already been credited via `Progress` for this chunk, shared across a
/// primary attempt and a possible redist-fallback retry: if a first attempt
/// streams `w` bytes before failing, the retry only emits `Progress` for
/// bytes beyond `w`, so the two attempts together credit exactly the chunk's
/// size once, never double-counting the re-streamed prefix.
pub(crate) async fn stream_unit_to_file(
    client: &HttpClient,
    url: &str,
    access_token: String,
    file: &mut fs::File,
    offset: u64,
    counted: &mut u64,
    tx: &mpsc::UnboundedSender<DownloadEvent>,
) -> Result<(), UnitAttemptError> {
    file.seek(SeekFrom::Start(offset))
        .await
        .map_err(UnitAttemptError::Write)?;

    let mut rx = client.fetch_chunk_stream(url, access_token);
    let mut pos: u64 = 0;

    loop {
        match rx.recv().await {
            Some(Ok(bytes)) => {
                file.write_all(&bytes).await.map_err(UnitAttemptError::Write)?;
                pos += bytes.len() as u64;
                if pos > *counted {
                    tx.send(DownloadEvent::Progress {
                        bytes: pos - *counted,
                    })
                    .ok();
                    *counted = pos;
                }
            }
            Some(Err(err)) => return Err(UnitAttemptError::Download(err)),
            None => break,
        }
    }
    file.flush().await.ok();
    Ok(())
}
