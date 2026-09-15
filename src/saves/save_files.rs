use crate::{SavesError, saves::SavesManager};

/// The raw listing of a game's cloud save files for the current user,
/// returned by [`GogDl::get_save_files`](crate::GogDl::get_save_files).
///
/// Wraps the response body from `cloudstorage.gog.com` unparsed. The inner
/// value is private and there are no accessors yet.
#[derive(Clone)]
pub struct SaveFiles(String);

impl SaveFiles {
    /// Not reachable from outside the crate — `SavesManager` is not
    /// exported. Call
    /// [`GogDl::get_save_files`](crate::GogDl::get_save_files) instead,
    /// which delegates here internally and documents the full contract.
    ///
    /// Obtains (or reuses a cached) `SavesAuth` and the game's
    /// `GameSaveIds`, then fetches
    /// `cloudstorage.gog.com/v1/{user_id}/{client_id}` with the game-scoped
    /// access token as a bearer token.
    pub async fn get_save_files(
        saves_manager: &SavesManager,
        game_id: i32,
        build_name: &str,
    ) -> Result<SaveFiles, SavesError> {
        let auth = saves_manager.get_saves_auth(game_id, build_name).await?;
        let game_ids = saves_manager.get_game_save_ids(game_id, build_name).await?;

        let url = format!(
            "https://cloudstorage.gog.com/v1/{}/{}",
            auth.user_id, game_ids.client_id
        );

        let response: String = saves_manager
            .client
            .fetch(
                &url,
                false,
                false,
                Some(&[
                    ("Authorization", &format!("Bearer {}", &auth.access_token)),
                    ("Accept", "application/json"),
                ]),
            )
            .await?;

        println!("{response:#}");
        Ok(SaveFiles(response))
    }
}
