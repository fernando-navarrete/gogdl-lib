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
    /// The file's path relative to the directory the caller keeps this
    /// game's saves in: [`name`](Self::name) without its `saves/` prefix
    /// (tolerated if absent) and without the leading location segment.
    ///
    /// `saves/__default/profile/slot1.sav` becomes `profile/slot1.sav`. The
    /// result is still untrusted text — it is resolved against the caller's
    /// directory by `PathResolver`, which rejects anything that escapes it.
    ///
    /// # Errors
    /// [`SavesError::InvalidSaveFileName`] if nothing is left after the
    /// location segment.
    pub fn relative_path(&self) -> Result<&str, SavesError> {
        let name = self.name.strip_prefix("saves/").unwrap_or(&self.name);
        match name.split_once('/') {
            Some((_location, relative)) if !relative.is_empty() => Ok(relative),
            _ => Err(SavesError::InvalidSaveFileName(self.name.clone())),
        }
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    fn save_file(name: &str) -> SaveFile {
        SaveFile {
            bytes: 0,
            last_modified: DateTime::<Utc>::UNIX_EPOCH,
            hash: String::new(),
            name: name.to_string(),
            content_type: String::new(),
        }
    }

    #[test]
    fn relative_path_strips_prefix_and_location() {
        let file = save_file("saves/__default/profile/slot1.sav");
        assert_eq!(file.relative_path().unwrap(), "profile/slot1.sav");
    }

    #[test]
    fn relative_path_handles_file_directly_under_location() {
        let file = save_file("saves/__default/config.ini");
        assert_eq!(file.relative_path().unwrap(), "config.ini");
    }

    #[test]
    fn relative_path_tolerates_missing_saves_prefix() {
        let file = save_file("__default/config.ini");
        assert_eq!(file.relative_path().unwrap(), "config.ini");
    }

    #[test]
    fn relative_path_rejects_name_with_nothing_after_location() {
        for name in ["saves/__default", "saves/__default/", "config.ini"] {
            assert!(matches!(
                save_file(name).relative_path(),
                Err(SavesError::InvalidSaveFileName(_))
            ));
        }
    }
}
