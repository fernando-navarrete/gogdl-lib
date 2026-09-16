use serde::Deserialize;

use crate::{SavesError, saves::SavesManager};

#[derive(Deserialize)]
pub struct RemoteConfig {
    version: String,
    content: OsConfig,
}

impl RemoteConfig {
    pub async fn get_remote_config(
        saves_manager: &SavesManager,
        game_id: i32,
        build_name: &str,
    ) -> Result<Self, SavesError> {
        let game_ids = saves_manager.get_game_save_ids(game_id, build_name).await?;
        let client_secret = game_ids.client_secret;
        let url = format!(
            "https://remote-config.gog.com/components/galaxy_client/clients/{client_secret}?component_version=2.0.43",
        );

        let remote_config: RemoteConfig = saves_manager
            .client
            .fetch_no_retry(&url, false, false, None)
            .await?;
        Ok(remote_config)
    }
    pub fn is_supported(&self) -> bool {
        let windows = match &self.content.windows {
            Some(windows) => windows,
            None => return false,
        };
        if let Some(cloud_storage) = &windows.cloud_storage {
            cloud_storage.enabled
        } else {
            false
        }
    }
    pub async fn get_locations(&self) -> Result<Vec<CloudStorageLocation>, SavesError> {
        let windows = match &self.content.windows {
            Some(windows) => windows,
            None => return Err(SavesError::CloudStorageNotSupported),
        };
        let cloud_storage = match &windows.cloud_storage {
            Some(cloud_storage) => cloud_storage,
            None => return Err(SavesError::CloudStorageNotSupported),
        };
        Ok(cloud_storage.locations.clone())
    }
}

#[derive(Deserialize)]
struct OsConfig {
    #[serde(alias = "Windows")]
    pub windows: Option<OsConfigDetails>,
    #[serde(alias = "MacOS")]
    pub macos: Option<OsConfigDetails>,
}

#[derive(Deserialize)]
struct OsConfigDetails {
    pub overlay: Option<OverlayDetail>,
    #[serde(alias = "cloudStorage")]
    pub cloud_storage: Option<CloudStorageDetail>,
}

#[derive(Deserialize)]
struct OverlayDetail {
    pub supported: bool,
}

#[derive(Deserialize)]
struct CloudStorageDetail {
    pub enabled: bool,
    pub locations: Vec<CloudStorageLocation>,
}

#[derive(Deserialize, Clone)]
pub struct CloudStorageLocation {
    pub name: String,
    pub location: String,
}
