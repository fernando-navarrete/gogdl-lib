use crate::{SavesError, saves::SavesManager};

#[derive(Clone)]
pub struct SaveFiles(String);

impl SaveFiles {
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
                Some(&[("Authorization", &format!("Bearer {}", &auth.access_token))]),
            )
            .await?;

        println!("{:?}", response);
        Ok(SaveFiles(response))
    }
}
