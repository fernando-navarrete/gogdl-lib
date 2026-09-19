use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::{CloudStorageLocation, SavesError, saves::SavesManager};

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
    /// game's saves in, assuming [`name`](Self::name) is laid out as
    /// `saves/<location>/<path>`: the `saves/` prefix (tolerated if absent)
    /// and the segment after it are always dropped.
    ///
    /// `saves/__default/profile/slot1.sav` becomes `profile/slot1.sav`. That
    /// assumption does not hold for every game. A game whose location is
    /// itself named `saves`, like Cyberpunk 2077, stores
    /// `saves/AutoSave-0/sav.dat`, and this method drops `AutoSave-0` along
    /// with the prefix, so every slot collapses onto `sav.dat`. Prefer
    /// [`relative_path_in`](Self::relative_path_in), which only drops a
    /// segment that is a declared location.
    ///
    /// The result is still untrusted text — it is resolved against the
    /// caller's directory by `PathResolver`, which rejects anything that
    /// escapes it.
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

    /// The file's path relative to the directory the caller keeps this
    /// game's saves in, dropping a leading segment of [`name`](Self::name)
    /// only when it really is one of the game's declared `locations`
    /// (from [`RemoteConfig::get_locations`](crate::RemoteConfig::get_locations)).
    ///
    /// GOG names objects `<location>/<path>`, where `<location>` is the
    /// [`CloudStorageLocation::name`] the game declares. Some games
    /// (`__default`) are listed as `saves/__default/profile/slot1.sav`, others
    /// declare a location called `saves` and are listed as
    /// `saves/AutoSave-0/sav.dat`. The name is resolved as follows:
    ///
    /// 1. If its first segment is a declared location, that segment is
    ///    dropped.
    /// 2. Otherwise a leading `saves/` is dropped, and then one more segment
    ///    if it is a declared location.
    /// 3. Otherwise the name is kept whole.
    ///
    /// So with `["saves"]`, `saves/AutoSave-0/sav.dat` becomes
    /// `AutoSave-0/sav.dat` and `saves/user.gls` becomes `user.gls`; with
    /// `["__default"]`, `saves/__default/profile/slot1.sav` becomes
    /// `profile/slot1.sav`. With no locations only `saves/` is dropped, so an
    /// empty slice is a safe fallback when the game's remote config cannot be
    /// read.
    ///
    /// The result is still untrusted text — it is resolved against the
    /// caller's directory by `PathResolver`, which rejects anything that
    /// escapes it.
    ///
    /// # Errors
    /// [`SavesError::InvalidSaveFileName`] if nothing is left once the
    /// prefix and location segment are dropped, e.g. `saves/` or
    /// `saves/__default/`.
    pub fn relative_path_in(&self, locations: &[CloudStorageLocation]) -> Result<&str, SavesError> {
        self.split_location(locations, |location| location.name.as_str())
            .map(|(_, relative)| relative)
    }

    /// [`relative_path_in`](Self::relative_path_in), but also returning which
    /// of `locations` the name belongs to — `None` when no declared location
    /// matched (only a leading `saves/` was dropped). `name_of` reads a
    /// location's alias from whatever type the caller keeps them in.
    pub(crate) fn split_location<'a, 'l, T>(
        &'a self,
        locations: &'l [T],
        name_of: impl Fn(&T) -> &str,
    ) -> Result<(Option<&'l T>, &'a str), SavesError> {
        let name = self.name.as_str();
        let (location, relative) = if let Some(found) = strip_location(name, locations, &name_of) {
            (Some(found.0), found.1)
        } else if let Some(rest) = name.strip_prefix("saves/") {
            match strip_location(rest, locations, &name_of) {
                Some((location, relative)) => (Some(location), relative),
                None => (None, rest),
            }
        } else {
            (None, name)
        };

        if relative.is_empty() {
            Err(SavesError::InvalidSaveFileName(self.name.clone()))
        } else {
            Ok((location, relative))
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

/// The one of `locations` that `s`'s first `/`-separated segment names, and
/// what follows that segment (empty if there is nothing); `None` if the
/// segment names none of them.
fn strip_location<'a, 'l, T>(
    s: &'a str,
    locations: &'l [T],
    name_of: &impl Fn(&T) -> &str,
) -> Option<(&'l T, &'a str)> {
    let (first, rest) = s.split_once('/').unwrap_or((s, ""));
    locations
        .iter()
        .find(|location| name_of(location) == first)
        .map(|location| (location, rest))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn locations(names: &[&str]) -> Vec<CloudStorageLocation> {
        names
            .iter()
            .map(|name| CloudStorageLocation {
                name: name.to_string(),
                location: String::new(),
            })
            .collect()
    }

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

    #[test]
    fn relative_path_in_keeps_every_slot_of_a_game_whose_location_is_saves() {
        let mut names = Vec::new();
        for slot in (0..20)
            .map(|n| format!("AutoSave-{n}"))
            .chain((25..=30).map(|n| format!("ManualSave-{n}")))
            .chain(["EndGameSave-0".to_string()])
        {
            for file in ["sav.dat", "metadata.9.json", "screenshot.png"] {
                names.push(format!("saves/{slot}/{file}"));
            }
        }
        names.push("saves/user.gls".to_string());

        let locations = locations(&["saves"]);
        let paths: HashSet<String> = names
            .iter()
            .map(|name| {
                save_file(name)
                    .relative_path_in(&locations)
                    .unwrap()
                    .to_string()
            })
            .collect();

        assert_eq!(paths.len(), names.len());
        assert!(paths.contains("AutoSave-0/sav.dat"));
        assert!(paths.contains("ManualSave-30/sav.dat"));
        assert!(paths.contains("user.gls"));
    }

    #[test]
    fn relative_path_in_strips_a_declared_location_after_saves() {
        let file = save_file("saves/__default/profile/slot1.sav");
        assert_eq!(
            file.relative_path_in(&locations(&["__default"])).unwrap(),
            "profile/slot1.sav"
        );
    }

    #[test]
    fn relative_path_in_strips_a_declared_location_without_saves_prefix() {
        let file = save_file("__default/config.ini");
        assert_eq!(
            file.relative_path_in(&locations(&["__default"])).unwrap(),
            "config.ini"
        );
    }

    #[test]
    fn relative_path_in_does_not_strip_a_segment_that_is_not_a_location() {
        let file = save_file("saves/AutoSave-0/sav.dat");
        assert_eq!(
            file.relative_path_in(&locations(&["__default"])).unwrap(),
            "AutoSave-0/sav.dat"
        );
    }

    #[test]
    fn relative_path_in_without_locations_only_strips_saves() {
        assert_eq!(
            save_file("saves/user.gls").relative_path_in(&[]).unwrap(),
            "user.gls"
        );
        assert_eq!(
            save_file("saves/AutoSave-0/sav.dat")
                .relative_path_in(&[])
                .unwrap(),
            "AutoSave-0/sav.dat"
        );
    }

    #[test]
    fn relative_path_in_tells_apart_files_that_differ_only_in_directory() {
        let locations = locations(&["saves"]);
        let a = save_file("saves/a/x");
        let b = save_file("saves/b/x");
        assert_ne!(
            a.relative_path_in(&locations).unwrap(),
            b.relative_path_in(&locations).unwrap()
        );
    }

    #[test]
    fn relative_path_in_rejects_name_with_nothing_left() {
        let declared = locations(&["__default"]);
        for name in ["saves/__default", "saves/__default/", "saves/", "__default"] {
            assert!(
                matches!(
                    save_file(name).relative_path_in(&declared),
                    Err(SavesError::InvalidSaveFileName(_))
                ),
                "{name}"
            );
        }
        assert!(matches!(
            save_file("saves/").relative_path_in(&[]),
            Err(SavesError::InvalidSaveFileName(_))
        ));
    }

    #[test]
    fn split_location_names_the_location_the_file_belongs_to() {
        let declared = locations(&["__default", "config"]);
        let by_name = |location: &CloudStorageLocation| location.name.clone();

        let file = save_file("saves/__default/profile/slot1.sav");
        let (location, relative) = file.split_location(&declared, |l| l.name.as_str()).unwrap();
        assert_eq!(location.map(by_name).as_deref(), Some("__default"));
        assert_eq!(relative, "profile/slot1.sav");

        let file = save_file("config/video.ini");
        let (location, relative) = file.split_location(&declared, |l| l.name.as_str()).unwrap();
        assert_eq!(location.map(by_name).as_deref(), Some("config"));
        assert_eq!(relative, "video.ini");

        let file = save_file("saves/AutoSave-0/sav.dat");
        let saves = locations(&["saves"]);
        let (location, relative) = file.split_location(&saves, |l| l.name.as_str()).unwrap();
        assert_eq!(location.map(by_name).as_deref(), Some("saves"));
        assert_eq!(relative, "AutoSave-0/sav.dat");
    }

    #[test]
    fn split_location_is_none_when_no_location_matches() {
        let declared = locations(&["__default"]);
        let file = save_file("saves/AutoSave-0/sav.dat");
        let (location, relative) = file.split_location(&declared, |l| l.name.as_str()).unwrap();
        assert!(location.is_none());
        assert_eq!(relative, "AutoSave-0/sav.dat");
    }
}
