use std::io::Read;
use std::path::Path;

use chrono::{DateTime, FixedOffset, SecondsFormat, Utc};
use filetime::{FileTime, set_file_times};
use flate2::read::GzDecoder;
use md5::{Digest, Md5};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc::UnboundedSender;

use crate::auth::SavesAuth;
use crate::saves::{error::SavesError, saves_manager::SavesManager};

/// One save file on GOG's cloud storage, identified by its raw listing line
/// (e.g. `saves/MyGame/quicksave.sav`).
#[derive(Debug, Clone)]
pub struct SaveFile(String);

impl SaveFile {
    /// The save's path relative to the game's save root, with the `saves/`
    /// listing prefix stripped.
    pub fn get_path(&self) -> String {
        self.0.strip_prefix("saves/").unwrap_or(&self.0).to_owned()
    }
}

/// Cumulative transfer progress for a save-file download or upload, sent
/// over the caller's `mpsc::UnboundedSender<SaveProgress>` as the transfer
/// proceeds. `total` is 0 if the server didn't report a size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaveProgress {
    pub transferred: u64,
    pub total: u64,
}

fn verify_md5(data: &[u8], expected_hex: &str) -> Result<(), SavesError> {
    let mut hasher = Md5::new();
    hasher.update(data);
    let actual = hex::encode(hasher.finalize());
    if actual != expected_hex {
        Err(SavesError::HashMismatch {
            expected: expected_hex.to_string(),
            actual,
        })
    } else {
        Ok(())
    }
}

fn parse_save_files_list(response: &str) -> Result<Vec<SaveFile>, SavesError> {
    response
        .lines()
        .map(|line| {
            if line.starts_with("saves/") {
                Ok(SaveFile(line.to_string()))
            } else {
                Err(SavesError::MalformedSaveLine(line.to_string()))
            }
        })
        .collect()
}

impl SaveFile {
    /// Lists every save file stored for `client_id` under the account tied
    /// to `saves_auth`.
    pub async fn list(
        saves_manager: &SavesManager,
        saves_auth: &SavesAuth,
        client_id: &str,
    ) -> Result<Vec<SaveFile>, SavesError> {
        let url = format!(
            "https://cloudstorage.gog.com/v1/{}/{}",
            saves_auth.user_id, client_id
        );
        let response = saves_manager
            .client
            .get_text_with_auth(&url, &saves_auth.access_token)
            .await?;
        parse_save_files_list(&response)
    }
    /// Downloads this save file to `path`, verifying its MD5 (from the
    /// response `ETag`) if present, gzip-decompressing it, and restoring the
    /// original modified-time (from the `X-Object-Meta-LocalLastModified`
    /// header) so a later upload can compare timestamps.
    pub async fn download(
        &self,
        saves_manager: &SavesManager,
        saves_auth: &SavesAuth,
        path: &Path,
        tx: UnboundedSender<SaveProgress>,
    ) -> Result<(), SavesError> {
        let url = format!(
            "https://cloudstorage.gog.com/v1/{}/{}/{}",
            saves_auth.user_id, saves_auth.client_id, self.0
        );

        let download = saves_manager
            .client
            .download_save(&url, &saves_auth.access_token, move |transferred, total| {
                tx.send(SaveProgress { transferred, total }).ok();
            })
            .await?;

        if let Some(hash) = &download.md5 {
            verify_md5(&download.bytes, hash)?;
        }

        let mut decoded_buffer = Vec::new();
        let mut z = GzDecoder::new(&download.bytes[..]);
        z.read_to_end(&mut decoded_buffer)?;

        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let mut file = tokio::fs::File::create(path).await?;
        file.write_all(&decoded_buffer).await?;
        file.flush().await?;

        if let Some(timestamp) = download.last_modified {
            let dt: DateTime<FixedOffset> = timestamp
                .parse()
                .map_err(|_| SavesError::InvalidTimestamp(timestamp.clone()))?;
            let file_time = FileTime::from_unix_time(dt.timestamp(), dt.timestamp_subsec_nanos());
            set_file_times(path, file_time, file_time)?;
        }

        Ok(())
    }
    /// Uploads the local file at `path` as `saves/{url_path}`, gzip-compressed,
    /// tagging it with the file's own modified-time so a later download can
    /// restore it.
    pub async fn upload(
        saves_manager: &SavesManager,
        saves_auth: &SavesAuth,
        path: &Path,
        url_path: &str,
        tx: UnboundedSender<SaveProgress>,
    ) -> Result<(), SavesError> {
        let request_id = uuid::Uuid::new_v4().to_string();
        let url = format!(
            "https://cloudstorage.gog.com/v1/{}/{}/saves/{}?_gog_request_id={}",
            saves_auth.user_id, saves_auth.client_id, url_path, request_id
        );

        let mut file = tokio::fs::File::open(path).await?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).await?;

        let metadata = tokio::fs::metadata(path).await?;
        let modified: DateTime<Utc> = metadata.modified()?.into();
        let timestamp = modified.to_rfc3339_opts(SecondsFormat::Secs, true);

        saves_manager
            .client
            .upload_save(
                &url,
                &saves_auth.access_token,
                &timestamp,
                buffer,
                move |sent, total| {
                    tx.send(SaveProgress {
                        transferred: sent,
                        total,
                    })
                    .ok();
                },
            )
            .await?;
        Ok(())
    }
    /// Deletes this save file from cloud storage.
    ///
    /// **Unverified**: DELETE support on `cloudstorage.gog.com` is the
    /// natural REST extrapolation of the documented list/upload/download
    /// endpoints (same URL shape as `download`), carried over from the
    /// original `gogdl-lib` implementation, but it has never been confirmed
    /// against a real account. Treat a successful `Ok(())` cautiously.
    pub async fn delete(
        &self,
        saves_manager: &SavesManager,
        saves_auth: &SavesAuth,
    ) -> Result<(), SavesError> {
        let url = format!(
            "https://cloudstorage.gog.com/v1/{}/{}/{}",
            saves_auth.user_id, saves_auth.client_id, self.0
        );
        saves_manager
            .client
            .delete_with_auth(&url, &saves_auth.access_token)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_save_files_list_accepts_valid_lines() {
        let response = "saves/foo/bar.sav\nsaves/baz.sav";
        let files = parse_save_files_list(response).unwrap();
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn parse_save_files_list_fails_on_malformed_line() {
        let response = "saves/foo/bar.sav\nnot-a-save-line";
        let result = parse_save_files_list(response);
        match result {
            Err(SavesError::MalformedSaveLine(line)) => assert_eq!(line, "not-a-save-line"),
            other => panic!("expected MalformedSaveLine, got {other:?}"),
        }
    }

    #[test]
    fn verify_md5_ok_when_matching() {
        let data = b"hello world";
        let mut hasher = Md5::new();
        hasher.update(data);
        let hex = hex::encode(hasher.finalize());
        assert!(verify_md5(data, &hex).is_ok());
    }

    #[test]
    fn verify_md5_errors_when_mismatched() {
        let data = b"hello world";
        let result = verify_md5(data, "0000000000000000000000000000000");
        match result {
            Err(SavesError::HashMismatch { expected, actual }) => {
                assert_eq!(expected, "0000000000000000000000000000000");
                assert_ne!(actual, expected);
            }
            other => panic!("expected HashMismatch, got {other:?}"),
        }
    }

    #[test]
    fn get_path_strips_saves_prefix() {
        let save_file = SaveFile("saves/foo/bar.sav".to_string());
        assert_eq!(save_file.get_path(), "foo/bar.sav");
    }

    #[test]
    fn get_path_leaves_non_prefixed_path_untouched() {
        let save_file = SaveFile("weird/foo.sav".to_string());
        assert_eq!(save_file.get_path(), "weird/foo.sav");
    }
}
