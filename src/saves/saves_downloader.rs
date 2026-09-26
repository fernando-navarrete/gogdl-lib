use std::{
    io::{Read, Write},
    path::PathBuf,
    time::SystemTime,
};

use chrono::DateTime;
use flate2::read::GzDecoder;
use tokio::sync::{OnceCell, mpsc};

use crate::{
    SavesError,
    client::HttpClient,
    constants::CLOUD_STORAGE_URL,
    fs::PathResolver,
    saves::{
        SaveFile, checksum::md5_hex, save_location::ResolvedSaveLocation, saves_auth::SavesAuth,
        saves_download_event::SavesDownloadEvent,
    },
};

/// A save location's directory, ready to receive files.
struct Target {
    name: String,
    path: PathBuf,
    /// Created on first use, so a location none of the files belong to is
    /// not created on disk.
    resolver: OnceCell<PathResolver>,
}

impl Target {
    async fn resolver(&self) -> Result<&PathResolver, SavesError> {
        Ok(self
            .resolver
            .get_or_try_init(|| PathResolver::new(self.path.clone()))
            .await?)
    }
}

pub struct SavesDownloader {
    pub client: HttpClient,
    pub auth: SavesAuth,
    pub client_id: String,
    /// Where objects live; [`CLOUD_STORAGE_URL`] outside tests.
    pub storage_url: String,
}

impl SavesDownloader {
    pub fn new(client: HttpClient, auth: SavesAuth, client_id: String) -> Self {
        Self {
            client,
            auth,
            client_id,
            storage_url: CLOUD_STORAGE_URL.to_string(),
        }
    }

    /// Downloads `files` one after another, each into the directory of the
    /// one of `locations` it belongs to, at [`SaveFile::relative_path_in`]
    /// beneath it. A file that belongs to none of them goes into the first
    /// location. A location's directory is created when the first file for it
    /// arrives, and `PathResolver` guarantees nothing is written outside it.
    ///
    /// Each file is received whole, checked against the `ETag` GOG sends for
    /// it, gunzipped, written, and given the modification time GOG stored
    /// with it. It is written to a temporary file beside its destination and
    /// renamed over it, so an existing save is replaced only once the new one
    /// is on disk. Not retried: the first failure aborts the call, leaving
    /// the files before it in place.
    ///
    /// # Errors
    /// [`SavesError::CloudStorageNotSupported`] if `locations` is empty, plus
    /// what downloading a file can fail with.
    pub async fn download_files(
        &self,
        files: &[SaveFile],
        locations: &[ResolvedSaveLocation],
        tx: mpsc::UnboundedSender<SavesDownloadEvent>,
    ) -> Result<(), SavesError> {
        let targets: Vec<Target> = locations
            .iter()
            .map(|location| Target {
                name: location.name.clone(),
                path: location.path.clone(),
                resolver: OnceCell::new(),
            })
            .collect();
        if targets.is_empty() {
            return Err(SavesError::CloudStorageNotSupported);
        }

        tx.send(SavesDownloadEvent::Preparing {
            total_files: files.len(),
            total_bytes: files.iter().map(|file| file.bytes).sum(),
        })
        .ok();

        for file in files {
            self.download_file(file, &targets, &tx).await?;
        }
        Ok(())
    }

    async fn download_file(
        &self,
        file: &SaveFile,
        targets: &[Target],
        tx: &mpsc::UnboundedSender<SavesDownloadEvent>,
    ) -> Result<(), SavesError> {
        let (target, relative) = file.split_location(targets, |target| target.name.as_str())?;
        let target = target.unwrap_or(&targets[0]);
        let destination = target.resolver().await?.resolve_path(relative).await?;
        let url = self
            .auth
            .object_url(&self.storage_url, &self.client_id, &file.name)?;
        let bearer = format!("Bearer {}", self.auth.access_token);

        tx.send(SavesDownloadEvent::FileStarted {
            name: file.name.clone(),
            destination: destination.clone(),
            total_bytes: file.bytes,
        })
        .ok();

        let mut compressed = Vec::with_capacity(file.bytes as usize);
        let response_headers = self
            .client
            .stream_chunk_with_headers(url.as_str(), Some(&[("Authorization", &bearer)]), |chunk| {
                tx.send(SavesDownloadEvent::Progress(chunk.len())).ok();
                compressed.extend_from_slice(&chunk);
                Box::pin(std::future::ready(Ok(())))
            })
            .await?;

        // The ETag is the MD5 of the stored (gzipped) bytes. Without one
        // there is nothing to check against, so the file is accepted as is.
        if let Some(etag) = response_headers.get("etag") {
            let expected = etag
                .to_str()
                .map_err(|_| SavesError::InvalidHeader("etag".to_string()))?
                .trim_matches('"');
            let actual = md5_hex(&compressed);
            if !actual.eq_ignore_ascii_case(expected) {
                return Err(SavesError::HashMismatch {
                    name: file.name.clone(),
                    expected: expected.to_string(),
                    actual,
                });
            }
        }

        // Parsed before anything is written, so a malformed header cannot
        // leave a half-finished file behind.
        let modified: Option<SystemTime> = response_headers
            .get("x-object-meta-locallastmodified")
            .map(|value| {
                value
                    .to_str()
                    .ok()
                    .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
                    .map(SystemTime::from)
                    .ok_or_else(|| {
                        SavesError::InvalidHeader("x-object-meta-locallastmodified".to_string())
                    })
            })
            .transpose()?;

        let mut contents = Vec::new();
        GzDecoder::new(&compressed[..]).read_to_end(&mut contents)?;

        replace_file(destination, contents, modified).await?;

        tx.send(SavesDownloadEvent::FileFinished {
            name: file.name.clone(),
        })
        .ok();
        Ok(())
    }
}

/// Suffix of the temporary file a save is written to before it replaces its
/// destination (`.<name>.gogdl-part`). The uploader never sends such a file.
pub(crate) const PART_SUFFIX: &str = ".gogdl-part";

/// Whether `name` is the file name of a leftover temporary download.
pub(crate) fn is_part_file(name: &str) -> bool {
    name.starts_with('.') && name.ends_with(PART_SUFFIX)
}

/// Removes the temporary file unless disarmed, so every early return between
/// creating it and renaming it leaves nothing behind.
struct PartFile {
    path: PathBuf,
    armed: bool,
}

impl Drop for PartFile {
    fn drop(&mut self) {
        if self.armed {
            std::fs::remove_file(&self.path).ok();
        }
    }
}

/// Writes `contents` to `destination` so that an existing file there is
/// replaced only once the new one is complete: into a sibling temporary file
/// (same directory, so the rename cannot cross filesystems), given `modified`,
/// flushed to disk, and renamed over `destination`.
///
/// All of it runs in one blocking task, with no `.await` between creating the
/// temporary file and renaming it. Dropping the caller's future therefore
/// cannot interrupt the write: the task finishes, leaving either the old
/// contents or the complete new ones, and no temporary file.
async fn replace_file(
    destination: PathBuf,
    contents: Vec<u8>,
    modified: Option<SystemTime>,
) -> std::io::Result<()> {
    tokio::task::spawn_blocking(move || {
        let name = destination
            .file_name()
            .ok_or_else(|| std::io::Error::other("destination has no file name"))?
            .to_string_lossy();
        let part_path = destination.with_file_name(format!(".{name}{PART_SUFFIX}"));

        let mut part = PartFile {
            path: part_path,
            armed: true,
        };
        let mut file = std::fs::File::create(&part.path)?;
        file.write_all(&contents)?;
        if let Some(modified) = modified {
            file.set_modified(modified)?;
        }
        file.sync_all()?;
        drop(file);
        std::fs::rename(&part.path, &destination)?;
        part.armed = false;
        Ok(())
    })
    .await
    .map_err(std::io::Error::other)?
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use chrono::{DateTime, Utc};
    use tokio::sync::Notify;

    use super::*;
    use crate::{
        ClientError,
        test_support::{ChunkServer, Reply, TempDir, gzip, md5_hex, saves_auth},
    };

    const OBJECT: &str = "/v1/42/client/__default/a.sav";

    struct Setup {
        server: ChunkServer,
        dir: TempDir,
        downloader: SavesDownloader,
        locations: [ResolvedSaveLocation; 1],
    }

    /// A server, and a save directory that already holds `a.sav` = "old save".
    async fn setup() -> Setup {
        let server = ChunkServer::start().await;
        let dir = TempDir::new();
        std::fs::write(dir.path().join("a.sav"), b"old save").unwrap();
        let mut downloader = SavesDownloader::new(
            HttpClient::new_with_client(reqwest::Client::new()),
            saves_auth(),
            "client".into(),
        );
        downloader.storage_url = format!("{}/v1/", server.base_url());
        let locations = [ResolvedSaveLocation {
            name: "__default".into(),
            path: dir.path().to_path_buf(),
        }];
        Setup {
            server,
            dir,
            downloader,
            locations,
        }
    }

    fn save_file() -> SaveFile {
        SaveFile {
            bytes: 8,
            last_modified: DateTime::<Utc>::UNIX_EPOCH,
            hash: String::new(),
            name: "__default/a.sav".into(),
            content_type: String::new(),
        }
    }

    impl Setup {
        async fn download(&self) -> Result<(), SavesError> {
            let (tx, _rx) = mpsc::unbounded_channel();
            self.downloader
                .download_files(&[save_file()], &self.locations, tx)
                .await
        }

        fn assert_old_save_and_no_part_file(&self) {
            assert_eq!(
                std::fs::read(self.dir.path().join("a.sav")).unwrap(),
                b"old save"
            );
            assert!(!self.dir.path().join(".a.sav.gogdl-part").exists());
        }
    }

    fn headers(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    #[tokio::test]
    async fn a_download_that_fails_mid_body_leaves_the_old_save() {
        let setup = setup().await;
        setup.server.script(
            OBJECT,
            vec![Reply::PartialThenClose {
                body: gzip(b"new save contents"),
                sent: 5,
            }],
        );

        let error = setup.download().await.unwrap_err();

        assert!(
            matches!(error, SavesError::ClientError(ClientError::NetworkError(_))),
            "{error:?}"
        );
        setup.assert_old_save_and_no_part_file();
    }

    #[tokio::test]
    async fn dropping_a_download_mid_transfer_leaves_the_old_save() {
        let setup = setup().await;
        let reached = Arc::new(Notify::new());
        setup.server.script(
            OBJECT,
            vec![Reply::PartialThenHang {
                body: gzip(b"new save contents"),
                sent: 5,
                notify: reached.clone(),
            }],
        );

        let download = setup.download();
        tokio::time::timeout(Duration::from_secs(5), async {
            tokio::select! {
                result = download => panic!("the download finished: {result:?}"),
                _ = reached.notified() => {}
            }
        })
        .await
        .expect("the server never sent the first bytes");

        setup.assert_old_save_and_no_part_file();
    }

    #[tokio::test]
    async fn an_etag_mismatch_leaves_the_old_save() {
        let setup = setup().await;
        setup.server.script(
            OBJECT,
            vec![Reply::BodyWithHeaders(
                headers(&[("ETag", "\"00000000000000000000000000000000\"")]),
                gzip(b"new save contents"),
            )],
        );

        let error = setup.download().await.unwrap_err();

        assert!(
            matches!(error, SavesError::HashMismatch { .. }),
            "{error:?}"
        );
        setup.assert_old_save_and_no_part_file();
    }

    #[tokio::test]
    async fn a_missing_local_last_modified_still_writes() {
        let setup = setup().await;
        let body = gzip(b"new save contents");
        let etag = format!("\"{}\"", md5_hex(&body));
        setup.server.script(
            OBJECT,
            vec![Reply::BodyWithHeaders(headers(&[("ETag", &etag)]), body)],
        );

        setup.download().await.unwrap();

        assert_eq!(
            std::fs::read(setup.dir.path().join("a.sav")).unwrap(),
            b"new save contents"
        );
        assert!(!setup.dir.path().join(".a.sav.gogdl-part").exists());
    }

    #[tokio::test]
    async fn the_stored_modification_time_is_set_on_the_replaced_save() {
        let setup = setup().await;
        setup.server.script(
            OBJECT,
            vec![Reply::BodyWithHeaders(
                headers(&[("X-Object-Meta-LocalLastModified", "2020-05-01T12:00:00Z")]),
                gzip(b"new save contents"),
            )],
        );

        setup.download().await.unwrap();

        let modified = std::fs::metadata(setup.dir.path().join("a.sav"))
            .unwrap()
            .modified()
            .unwrap();
        let expected =
            SystemTime::from(DateTime::parse_from_rfc3339("2020-05-01T12:00:00Z").unwrap());
        assert_eq!(modified, expected);
    }

    #[tokio::test]
    async fn a_failed_write_removes_the_part_file() {
        let dir = TempDir::new();
        // A directory at the destination makes the final rename fail, after
        // the temporary file has been written.
        let destination = dir.path().join("a.sav");
        std::fs::create_dir(&destination).unwrap();

        let result = replace_file(destination.clone(), b"new".to_vec(), None).await;

        assert!(result.is_err());
        assert!(destination.is_dir());
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(leftovers, ["a.sav"]);
    }

    #[test]
    fn part_files_are_recognised_by_name() {
        assert!(is_part_file(".a.sav.gogdl-part"));
        assert!(!is_part_file("a.sav"));
        assert!(!is_part_file("a.gogdl-part"));
    }
}
