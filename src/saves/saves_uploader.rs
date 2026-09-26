use std::{
    io::Write,
    path::{Path, PathBuf},
};

use chrono::{DateTime, SecondsFormat, Utc};
use flate2::{Compression, write::GzEncoder};
use tokio::sync::mpsc;

use crate::{
    SavesError,
    client::HttpClient,
    constants::CLOUD_STORAGE_URL,
    saves::{
        checksum::md5_hex, save_location::ResolvedSaveLocation, saves_auth::SavesAuth,
        saves_upload_event::SavesUploadEvent,
    },
};

/// What GOG Galaxy's own client identifies itself as when syncing saves.
const GALAXY_USER_AGENT: &str = "GOGGalaxyCommunicationService/2.0.4.164 (Windows_32bit)";

pub struct SavesUploader {
    pub client: HttpClient,
    pub auth: SavesAuth,
    pub client_id: String,
    /// Where objects live; [`CLOUD_STORAGE_URL`] outside tests.
    pub storage_url: String,
}

impl SavesUploader {
    pub fn new(client: HttpClient, auth: SavesAuth, client_id: String) -> Self {
        Self {
            client,
            auth,
            client_id,
            storage_url: CLOUD_STORAGE_URL.to_string(),
        }
    }

    /// Uploads every file found under the directory of each of `locations`
    /// (recursively; symlinks are skipped), each to its own cloud location at
    /// the name [`remote_name`] gives it. A location whose directory does not
    /// exist has nothing to upload and is skipped.
    ///
    /// Files go up one after another in the order of `locations`, gzip-compressed,
    /// each carrying its local modification time. Not retried: the first
    /// failure aborts the call, leaving the files before it uploaded.
    pub async fn upload_files(
        &self,
        locations: &[ResolvedSaveLocation],
        tx: mpsc::UnboundedSender<SavesUploadEvent>,
    ) -> Result<(), SavesError> {
        let mut uploads = Vec::new();
        for location in locations {
            if !tokio::fs::metadata(&location.path)
                .await
                .is_ok_and(|metadata| metadata.is_dir())
            {
                continue;
            }
            for (source, relative) in list_files(&location.path).await? {
                uploads.push((source, remote_name(&location.name, &relative)));
            }
        }

        tx.send(SavesUploadEvent::Preparing {
            total_files: uploads.len(),
        })
        .ok();

        for (source, name) in uploads {
            self.upload_file(&source, &name, &tx).await?;
        }
        Ok(())
    }

    async fn upload_file(
        &self,
        source: &Path,
        name: &str,
        tx: &mpsc::UnboundedSender<SavesUploadEvent>,
    ) -> Result<(), SavesError> {
        let contents = tokio::fs::read(source).await?;
        let modified: DateTime<Utc> = tokio::fs::metadata(source).await?.modified()?.into();
        let modified = modified.to_rfc3339_opts(SecondsFormat::Secs, true);

        let mut encoder = GzEncoder::new(Vec::new(), Compression::new(6));
        encoder.write_all(&contents)?;
        let compressed = encoder.finish()?;
        let etag = md5_hex(&compressed);

        let mut url = self
            .auth
            .object_url(&self.storage_url, &self.client_id, name)?;
        url.query_pairs_mut().append_pair(
            "_gog_request_id",
            &format!("{:032x}", rand::random::<u128>()),
        );
        let bearer = format!("Bearer {}", self.auth.access_token);

        tx.send(SavesUploadEvent::FileStarted {
            name: name.to_string(),
            source: source.to_path_buf(),
            total_bytes: compressed.len() as u64,
        })
        .ok();

        let progress_tx = tx.clone();
        self.client
            .put_stream(
                url.as_str(),
                Some(&[
                    ("Authorization", &bearer),
                    ("X-Object-Meta-LocalLastModified", &modified),
                    ("Etag", &etag),
                    ("Content-Encoding", "gzip"),
                    ("Content-Type", "application/octet-stream"),
                    ("Accept", "*/*"),
                    ("Expect", "100-continue"),
                    ("User-Agent", GALAXY_USER_AGENT),
                    ("X-Object-Meta-User-Agent", GALAXY_USER_AGENT),
                ]),
                compressed,
                move |sent| {
                    progress_tx.send(SavesUploadEvent::Progress(sent)).ok();
                },
            )
            .await?;

        tx.send(SavesUploadEvent::FileFinished {
            name: name.to_string(),
        })
        .ok();
        Ok(())
    }
}

/// The cloud-side name for the local file at `relative` (a `/`-separated
/// path under the save directory) in the location `location_name`:
/// `<location>/<relative>`. The inverse of
/// [`SaveFile::relative_path_in`](crate::SaveFile::relative_path_in).
///
/// There is no fixed `saves/` namespace above the location: a `saves/` at the
/// start of a name is the location itself, for a game that calls its location
/// `saves` (Cyberpunk 2077 stores `saves/AutoSave-0/sav.dat`).
fn remote_name(location_name: &str, relative: &str) -> String {
    format!("{location_name}/{relative}")
}

/// Every regular file under `base`, as `(absolute path, relative path)`
/// pairs, with relative paths `/`-separated and the list sorted by them so
/// uploads happen in a stable order. Symlinks are skipped, so nothing
/// outside `base` can be reached through the walk.
async fn list_files(base: &Path) -> Result<Vec<(PathBuf, String)>, SavesError> {
    let mut files = Vec::new();
    let mut pending = vec![base.to_path_buf()];

    while let Some(dir) = pending.pop() {
        let mut entries = tokio::fs::read_dir(&dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let file_type = entry.file_type().await?;
            let path = entry.path();
            if file_type.is_dir() {
                pending.push(path);
            } else if file_type.is_file() {
                let relative = path
                    .strip_prefix(base)
                    .ok()
                    .and_then(|relative| {
                        relative
                            .components()
                            .map(|part| part.as_os_str().to_str())
                            .collect::<Option<Vec<_>>>()
                    })
                    .map(|parts| parts.join("/"))
                    .ok_or_else(|| {
                        SavesError::InvalidSaveFileName(path.to_string_lossy().into_owned())
                    })?;
                files.push((path, relative));
            }
        }
    }

    files.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(files)
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use tokio::sync::Notify;

    use super::*;
    use crate::{
        CloudStorageLocation, SaveFile,
        test_support::{ChunkServer, Reply, TempDir},
    };

    fn round_trip(location_name: &str, relative: &str) -> (String, String) {
        let name = remote_name(location_name, relative);
        let file = SaveFile {
            bytes: 0,
            last_modified: DateTime::<Utc>::UNIX_EPOCH,
            hash: String::new(),
            name: name.clone(),
            content_type: String::new(),
        };
        let locations = [CloudStorageLocation {
            name: location_name.to_string(),
            location: String::new(),
        }];
        (name, file.relative_path_in(&locations).unwrap().to_string())
    }

    #[test]
    fn remote_name_round_trips_with_relative_path_in() {
        let (name, relative) = round_trip("__default", "profile/slot1.sav");
        assert_eq!(name, "__default/profile/slot1.sav");
        assert_eq!(relative, "profile/slot1.sav");
    }

    #[test]
    fn remote_name_reproduces_the_names_of_a_game_whose_location_is_saves() {
        let (name, relative) = round_trip("saves", "AutoSave-0/sav.dat");
        assert_eq!(name, "saves/AutoSave-0/sav.dat");
        assert_eq!(relative, "AutoSave-0/sav.dat");

        let (name, relative) = round_trip("saves", "user.gls");
        assert_eq!(name, "saves/user.gls");
        assert_eq!(relative, "user.gls");
    }

    #[test]
    fn remote_name_round_trips_to_the_right_one_of_several_locations() {
        let locations = [
            CloudStorageLocation {
                name: "saves".to_string(),
                location: String::new(),
            },
            CloudStorageLocation {
                name: "config".to_string(),
                location: String::new(),
            },
        ];
        for (index, location) in locations.iter().enumerate() {
            let file = SaveFile {
                bytes: 0,
                last_modified: DateTime::<Utc>::UNIX_EPOCH,
                hash: String::new(),
                name: remote_name(&location.name, "slot/data.sav"),
                content_type: String::new(),
            };
            let (matched, relative) = file
                .split_location(&locations, |l| l.name.as_str())
                .unwrap();
            assert_eq!(matched.unwrap().name, locations[index].name);
            assert_eq!(relative, "slot/data.sav");
        }
    }

    #[tokio::test]
    async fn dropping_an_upload_mid_transfer_leaves_the_local_files_untouched() {
        let server = ChunkServer::start().await;
        let dir = TempDir::new();
        let files = [
            ("a.sav", b"first save".as_slice()),
            ("b.sav", b"second save"),
        ];
        for (name, contents) in files {
            std::fs::write(dir.path().join(name), contents).unwrap();
        }
        let modified = |name: &str| {
            std::fs::metadata(dir.path().join(name))
                .unwrap()
                .modified()
                .unwrap()
        };
        let before = (modified("a.sav"), modified("b.sav"));

        let object = |name: &str| format!("/v1/42/client/__default/{name}");
        let reached_b = Arc::new(Notify::new());
        server.script(&object("a.sav"), vec![Reply::Status(200)]);
        server.script(&object("b.sav"), vec![Reply::Stall(reached_b.clone())]);

        let mut uploader = SavesUploader::new(
            HttpClient::new_with_client(reqwest::Client::new()),
            SavesAuth {
                access_token: "token".into(),
                refresh_token: String::new(),
                expires_in: 3600,
                token_type: "bearer".into(),
                session_id: String::new(),
                scope: None,
                user_id: "42".into(),
                valid_until: None,
            },
            "client".into(),
        );
        uploader.storage_url = format!("{}/v1/", server.base_url());
        let locations = [ResolvedSaveLocation {
            name: "__default".into(),
            path: dir.path().to_path_buf(),
        }];

        // Drop the upload once the server has b.sav and won't answer.
        let (tx, mut rx) = mpsc::unbounded_channel();
        let upload = uploader.upload_files(&locations, tx);
        tokio::time::timeout(Duration::from_secs(5), async {
            tokio::select! {
                result = upload => panic!("the upload finished: {result:?}"),
                _ = reached_b.notified() => {}
            }
        })
        .await
        .expect("b.sav was never sent");

        for (name, contents) in files {
            assert_eq!(std::fs::read(dir.path().join(name)).unwrap(), contents);
        }
        assert_eq!((modified("a.sav"), modified("b.sav")), before);
        assert_eq!(server.requests(&object("a.sav")), 1);
        let (mut started, mut finished) = (Vec::new(), Vec::new());
        while let Ok(event) = rx.try_recv() {
            match event {
                SavesUploadEvent::FileStarted { name, .. } => started.push(name),
                SavesUploadEvent::FileFinished { name } => finished.push(name),
                _ => {}
            }
        }
        assert_eq!(started, ["__default/a.sav", "__default/b.sav"]);
        assert_eq!(finished, ["__default/a.sav"]);
    }
}
