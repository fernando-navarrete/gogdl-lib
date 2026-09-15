use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::{SavesError, saves::SavesManager};

/// One entry in a game's cloud save listing for the current user, as
/// returned (in a `Vec`) by
/// [`GogDl::get_save_files`](crate::GogDl::get_save_files).
///
/// Deserialized directly from the JSON array served by
/// `cloudstorage.gog.com`; field names match the response keys. Describes
/// the stored object only — the file contents are not downloaded.
#[derive(Clone, Deserialize)]
pub struct SaveFile {
    /// Size of the stored file in bytes.
    pub bytes: u64,
    /// When the file was last uploaded to cloud storage, in UTC.
    pub last_modified: DateTime<Utc>,
    /// Content hash of the stored file, as reported by GOG, for comparing
    /// against a local copy.
    pub hash: String,
    /// The file's path within the game's cloud storage area, `/`-separated.
    pub name: String,
    /// MIME type the file was stored with.
    pub content_type: String,
}

impl SaveFile {
    /// Not reachable from outside the crate — `SavesManager` is not
    /// exported. Call
    /// [`GogDl::get_save_files`](crate::GogDl::get_save_files) instead,
    /// which delegates here internally and documents the full contract.
    ///
    /// Obtains (or reuses a cached) `SavesAuth` and the game's
    /// `GameSaveIds`, then fetches
    /// `cloudstorage.gog.com/v1/{user_id}/{client_id}` with the game-scoped
    /// access token as a bearer token, deserializing the JSON array into
    /// [`SaveFile`]s. The request is made once, without retries.
    pub async fn get_save_files(
        saves_manager: &SavesManager,
        game_id: i32,
        build_name: &str,
    ) -> Result<Vec<SaveFile>, SavesError> {
        let auth = saves_manager.get_saves_auth(game_id, build_name).await?;
        let game_ids = saves_manager.get_game_save_ids(game_id, build_name).await?;

        let url = format!(
            "https://cloudstorage.gog.com/v1/{}/{}",
            auth.user_id, game_ids.client_id
        );

        let response: Vec<SaveFile> = saves_manager
            .client
            .fetch_no_retry(
                &url,
                false,
                false,
                Some(&[
                    ("Authorization", &format!("Bearer {}", &auth.access_token)),
                    ("Accept", "application/json"),
                ]),
            )
            .await?;

        Ok(response)
    }
}
