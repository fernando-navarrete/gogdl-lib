use std::{io::Read, path::PathBuf, time::SystemTime};

use chrono::DateTime;
use flate2::read::GzDecoder;
use tokio::{
    io::AsyncWriteExt,
    sync::{OnceCell, mpsc},
};

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
    /// with it. Not retried: the first failure aborts the call, leaving the
    /// files before it in place.
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

        let mut out = tokio::fs::File::create(&destination).await?;
        out.write_all(&contents).await?;
        out.flush().await?;
        if let Some(modified) = modified {
            out.into_std().await.set_modified(modified)?;
        }

        tx.send(SavesDownloadEvent::FileFinished {
            name: file.name.clone(),
        })
        .ok();
        Ok(())
    }
}
