use std::path::{Path, PathBuf};

use crate::{CloudStorageLocation, SavesError, fs::sanitize_relative_path};

/// One of a game's cloud save locations, expanded to a directory.
pub struct ResolvedSaveLocation {
    /// The location's alias from the remote config (`__default`, `saves`, …),
    /// which is also the leading segment of its files' cloud names.
    pub name: String,
    /// The absolute directory the location's Galaxy path expression expands
    /// to inside the Wine prefix (or the install directory).
    pub path: PathBuf,
}

/// Where a Galaxy path variable points.
enum Base {
    /// The game's install directory.
    Install,
    /// A directory below the Wine user directory, as a `/`-separated path.
    User(&'static str),
}

/// Expands each of `locations` to a directory, in the same order.
///
/// Galaxy writes a location as `<?VARIABLE?>/relative/path`. The variable
/// maps to a directory of a Windows profile, which inside the Wine prefix
/// lives under `<prefix>/drive_c/users/<user>` (see [`wine_user_dir`] for how
/// `<user>` is found):
///
/// | Variable                     | Directory                   |
/// |------------------------------|-----------------------------|
/// | `SAVED_GAMES`                | `<user>/Saved Games`        |
/// | `DOCUMENTS`                  | `<user>/Documents`          |
/// | `APPLICATION_DATA_ROAMING`   | `<user>/AppData/Roaming`    |
/// | `APPLICATION_DATA_LOCAL`     | `<user>/AppData/Local`      |
/// | `APPLICATION_DATA_LOCAL_LOW` | `<user>/AppData/LocalLow`   |
/// | `INSTALL`                    | `install_path`              |
///
/// The user directory is only looked up if some location needs it.
///
/// The path after the variable is untrusted: separators are normalized, `..`
/// components are dropped and characters Windows forbids in file names are
/// replaced, so a location can never leave its variable's directory.
///
/// # Errors
/// - [`SavesError::InvalidSaveLocation`] if an expression is not a variable
///   followed by an optional path.
/// - [`SavesError::UnknownSaveLocationVariable`] for a variable not in the
///   table, such as the macOS-only `APPLICATION_SUPPORT`.
/// - [`SavesError::WineUserDirNotFound`] if a location needs the user
///   directory and none can be identified.
pub async fn resolve_locations(
    locations: &[CloudStorageLocation],
    prefix: &Path,
    install_path: &Path,
) -> Result<Vec<ResolvedSaveLocation>, SavesError> {
    let mut user_dir: Option<PathBuf> = None;
    let mut resolved = Vec::with_capacity(locations.len());

    for location in locations {
        let (variable, relative) = split_expression(&location.location)?;
        let base = match base_of(variable)? {
            Base::Install => install_path.to_path_buf(),
            Base::User(subdir) => {
                if user_dir.is_none() {
                    user_dir = Some(wine_user_dir(prefix).await?);
                }
                let user_dir = user_dir.as_deref().unwrap_or(prefix);
                user_dir.join(subdir)
            }
        };
        resolved.push(ResolvedSaveLocation {
            name: location.name.clone(),
            path: base.join(sanitize_relative_path(relative)),
        });
    }
    Ok(resolved)
}

/// The variable name and the path after it, for `<?NAME?>path`.
fn split_expression(expression: &str) -> Result<(&str, &str), SavesError> {
    let invalid = || SavesError::InvalidSaveLocation(expression.to_string());

    let rest = expression.strip_prefix("<?").ok_or_else(invalid)?;
    let (variable, relative) = rest.split_once("?>").ok_or_else(invalid)?;
    let well_formed = !variable.is_empty()
        && variable
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !well_formed || relative.contains("<?") {
        return Err(invalid());
    }
    Ok((variable, relative))
}

fn base_of(variable: &str) -> Result<Base, SavesError> {
    match variable {
        "INSTALL" => Ok(Base::Install),
        "SAVED_GAMES" => Ok(Base::User("Saved Games")),
        "DOCUMENTS" => Ok(Base::User("Documents")),
        "APPLICATION_DATA_ROAMING" => Ok(Base::User("AppData/Roaming")),
        "APPLICATION_DATA_LOCAL" => Ok(Base::User("AppData/Local")),
        "APPLICATION_DATA_LOCAL_LOW" => Ok(Base::User("AppData/LocalLow")),
        other => Err(SavesError::UnknownSaveLocationVariable(other.to_string())),
    }
}

/// `<prefix>/drive_c/users/<user>`, the profile directory Wine keeps a
/// prefix's Windows user in.
///
/// The user's name is not recorded anywhere reliable, so it is worked out
/// from what is there; the first directory that exists wins:
///
/// 1. `steamuser`, which Proton always uses.
/// 2. The host's `$USER`, then `$USERNAME`, which plain Wine uses.
/// 3. The only directory in `drive_c/users` other than `Public`, if there is
///    exactly one.
///
/// # Errors
/// [`SavesError::WineUserDirNotFound`] if none of these applies, including
/// when `drive_c/users` does not exist.
async fn wine_user_dir(prefix: &Path) -> Result<PathBuf, SavesError> {
    let host_users: Vec<String> = ["USER", "USERNAME"]
        .iter()
        .filter_map(|key| std::env::var(key).ok())
        .collect();
    find_user_dir(prefix, &host_users).await
}

async fn find_user_dir(prefix: &Path, host_users: &[String]) -> Result<PathBuf, SavesError> {
    let users = prefix.join("drive_c").join("users");
    let not_found = || SavesError::WineUserDirNotFound(prefix.to_path_buf());

    let named = std::iter::once("steamuser").chain(host_users.iter().map(String::as_str));
    for name in named {
        if !is_single_component(name) {
            continue;
        }
        let candidate = users.join(name);
        if tokio::fs::metadata(&candidate)
            .await
            .is_ok_and(|metadata| metadata.is_dir())
        {
            return Ok(candidate);
        }
    }

    let mut entries = tokio::fs::read_dir(&users).await.map_err(|_| not_found())?;
    let mut others = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(|_| not_found())? {
        let is_dir = entry.file_type().await.is_ok_and(|kind| kind.is_dir());
        if is_dir && !entry.file_name().eq_ignore_ascii_case("Public") {
            others.push(entry.path());
        }
    }
    match others.as_slice() {
        [only] => Ok(only.clone()),
        _ => Err(not_found()),
    }
}

/// Whether `name` is a plain directory name, so joining it to a directory
/// cannot leave that directory. Guards against a hostile `$USER`.
fn is_single_component(name: &str) -> bool {
    !name.is_empty() && !matches!(name, "." | "..") && !name.contains(['/', '\\', '\0'])
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    /// A directory under the system temp dir, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "gogdl-save-location-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn add_user(&self, name: &str) {
            std::fs::create_dir_all(self.0.join("drive_c/users").join(name)).unwrap();
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    fn location(name: &str, expression: &str) -> CloudStorageLocation {
        CloudStorageLocation {
            name: name.to_string(),
            location: expression.to_string(),
        }
    }

    /// Resolves one expression against a prefix whose only user is `steamuser`.
    async fn resolve_one(expression: &str) -> Result<PathBuf, SavesError> {
        let prefix = TempDir::new();
        prefix.add_user("steamuser");
        let resolved = resolve_locations(
            &[location("saves", expression)],
            &prefix.0,
            Path::new("/games/Game"),
        )
        .await?;
        let user = prefix.0.join("drive_c/users/steamuser");
        Ok(resolved[0]
            .path
            .strip_prefix(&user)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| resolved[0].path.clone()))
    }

    #[tokio::test]
    async fn each_variable_expands_to_its_directory() {
        for (variable, expected) in [
            ("SAVED_GAMES", "Saved Games/Game"),
            ("DOCUMENTS", "Documents/Game"),
            ("APPLICATION_DATA_ROAMING", "AppData/Roaming/Game"),
            ("APPLICATION_DATA_LOCAL", "AppData/Local/Game"),
            ("APPLICATION_DATA_LOCAL_LOW", "AppData/LocalLow/Game"),
        ] {
            let path = resolve_one(&format!("<?{variable}?>/Game")).await.unwrap();
            assert_eq!(path, Path::new(expected), "{variable}");
        }
    }

    #[tokio::test]
    async fn install_resolves_against_the_install_path_not_the_prefix() {
        let path = resolve_one("<?INSTALL?>/saves").await.unwrap();
        assert_eq!(path, Path::new("/games/Game/saves"));
    }

    #[tokio::test]
    async fn install_does_not_need_a_wine_user_directory() {
        let prefix = TempDir::new();
        let resolved = resolve_locations(
            &[location("saves", "<?INSTALL?>/saves")],
            &prefix.0,
            Path::new("/games/Game"),
        )
        .await
        .unwrap();
        assert_eq!(resolved[0].path, Path::new("/games/Game/saves"));
    }

    #[tokio::test]
    async fn backslash_separators_are_normalized() {
        let path = resolve_one(r"<?DOCUMENTS?>\My Game\Saves").await.unwrap();
        assert_eq!(path, Path::new("Documents/My Game/Saves"));
    }

    #[tokio::test]
    async fn parent_components_cannot_escape_the_variable_directory() {
        let path = resolve_one("<?SAVED_GAMES?>/../../../etc").await.unwrap();
        assert_eq!(path, Path::new("Saved Games/etc"));
    }

    #[tokio::test]
    async fn a_bare_variable_is_its_own_directory() {
        assert_eq!(
            resolve_one("<?SAVED_GAMES?>").await.unwrap(),
            Path::new("Saved Games")
        );
    }

    #[tokio::test]
    async fn every_location_keeps_its_own_name_and_directory() {
        let prefix = TempDir::new();
        prefix.add_user("steamuser");
        let resolved = resolve_locations(
            &[
                location("a", "<?SAVED_GAMES?>/A"),
                location("b", "<?APPLICATION_DATA_LOCAL?>/B"),
            ],
            &prefix.0,
            Path::new("/games/Game"),
        )
        .await
        .unwrap();
        let user = prefix.0.join("drive_c/users/steamuser");
        assert_eq!(resolved[0].name, "a");
        assert_eq!(resolved[0].path, user.join("Saved Games/A"));
        assert_eq!(resolved[1].name, "b");
        assert_eq!(resolved[1].path, user.join("AppData/Local/B"));
    }

    #[tokio::test]
    async fn unknown_variables_are_an_error() {
        for variable in ["APPLICATION_SUPPORT", "SAVED_GAMEZ"] {
            let err = resolve_one(&format!("<?{variable}?>/Game"))
                .await
                .unwrap_err();
            assert!(
                matches!(&err, SavesError::UnknownSaveLocationVariable(name) if name == variable),
                "{variable}: {err}"
            );
        }
    }

    #[tokio::test]
    async fn malformed_expressions_are_an_error() {
        for expression in [
            "Saved Games/Game",
            "C:/Users/Game",
            "<?SAVED_GAMES/Game",
            "<??>/Game",
            "<?p=SAVED_GAMES?>/Game",
            "<?SAVED_GAMES?>/<?DOCUMENTS?>",
            "",
        ] {
            let err = resolve_one(expression).await.unwrap_err();
            assert!(
                matches!(err, SavesError::InvalidSaveLocation(_)),
                "{expression:?}: {err}"
            );
        }
    }

    #[tokio::test]
    async fn steamuser_is_preferred_over_a_host_named_user() {
        let prefix = TempDir::new();
        prefix.add_user("alice");
        prefix.add_user("steamuser");
        let found = find_user_dir(&prefix.0, &["alice".to_string()])
            .await
            .unwrap();
        assert_eq!(found, prefix.0.join("drive_c/users/steamuser"));
    }

    #[tokio::test]
    async fn the_host_user_is_used_when_there_is_no_steamuser() {
        let prefix = TempDir::new();
        prefix.add_user("alice");
        prefix.add_user("bob");
        let found = find_user_dir(&prefix.0, &["alice".to_string()])
            .await
            .unwrap();
        assert_eq!(found, prefix.0.join("drive_c/users/alice"));
    }

    #[tokio::test]
    async fn the_only_non_public_directory_is_the_fallback() {
        let prefix = TempDir::new();
        prefix.add_user("Public");
        prefix.add_user("carol");
        let found = find_user_dir(&prefix.0, &[]).await.unwrap();
        assert_eq!(found, prefix.0.join("drive_c/users/carol"));
    }

    #[tokio::test]
    async fn an_ambiguous_or_empty_prefix_has_no_user_directory() {
        let ambiguous = TempDir::new();
        ambiguous.add_user("Public");
        ambiguous.add_user("carol");
        ambiguous.add_user("dave");
        let missing = TempDir::new();
        let only_public = TempDir::new();
        only_public.add_user("Public");

        for prefix in [&ambiguous, &missing, &only_public] {
            let err = find_user_dir(&prefix.0, &[]).await.unwrap_err();
            assert!(matches!(err, SavesError::WineUserDirNotFound(_)), "{err}");
        }
    }

    #[tokio::test]
    async fn a_hostile_host_user_name_cannot_leave_drive_c_users() {
        let prefix = TempDir::new();
        prefix.add_user("carol");
        let found = find_user_dir(&prefix.0, &["../..".to_string(), "a/b".to_string()])
            .await
            .unwrap();
        assert_eq!(found, prefix.0.join("drive_c/users/carol"));
    }
}
