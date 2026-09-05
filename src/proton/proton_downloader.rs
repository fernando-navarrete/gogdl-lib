use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use tar::Archive;
use tokio::{io::AsyncWriteExt, sync::mpsc};
use tokio_util::io::SyncIoBridge;

use crate::{
    ProtonError, ProtonGeRelease,
    client::{ClientError, HttpClient},
    fs::PathResolver,
    proton::ProtonDownloadEvent,
};

pub struct ProtonDownloader {
    pub client: HttpClient,
}

/// True if `err` is the kind of write failure a `tokio::io::duplex` writer
/// sees when its paired reader was dropped — i.e. a symptom of the
/// extraction side closing first, not a real network/disk problem on the
/// download side.
fn is_pipe_closed_by_reader(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::WriteZero
    )
}

impl ProtonDownloader {
    pub fn new(client: HttpClient) -> Self {
        Self { client }
    }

    /// Downloads `release`'s Linux x86_64 tarball and extracts it into
    /// `path`, never writing the compressed archive itself to disk: the
    /// network response is piped straight into a gzip decoder and tar
    /// extractor as it arrives, bounded by a fixed-size in-memory buffer.
    ///
    /// `path` is the *parent* directory the tarball's own top-level
    /// directory is extracted into; it's created (and canonicalized) if
    /// missing. The transfer is gated on the destination disk having at
    /// least the tarball's compressed size free before a single byte is
    /// read. Returns the path to that extracted top-level directory.
    ///
    /// Not resumable: there is no retry here, so a failure partway through
    /// leaves a partial tree under `path` that the caller is responsible for
    /// removing before trying again.
    pub async fn download_proton_release(
        &self,
        release: &ProtonGeRelease,
        path: &Path,
        tx: mpsc::UnboundedSender<ProtonDownloadEvent>,
    ) -> Result<PathBuf, ProtonError> {
        let asset = match release
            .assets
            .iter()
            .find(|asset| asset.name.ends_with(".tar.gz") && !asset.name.contains("aarch64"))
        {
            Some(asset) => asset,
            None => return Err(ProtonError::NoSuitableAsset(release.tag_name.clone())),
        };

        let path_resolver = PathResolver::new(path.to_path_buf()).await?;

        let available_space = match path_resolver.get_free_space() {
            Ok(space) => space,
            Err(_) => return Err(ProtonError::CouldNotResolveFreeSpace),
        };
        if asset.size > available_space {
            return Err(ProtonError::NotEnoughFreeSpace);
        }

        let dest = path_resolver.base().to_path_buf();

        tx.send(ProtonDownloadEvent::Downloading {
            total_bytes: asset.size,
        })
        .ok();

        // A bounded pipe: the network write side blocks once it's 1 MiB
        // ahead of what extraction has consumed, so memory use stays flat
        // regardless of tarball size.
        let (writer, reader) = tokio::io::duplex(1024 * 1024);

        let download_future = async {
            let mut writer = writer;
            let result = self
                .client
                .stream_chunk(&asset.browser_download_url, async |chunk| {
                    tx.send(ProtonDownloadEvent::Progress(chunk.len())).ok();
                    writer.write_all(&chunk).await
                })
                .await;
            // Always shut down the write half so extraction sees a clean
            // EOF rather than hanging, whether the transfer succeeded or
            // failed partway.
            let _ = writer.shutdown().await;
            result
        };

        let dest_for_extraction = dest.clone();
        let tx_extract = tx.clone();
        let extract_future = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            let sync_reader = SyncIoBridge::new(reader);
            let gz = GzDecoder::new(sync_reader);
            let mut archive = Archive::new(gz);
            for entry in archive.entries()? {
                let mut entry = entry?;
                let entry_path = entry.path()?.into_owned();
                if entry.unpack_in(&dest_for_extraction)? {
                    tx_extract
                        .send(ProtonDownloadEvent::Extracted(
                            entry_path.to_string_lossy().into_owned(),
                        ))
                        .ok();
                }
            }
            // Drain whatever's left (trailing tar padding, the gzip
            // footer) so the writer above always finishes with a clean
            // EOF instead of a spurious broken-pipe once we drop the
            // reader here.
            let mut sync_reader = archive.into_inner().into_inner();
            std::io::copy(&mut sync_reader, &mut std::io::sink())?;
            Ok(())
        });

        let (download_result, extract_result) = tokio::join!(download_future, extract_future);

        let extract_result: Result<(), ProtonError> = match extract_result {
            Ok(inner) => inner.map_err(ProtonError::Io),
            Err(join_err) => Err(ProtonError::ExtractionError(join_err.to_string())),
        };

        match download_result {
            Ok(()) => extract_result,
            Err(ClientError::ChunkStreamCallbackError(io_err))
                if is_pipe_closed_by_reader(&io_err) && extract_result.is_err() =>
            {
                // Extraction failed (or bailed) first and closed its end of
                // the pipe; that failure is the real cause, this is just
                // the downstream symptom.
                extract_result
            }
            Err(ClientError::ChunkStreamCallbackError(io_err)) => Err(ProtonError::Io(io_err)),
            Err(err) => Err(ProtonError::ClientError(err)),
        }?;

        let extracted_name = asset.name.strip_suffix(".tar.gz").unwrap_or(&asset.name);
        Ok(dest.join(extracted_name))
    }
}
