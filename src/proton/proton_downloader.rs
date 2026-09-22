use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use bytes::Bytes;
use flate2::read::GzDecoder;
use tar::Archive;
use tokio::{
    io::AsyncWriteExt,
    sync::{Mutex, mpsc},
};
use tokio_util::io::SyncIoBridge;

use crate::{
    ProtonError, ProtonGeRelease,
    client::{ClientError, HttpClient},
    fs::{PathResolver, sanitize_filename},
    proton::ProtonDownloadEvent,
};

/// Tracks the single top-level path component shared by every unpacked tar
/// entry, so the extracted tree can be found and renamed afterwards without
/// trusting the asset filename. `None` once entries disagree on it.
#[derive(Default)]
struct ExtractionRoot(Option<Option<OsString>>);

impl ExtractionRoot {
    fn observe(&mut self, entry_path: &Path) {
        let component = match entry_path.components().next() {
            Some(Component::Normal(part)) => Some(part.to_os_string()),
            _ => None,
        };
        match &self.0 {
            None => self.0 = Some(component),
            Some(existing) if *existing != component => self.0 = Some(None),
            Some(_) => {}
        }
    }

    /// The shared top-level component, or `None` if no entries were unpacked
    /// or they didn't all agree on one.
    fn into_inner(self) -> Option<OsString> {
        self.0.flatten()
    }
}

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
    /// read.
    ///
    /// Once extraction finishes, the archive's top-level directory (whatever
    /// it was actually named inside the tarball) is renamed to
    /// `release.tag_name`, sanitized for the filesystem — some Proton-GE
    /// releases ship theirs with an architecture suffix (e.g.
    /// `GE-Proton10-4-x86_64` for tag `GE-Proton10-4`), which breaks
    /// frontends that expect the directory name to match the release tag.
    /// Returns `path/<tag_name>`. If a directory with that name already
    /// exists, it's removed first, so a re-download always leaves a single,
    /// clean tree behind.
    ///
    /// [`ProtonDownloadEvent::Extracted`] events fire during extraction and
    /// report paths as they appear inside the archive, i.e. *before* this
    /// rename.
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
        let asset = release.get_suitable_asset()?;

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
            let writer_slot = Mutex::new(writer);
            let writer_ref = &writer_slot;
            let result = self
                .client
                .stream_chunk(&asset.browser_download_url, |chunk: Bytes| {
                    tx.send(ProtonDownloadEvent::Progress(chunk.len())).ok();
                    let writer = writer_ref;
                    Box::pin(async move { writer.lock().await.write_all(&chunk).await })
                })
                .await;
            // Always shut down the write half so extraction sees a clean
            // EOF rather than hanging, whether the transfer succeeded or
            // failed partway.
            let mut writer = writer_slot.into_inner();
            let _ = writer.shutdown().await;
            result
        };

        let dest_for_extraction = dest.clone();
        let tx_extract = tx.clone();
        let extract_future =
            tokio::task::spawn_blocking(move || -> std::io::Result<Option<OsString>> {
                let sync_reader = SyncIoBridge::new(reader);
                let gz = GzDecoder::new(sync_reader);
                let mut archive = Archive::new(gz);
                let mut root = ExtractionRoot::default();
                for entry in archive.entries()? {
                    let mut entry = entry?;
                    let entry_path = entry.path()?.into_owned();
                    if entry.unpack_in(&dest_for_extraction)? {
                        root.observe(&entry_path);
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
                Ok(root.into_inner())
            });

        let (download_result, extract_result) = tokio::join!(download_future, extract_future);

        let extract_result: Result<Option<OsString>, ProtonError> = match extract_result {
            Ok(inner) => inner.map_err(ProtonError::Io),
            Err(join_err) => Err(ProtonError::ExtractionError(join_err.to_string())),
        };

        let root_component = match download_result {
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

        let root_component = root_component.ok_or_else(|| {
            ProtonError::ExtractionError("archive has no single top-level directory".to_string())
        })?;

        let safe_tag = sanitize_filename(&release.tag_name);
        if safe_tag.is_empty() {
            return Err(ProtonError::ExtractionError(format!(
                "release tag {:?} sanitizes to an empty directory name",
                release.tag_name
            )));
        }

        let source = dest.join(&root_component);
        let target = dest.join(&safe_tag);

        if source != target {
            if tokio::fs::try_exists(&target).await? {
                tokio::fs::remove_dir_all(&target).await?;
            }
            tokio::fs::rename(&source, &target).await?;
        }

        Ok(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_entries_observed_has_no_root() {
        let root = ExtractionRoot::default();
        assert_eq!(root.into_inner(), None);
    }

    #[test]
    fn single_top_level_directory_is_the_root() {
        let mut root = ExtractionRoot::default();
        root.observe(Path::new("GE-Proton10-4/"));
        root.observe(Path::new("GE-Proton10-4/bin/wine"));
        root.observe(Path::new("GE-Proton10-4/lib/libwine.so"));
        assert_eq!(root.into_inner(), Some(OsString::from("GE-Proton10-4")));
    }

    #[test]
    fn architecture_suffixed_root_is_still_detected() {
        let mut root = ExtractionRoot::default();
        root.observe(Path::new("GE-Proton10-4-x86_64/"));
        root.observe(Path::new("GE-Proton10-4-x86_64/bin/wine"));
        assert_eq!(
            root.into_inner(),
            Some(OsString::from("GE-Proton10-4-x86_64"))
        );
    }

    #[test]
    fn disagreeing_top_level_components_have_no_root() {
        let mut root = ExtractionRoot::default();
        root.observe(Path::new("GE-Proton10-4/bin/wine"));
        root.observe(Path::new("something-else/README"));
        assert_eq!(root.into_inner(), None);
    }

    #[test]
    fn a_root_level_file_counts_as_its_own_component() {
        // A single top-level file (no shared directory) is unusual for a
        // Proton-GE tarball, but should still resolve to that one name
        // rather than silently becoming ambiguous.
        let mut root = ExtractionRoot::default();
        root.observe(Path::new("only-file.txt"));
        assert_eq!(root.into_inner(), Some(OsString::from("only-file.txt")));
    }
}
