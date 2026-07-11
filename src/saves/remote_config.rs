use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::saves::{error::SavesError, saves_manager::SavesManager};

#[derive(Serialize, Deserialize, Debug, Clone)]
struct CloudStorageLocation {
    #[allow(dead_code)]
    name: String,
    location: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct CloudStorage {
    enabled: bool,
    locations: Vec<CloudStorageLocation>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct IsSupported {
    #[allow(dead_code)]
    supported: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct OsData {
    #[allow(dead_code)]
    overlay: IsSupported,
    #[serde(alias = "cloudStorage")]
    cloud_storage: CloudStorage,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct RemoteConfigContent {
    #[serde(alias = "Windows")]
    windows: OsData,
    #[serde(alias = "MacOS")]
    #[allow(dead_code)]
    macos: OsData,
}

/// A game's Galaxy client remote-config document: whether cloud saves are
/// enabled for it, and where its save data lives locally, expressed as a
/// GOG-specific known-folder placeholder path.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RemoteConfig {
    #[allow(dead_code)]
    version: String,
    content: RemoteConfigContent,
}

impl RemoteConfig {
    fn known_folder_map() -> HashMap<&'static str, &'static str> {
        HashMap::from([
            ("SAVED_GAMES", "Saved Games"),
            ("DOCUMENTS", "Documents"),
            ("DESKTOP", "Desktop"),
            ("APPDATA", "AppData/Roaming"),
            ("LOCAL_APPDATA", "AppData/Local"),
            ("PROGRAMDATA", "ProgramData"),
            ("PUBLIC", "Users/Public"),
            ("INSTALL", "INSTALLATION_PATH"),
        ])
    }
    /// Whether this game's Windows client has cloud storage enabled at all
    /// (only Windows is consulted — GOG's own save/remote-config model is
    /// itself Windows-centric).
    pub fn is_supported(&self) -> bool {
        self.content.windows.cloud_storage.enabled
            && !self.content.windows.cloud_storage.locations.is_empty()
    }
    /// Resolves the first configured cloud-storage location into
    /// `(known_folder_name, relative_path)`, e.g. `("Saved Games", "MyGame/saves")`.
    /// The raw location string looks like `<?SAVED_GAMES?>/MyGame/saves`.
    pub fn get_path(&self) -> Result<(String, String), SavesError> {
        let map = Self::known_folder_map();

        let gog_path = self.content.windows.cloud_storage.locations[0]
            .location
            .clone();
        let (placeholder, remainder) = gog_path
            .split_once('>')
            .ok_or_else(|| SavesError::MalformedRemotePath(gog_path.clone()))?;

        let folder_key = placeholder
            .strip_prefix("<?")
            .ok_or_else(|| SavesError::MalformedRemotePath(gog_path.clone()))?
            .strip_suffix('?')
            .ok_or_else(|| SavesError::MalformedRemotePath(gog_path.clone()))?;
        let mapped = map
            .get(folder_key)
            .ok_or_else(|| SavesError::UnknownFolderKey(folder_key.to_string()))?
            .to_string();

        let mut path_buf = PathBuf::new();
        remainder
            .split(['/', '\\'])
            .filter(|s| !s.is_empty())
            .for_each(|p| path_buf.push(p));

        let path_str = path_buf.to_str().unwrap_or_default().to_owned();
        Ok((mapped, path_str))
    }
    pub async fn get_remote_config(
        saves_manager: &SavesManager,
        client_id: &str,
    ) -> Result<RemoteConfig, SavesError> {
        {
            let lock = saves_manager.inner.lock().await;
            if let Some(remote_config) = lock.remote_config.get(client_id) {
                return Ok(remote_config.clone());
            }
        }
        let url = format!(
            "https://remote-config.gog.com/components/galaxy_client/clients/{client_id}?component_version=2.0.43"
        );
        let remote_config: RemoteConfig = saves_manager.client.get_json(&url).await?;

        let mut lock = saves_manager.inner.lock().await;
        lock.remote_config
            .insert(client_id.to_string(), remote_config.clone());
        Ok(remote_config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_location(location: &str) -> RemoteConfig {
        RemoteConfig {
            version: "1".to_string(),
            content: RemoteConfigContent {
                windows: OsData {
                    overlay: IsSupported { supported: true },
                    cloud_storage: CloudStorage {
                        enabled: true,
                        locations: vec![CloudStorageLocation {
                            name: "main".to_string(),
                            location: location.to_string(),
                        }],
                    },
                },
                macos: OsData {
                    overlay: IsSupported { supported: false },
                    cloud_storage: CloudStorage {
                        enabled: false,
                        locations: Vec::new(),
                    },
                },
            },
        }
    }

    #[test]
    fn is_supported_true_when_enabled_with_locations() {
        let config = config_with_location("<?SAVED_GAMES?>/MyGame/saves");
        assert!(config.is_supported());
    }

    #[test]
    fn get_path_resolves_known_folder_placeholder() {
        let config = config_with_location("<?SAVED_GAMES?>/MyGame/saves");
        let (folder, path) = config.get_path().unwrap();
        assert_eq!(folder, "Saved Games");
        assert_eq!(path, "MyGame/saves");
    }

    #[test]
    fn get_path_errors_on_malformed_placeholder() {
        let config = config_with_location("no-placeholder-here");
        assert!(matches!(
            config.get_path(),
            Err(SavesError::MalformedRemotePath(_))
        ));
    }

    #[test]
    fn get_path_errors_on_unknown_folder_key() {
        let config = config_with_location("<?NOT_A_KEY?>/MyGame/saves");
        assert!(matches!(
            config.get_path(),
            Err(SavesError::UnknownFolderKey(key)) if key == "NOT_A_KEY"
        ));
    }
}
