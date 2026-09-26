use std::ffi::{OsStr, OsString};
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

/// Prefix of the hidden directory a release is extracted into before it
/// replaces `path/<tag>`: `.gogdl-staging-<tag>-<random>`.
const STAGING_PREFIX: &str = ".gogdl-staging-";

/// A staging directory (`extract/` for the unpacked archive, `previous/` for
/// the install being replaced), removed on drop.
///
/// It is moved into the extraction task, so the task that writes into it is
/// also the one that removes it, after it has stopped writing. The async side
/// never deletes it while extraction could still be running.
struct StagingDir {
    path: PathBuf,
}

impl StagingDir {
    fn create(dest: &Path, safe_tag: &str) -> std::io::Result<Self> {
        let path = dest.join(format!(
            "{STAGING_PREFIX}{safe_tag}-{:016x}",
            rand::random::<u64>()
        ));
        std::fs::create_dir(&path)?;
        let staging = Self { path };
        std::fs::create_dir(staging.extract_dir())?;
        Ok(staging)
    }

    fn extract_dir(&self) -> PathBuf {
        self.path.join("extract")
    }
}

impl Drop for StagingDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}

/// Removes staging directories of `safe_tag` left by a process that died
/// before its guard ran. Other tags' are left alone: they may be a download
/// in progress.
async fn remove_stale_staging(dest: &Path, safe_tag: &str) -> std::io::Result<()> {
    let prefix = format!("{STAGING_PREFIX}{safe_tag}-");
    let mut entries = tokio::fs::read_dir(dest).await?;
    while let Some(entry) = entries.next_entry().await? {
        if entry.file_name().to_string_lossy().starts_with(&prefix) {
            tokio::fs::remove_dir_all(entry.path()).await?;
        }
    }
    Ok(())
}

/// Moves the extracted `root` onto `target`, replacing what was there. The
/// old install is moved aside first and only deleted (with the staging
/// directory) once the new one is in place; if the second rename fails, the
/// old one is put back.
///
/// Runs in one blocking task with no `.await` in it, so dropping the caller's
/// future cannot leave it half-done.
async fn swap_into_place(
    staging: StagingDir,
    root: OsString,
    target: PathBuf,
) -> std::io::Result<()> {
    tokio::task::spawn_blocking(move || swap(&staging, &root, &target))
        .await
        .map_err(std::io::Error::other)?
}

fn swap(staging: &StagingDir, root: &OsStr, target: &Path) -> std::io::Result<()> {
    let previous = staging.path.join("previous");
    let had_previous = target.try_exists()?;
    if had_previous {
        std::fs::rename(target, &previous)?;
    }
    if let Err(err) = std::fs::rename(staging.extract_dir().join(root), target) {
        if had_previous {
            std::fs::rename(&previous, target).ok();
        }
        return Err(err);
    }
    Ok(())
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
    /// `path` is the *parent* directory the release is installed into; it's
    /// created (and canonicalized) if missing. The transfer is gated on the
    /// destination disk having at least the tarball's compressed size free
    /// before a single byte is read (the asset carries no extracted size). A
    /// re-download keeps the old tree on disk until the swap below.
    ///
    /// The archive is extracted into a hidden staging directory,
    /// `path/.gogdl-staging-<tag>-<random>`. Once extraction finishes, the
    /// archive's top-level directory (whatever it was actually named inside
    /// the tarball) replaces `path/<tag_name>`, sanitized for the filesystem:
    /// some Proton-GE releases ship theirs with an architecture suffix (e.g.
    /// `GE-Proton10-4-x86_64` for tag `GE-Proton10-4`), which breaks
    /// frontends that expect the directory name to match the release tag. An
    /// existing `path/<tag_name>` is replaced wholesale, so a re-download
    /// never leaves files of the old tree behind. Returns `path/<tag_name>`.
    ///
    /// On failure, or if the future is dropped, `path` is left as it was: the
    /// extraction task removes the staging directory itself once it has
    /// stopped writing, shortly after a drop. Once the swap has started it
    /// completes. Staging directories of the same tag left by a crashed
    /// process are removed at the start of the next call, so don't run two
    /// downloads of the same tag into the same `path` at once.
    ///
    /// [`ProtonDownloadEvent::Extracted`] events fire during extraction and
    /// report paths as they appear inside the archive, i.e. *before* the
    /// swap.
    ///
    /// Not resumable: there is no retry here.
    pub async fn download_proton_release(
        &self,
        release: &ProtonGeRelease,
        path: &Path,
        tx: mpsc::UnboundedSender<ProtonDownloadEvent>,
    ) -> Result<PathBuf, ProtonError> {
        let asset = release.get_suitable_asset()?;

        let safe_tag = sanitize_filename(&release.tag_name);
        if safe_tag.is_empty() {
            return Err(ProtonError::ExtractionError(format!(
                "release tag {:?} sanitizes to an empty directory name",
                release.tag_name
            )));
        }

        let path_resolver = PathResolver::new(path.to_path_buf()).await?;

        path_resolver.check_free_space(asset.size)?;

        let dest = path_resolver.base().to_path_buf();

        remove_stale_staging(&dest, &safe_tag).await?;
        let staging = StagingDir::create(&dest, &safe_tag)?;

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

        let extract_dir = staging.extract_dir();
        let tx_extract = tx.clone();
        let extract_future = tokio::task::spawn_blocking(
            move || -> std::io::Result<(StagingDir, Option<OsString>)> {
                let sync_reader = SyncIoBridge::new(reader);
                let gz = GzDecoder::new(sync_reader);
                let mut archive = Archive::new(gz);
                let mut root = ExtractionRoot::default();
                for entry in archive.entries()? {
                    let mut entry = entry?;
                    let entry_path = entry.path()?.into_owned();
                    if entry.unpack_in(&extract_dir)? {
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
                Ok((staging, root.into_inner()))
            },
        );

        let (download_result, extract_result) = tokio::join!(download_future, extract_future);

        let extract_result: Result<(StagingDir, Option<OsString>), ProtonError> =
            match extract_result {
                Ok(inner) => inner.map_err(ProtonError::Io),
                Err(join_err) => Err(ProtonError::ExtractionError(join_err.to_string())),
            };

        let (staging, root_component) = match download_result {
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

        let target = dest.join(&safe_tag);
        swap_into_place(staging, root_component, target.clone()).await?;

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

    use std::{sync::Arc, time::Duration};

    use tokio::sync::Notify;

    use crate::test_support::{ChunkServer, Reply, TempDir};

    const TARBALL: &str = "/GE-Proton10-4.tar.gz";

    /// A gzipped tarball holding `files` under `root/`. Uncompressed deflate,
    /// so a truncated body still yields the entries before the cut.
    fn tarball(root: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (name, data) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, format!("{root}/{name}"), *data)
                .unwrap();
        }
        let tar = builder.into_inner().unwrap();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::none());
        std::io::Write::write_all(&mut encoder, &tar).unwrap();
        encoder.finish().unwrap()
    }

    fn release(server: &ChunkServer, size: usize) -> ProtonGeRelease {
        serde_json::from_value(serde_json::json!({
            "url": "https://api.github.com/repos/o/r/releases/1",
            "tag_name": "GE-Proton10-4",
            "id": 1,
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "published_at": "2026-01-01T00:00:00Z",
            "assets": [{
                "name": "GE-Proton10-4.tar.gz",
                "browser_download_url": format!("{}{TARBALL}", server.base_url()),
                "size": size,
            }],
        }))
        .unwrap()
    }

    fn downloader() -> ProtonDownloader {
        ProtonDownloader::new(HttpClient::new_with_client(reqwest::Client::new()))
    }

    fn staging_dirs(path: &Path) -> Vec<String> {
        std::fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(STAGING_PREFIX))
            .collect()
    }

    fn seed_previous(dir: &TempDir) {
        let old = dir.path().join("GE-Proton10-4");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("stale.txt"), "old").unwrap();
    }

    fn previous_intact(dir: &TempDir) -> bool {
        std::fs::read_to_string(dir.path().join("GE-Proton10-4/stale.txt"))
            .is_ok_and(|s| s == "old")
    }

    #[tokio::test]
    async fn a_clean_install_lands_at_the_tag() {
        let server = ChunkServer::start().await;
        let body = tarball(
            "GE-Proton10-4-x86_64",
            &[("bin/wine", b"wine"), ("VERSION", b"1")],
        );
        server.script(TARBALL, vec![Reply::Body(body.clone())]);
        let dir = TempDir::new();
        let (tx, mut rx) = mpsc::unbounded_channel();

        let installed = downloader()
            .download_proton_release(&release(&server, body.len()), dir.path(), tx)
            .await
            .unwrap();

        let target = dir.path().canonicalize().unwrap().join("GE-Proton10-4");
        assert_eq!(installed, target);
        assert_eq!(std::fs::read(target.join("bin/wine")).unwrap(), b"wine");
        assert!(staging_dirs(dir.path()).is_empty());
        let mut extracted = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let ProtonDownloadEvent::Extracted(name) = event {
                extracted.push(name);
            }
        }
        assert!(
            extracted
                .iter()
                .all(|name| name.starts_with("GE-Proton10-4-x86_64/"))
        );
        assert!(!extracted.is_empty());
    }

    #[tokio::test]
    async fn a_redownload_replaces_the_old_tree() {
        let server = ChunkServer::start().await;
        let body = tarball("GE-Proton10-4", &[("VERSION", b"2")]);
        server.script(TARBALL, vec![Reply::Body(body.clone())]);
        let dir = TempDir::new();
        seed_previous(&dir);
        let (tx, _rx) = mpsc::unbounded_channel();

        downloader()
            .download_proton_release(&release(&server, body.len()), dir.path(), tx)
            .await
            .unwrap();

        let target = dir.path().join("GE-Proton10-4");
        assert!(!target.join("stale.txt").exists());
        assert_eq!(std::fs::read(target.join("VERSION")).unwrap(), b"2");
        assert!(staging_dirs(dir.path()).is_empty());
    }

    #[tokio::test]
    async fn a_truncated_stream_keeps_the_previous_install() {
        let server = ChunkServer::start().await;
        let big = vec![7u8; 256 * 1024];
        let body = tarball("GE-Proton10-4", &[("big", &big), ("VERSION", b"2")]);
        let sent = body.len() / 2;
        server.script(TARBALL, vec![Reply::Body(body[..sent].to_vec())]);
        let dir = TempDir::new();
        seed_previous(&dir);
        let (tx, _rx) = mpsc::unbounded_channel();

        let err = downloader()
            .download_proton_release(&release(&server, body.len()), dir.path(), tx)
            .await
            .unwrap_err();

        assert!(matches!(err, ProtonError::Io(_)), "{err}");
        assert!(previous_intact(&dir));
        assert!(staging_dirs(dir.path()).is_empty());
    }

    #[tokio::test]
    async fn a_failing_extraction_is_reported_over_the_broken_pipe() {
        // Larger than the pipe, so the download side really does hit a broken
        // pipe once extraction gives up on the first bytes.
        let server = ChunkServer::start().await;
        let body = vec![0x42u8; 2 * 1024 * 1024];
        server.script(TARBALL, vec![Reply::Body(body.clone())]);
        let dir = TempDir::new();
        seed_previous(&dir);
        let (tx, _rx) = mpsc::unbounded_channel();

        let err = downloader()
            .download_proton_release(&release(&server, body.len()), dir.path(), tx)
            .await
            .unwrap_err();

        assert!(matches!(err, ProtonError::Io(_)), "{err}");
        assert!(previous_intact(&dir));
        assert!(staging_dirs(dir.path()).is_empty());
    }

    #[tokio::test]
    async fn a_dropped_download_leaves_no_staging_dir() {
        let server = ChunkServer::start().await;
        let big = vec![7u8; 256 * 1024];
        let body = tarball("GE-Proton10-4", &[("a", &big), ("b", &big)]);
        let notify = Arc::new(Notify::new());
        server.script(
            TARBALL,
            vec![Reply::PartialThenHang {
                body: body.clone(),
                sent: body.len() / 2,
                notify: notify.clone(),
            }],
        );
        let dir = TempDir::new();
        seed_previous(&dir);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let release = release(&server, body.len());
        let downloader = downloader();

        let mut call = Box::pin(downloader.download_proton_release(&release, dir.path(), tx));
        tokio::select! {
            _ = &mut call => panic!("finished against a hanging server"),
            _ = notify.notified() => {}
        }
        // Let extraction start writing into the staging directory.
        loop {
            tokio::select! {
                _ = &mut call => panic!("finished against a hanging server"),
                event = rx.recv() => {
                    if matches!(event, Some(ProtonDownloadEvent::Extracted(_))) {
                        break;
                    }
                }
            }
        }
        drop(call);

        for _ in 0..100 {
            if staging_dirs(dir.path()).is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(staging_dirs(dir.path()).is_empty());
        assert!(previous_intact(&dir));
    }

    #[tokio::test]
    async fn stale_staging_dirs_of_the_tag_are_swept() {
        let server = ChunkServer::start().await;
        let body = tarball("GE-Proton10-4", &[("VERSION", b"2")]);
        server.script(TARBALL, vec![Reply::Body(body.clone())]);
        let dir = TempDir::new();
        let stale = dir.path().join(".gogdl-staging-GE-Proton10-4-dead");
        let other = dir.path().join(".gogdl-staging-GE-Proton9-1-live");
        for d in [&stale, &other] {
            std::fs::create_dir_all(d).unwrap();
            std::fs::write(d.join("x"), "x").unwrap();
        }
        let (tx, _rx) = mpsc::unbounded_channel();

        downloader()
            .download_proton_release(&release(&server, body.len()), dir.path(), tx)
            .await
            .unwrap();

        assert!(!stale.exists());
        assert!(other.exists());
    }
}
