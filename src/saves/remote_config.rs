use serde::Deserialize;

use crate::{SavesError, saves::SavesManager};

/// A game's Galaxy client remote configuration, as served by
/// `remote-config.gog.com` and returned by
/// [`GogDl::get_remote_config`](crate::GogDl::get_remote_config).
///
/// GOG publishes one of these per game client. It declares, per operating
/// system, what the Galaxy client enables for the game — of which this crate
/// reads the cloud saves part: whether the game supports them
/// ([`is_supported`](Self::is_supported)) and which directories they live in
/// ([`get_locations`](Self::get_locations)).
///
/// Deserialized from the JSON document GOG serves; only the keys used here
/// are modelled, the rest of the document is ignored.
#[derive(Deserialize)]
pub struct RemoteConfig {
    /// The document's body: one section per operating system.
    content: OsConfig,
}

impl RemoteConfig {
    /// Not reachable from outside the crate — `SavesManager` is not
    /// exported. Call
    /// [`GogDl::get_remote_config`](crate::GogDl::get_remote_config)
    /// instead, which delegates here internally and documents the full
    /// contract.
    ///
    /// Resolves the game's client credentials for `build_name` via
    /// `SavesManager::get_game_save_ids` (cached on the manager), then
    /// fetches
    /// `remote-config.gog.com/components/galaxy_client/clients/{client_id}`,
    /// pinned to `component_version=2.0.43`, and deserializes the JSON
    /// document into a [`RemoteConfig`]. Unlike the rest of the saves API
    /// this request carries no authorization header; it is made once,
    /// without retries, and the result is not cached.
    pub async fn get_remote_config(
        saves_manager: &SavesManager,
        game_id: i32,
        build_name: &str,
    ) -> Result<Self, SavesError> {
        let game_ids = saves_manager.get_game_save_ids(game_id, build_name).await?;
        let client_id = game_ids.client_id;
        let url = format!(
            "https://remote-config.gog.com/components/galaxy_client/clients/{client_id}?component_version=2.0.43",
        );

        let remote_config: RemoteConfig = saves_manager
            .client
            .fetch_no_retry(&url, false, false, None)
            .await?;
        Ok(remote_config)
    }
    /// Whether the game declares working cloud save support.
    ///
    /// Reads the Windows section only: `true` when the document has one and
    /// its `cloudStorage` block is present and enabled, `false` otherwise —
    /// including when the game declares cloud storage for another operating
    /// system but not for Windows.
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
    /// The cloud save locations the game declares, in the order GOG lists
    /// them.
    ///
    /// Reads the Windows section only, like
    /// [`is_supported`](Self::is_supported). `async` only for symmetry with
    /// the rest of the saves API — it awaits nothing and never performs a
    /// request, so it reflects the document fetched when this
    /// `RemoteConfig` was obtained.
    ///
    /// A section whose `cloudStorage` block is present but *disabled* still
    /// returns its locations; check
    /// [`is_supported`](Self::is_supported) first if that matters.
    ///
    /// # Errors
    /// - [`SavesError::CloudStorageNotSupported`] if the document has no
    ///   Windows section, or that section declares no `cloudStorage` block.
    pub fn get_locations(&self) -> Result<Vec<CloudStorageLocation>, SavesError> {
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

/// The body of a [`RemoteConfig`]: one section per operating system, keyed
/// by GOG's own OS names. A system GOG publishes no section for
/// deserializes as `None`.
#[derive(Deserialize)]
struct OsConfig {
    /// The game's Windows client settings. The only section this crate
    /// reads.
    #[serde(alias = "Windows")]
    pub windows: Option<OsConfigDetails>,
}

/// What the Galaxy client enables for a game on one operating system.
#[derive(Deserialize)]
struct OsConfigDetails {
    /// The game's cloud save settings, if it declares any. Absent for games
    /// that never shipped cloud saves on this system.
    #[serde(alias = "cloudStorage")]
    pub cloud_storage: Option<CloudStorageDetail>,
}

/// A game's cloud save settings for one operating system.
#[derive(Deserialize)]
struct CloudStorageDetail {
    /// Whether cloud saves are turned on for the game. Backs
    /// [`RemoteConfig::is_supported`].
    pub enabled: bool,
    /// The directories the game's saves are read from and written to. Backs
    /// [`RemoteConfig::get_locations`], and may be non-empty even when
    /// `enabled` is `false`.
    pub locations: Vec<CloudStorageLocation>,
}

/// One named cloud save location declared by a game, as returned (in a
/// `Vec`) by [`RemoteConfig::get_locations`].
///
/// Describes where on the local machine one set of cloud saves lives. GOG
/// writes the path with Galaxy's own variable syntax rather than as a
/// concrete path, and this crate passes it through verbatim.
/// [`GogDl::download_save_files`](crate::GogDl::download_save_files) and
/// [`GogDl::upload_save_files`](crate::GogDl::upload_save_files) expand it
/// against a Wine prefix themselves.
#[derive(Deserialize, Clone)]
pub struct CloudStorageLocation {
    /// The location's alias, e.g. `__default` or `saves` (Cyberpunk 2077
    /// declares a single location called `saves`). GOG also uses it to scope
    /// the game's stored files, so it shows up as the leading path segment of
    /// [`SaveFile::name`](crate::SaveFile::name), which
    /// [`SaveFile::relative_path_in`](crate::SaveFile::relative_path_in)
    /// strips off again.
    pub name: String,
    /// The directory the saves live in, as a Galaxy path expression (e.g.
    /// `<?SAVED_GAMES?>/GameName`) — not an expanded filesystem path.
    pub location: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> RemoteConfig {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn a_captured_config_deserializes() {
        let config = parse(include_str!("../../tests/fixtures/remote_config.json"));
        assert!(config.is_supported());
        let locations = config.get_locations().unwrap();
        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].name, "saves");
        assert_eq!(
            locations[0].location,
            "<?SAVED_GAMES?>/CD Projekt Red/Cyberpunk 2077"
        );
    }

    #[test]
    fn a_document_without_a_windows_section_is_unsupported() {
        let config = parse(
            r#"{"content": {"MacOS": {"cloudStorage": {"enabled": true, "locations": []}}}}"#,
        );
        assert!(!config.is_supported());
        assert!(matches!(
            config.get_locations(),
            Err(SavesError::CloudStorageNotSupported)
        ));
    }

    #[test]
    fn a_windows_section_without_cloud_storage_is_unsupported() {
        let config = parse(r#"{"content": {"Windows": {"overlay": {"supported": false}}}}"#);
        assert!(!config.is_supported());
        assert!(matches!(
            config.get_locations(),
            Err(SavesError::CloudStorageNotSupported)
        ));
    }

    #[test]
    fn disabled_cloud_storage_still_lists_its_locations() {
        let config = parse(
            r#"{"content": {"Windows": {"cloudStorage": {"enabled": false,
                "locations": [{"name": "__default", "location": "<?SAVED_GAMES?>/G"}]}}}}"#,
        );
        assert!(!config.is_supported());
        let locations = config.get_locations().unwrap();
        assert_eq!(locations[0].name, "__default");
    }
}
