use dashmap::DashMap;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use sysinfo::Disks;
use tokio::fs::{self, File};
use tokio::sync::OnceCell;

use crate::fs::error::FileSystemError;

pub struct PathResolver {
    canonical_base: PathBuf,
    dir_cache: DashMap<String, Arc<OnceCell<PathBuf>>>,
}

impl PathResolver {
    pub async fn new(base: PathBuf) -> Result<Self, FileSystemError> {
        if let Err(e) = fs::create_dir_all(&base).await {
            return Err(FileSystemError::PathResolverCreationError(e));
        };
        let canonical_base = match fs::canonicalize(&base).await {
            Ok(canonical_base) => canonical_base,
            Err(e) => return Err(FileSystemError::PathResolverCreationError(e)),
        };
        Ok(Self {
            canonical_base,
            dir_cache: DashMap::new(),
        })
    }

    /// The canonicalized base directory this resolver was constructed with.
    pub fn base(&self) -> &Path {
        &self.canonical_base
    }

    pub async fn get_file_size(&self, path: &Path) -> Result<u64, FileSystemError> {
        let metadata = match fs::metadata(path).await {
            Ok(metadata) => metadata,
            Err(e) => return Err(FileSystemError::FileMetadataError(e)),
        };
        Ok(metadata.len())
    }

    pub fn get_free_space(&self) -> Result<u64, FileSystemError> {
        free_space_of(&self.canonical_base)
    }

    pub async fn allocate_file(
        &self,
        raw_relative_path: &str,
        total_size: u64,
    ) -> Result<PathBuf, FileSystemError> {
        let path = self.resolve_path(raw_relative_path).await?;
        let file = match fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .await
        {
            Ok(file) => file,
            Err(e) => return Err(FileSystemError::FileCreationError(e)),
        };
        if let Err(err) = file.set_len(total_size).await {
            return Err(FileSystemError::FileMetadataError(err));
        };
        Ok(path)
    }

    pub async fn resolve(
        &self,
        raw_dir: &str,
        raw_filename: &str,
    ) -> Result<PathBuf, FileSystemError> {
        let cell = self
            .dir_cache
            .entry(raw_dir.to_string())
            .or_insert_with(|| Arc::new(OnceCell::new()))
            .clone();

        let validated_dir: &PathBuf = match cell
            .get_or_try_init(|| self.validate_directory(raw_dir))
            .await
        {
            Ok(dir) => dir,
            Err(e) => return Err(e),
        };

        let safe_filename = sanitize_filename(raw_filename);
        if safe_filename.is_empty() {
            return Err(FileSystemError::PathResolutionError(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("empty filename after sanitizing: {raw_filename:?}"),
            )));
        }

        Ok(validated_dir.join(safe_filename))
    }

    pub async fn resolve_path(&self, raw_relative_path: &str) -> Result<PathBuf, FileSystemError> {
        let normalized = normalize_separators(raw_relative_path);
        let path = Path::new(normalized.as_ref());

        let raw_dir = path
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();

        let raw_filename = path
            .file_name()
            .ok_or_else(|| {
                FileSystemError::PathResolutionError(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("path has no filename component: {raw_relative_path:?}"),
                ))
            })?
            .to_string_lossy()
            .into_owned();

        self.resolve(&raw_dir, &raw_filename).await
    }
    pub async fn open_file(
        &self,
        raw_relative_path: &str,
        write: bool,
    ) -> Result<File, FileSystemError> {
        let path = self.resolve_path(raw_relative_path).await?;
        match tokio::fs::OpenOptions::new().write(write).open(&path).await {
            Ok(file) => Ok(file),
            Err(e) => Err(FileSystemError::FileOpenError(e)),
        }
    }

    pub async fn resolve_existing_path(
        &self,
        raw_relative_path: &str,
    ) -> Result<Option<PathBuf>, FileSystemError> {
        let normalized = normalize_separators(raw_relative_path);
        let path = Path::new(normalized.as_ref());

        let raw_dir = path
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let raw_filename = path
            .file_name()
            .ok_or_else(|| {
                FileSystemError::PathResolutionError(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("path has no filename component: {raw_relative_path:?}"),
                ))
            })?
            .to_string_lossy()
            .into_owned();

        let sanitized_dir = sanitize_relative_path(&raw_dir);
        let safe_filename = sanitize_filename(&raw_filename);
        if safe_filename.is_empty() {
            return Err(FileSystemError::PathResolutionError(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("empty filename after sanitizing: {raw_filename:?}"),
            )));
        }

        let candidate = self
            .canonical_base
            .join(&sanitized_dir)
            .join(&safe_filename);

        match fs::try_exists(&candidate).await {
            Ok(exists) => {
                if !exists {
                    return Ok(None);
                }
            }
            Err(err) => {
                return Err(FileSystemError::PathResolutionError(err));
            }
        }

        // canonicalize requires the path to exist, which we've just
        // confirmed — this also resolves any symlinks before the
        // containment check.
        let canonical = match fs::canonicalize(&candidate).await {
            Ok(canonical) => canonical,
            Err(err) => {
                return Err(FileSystemError::PathResolutionError(err));
            }
        };
        if !canonical.starts_with(&self.canonical_base) {
            return Err(FileSystemError::PathResolutionError(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("path escapes base directory: {raw_relative_path:?}"),
            )));
        }

        Ok(Some(canonical))
    }

    async fn validate_directory(&self, raw_dir: &str) -> Result<PathBuf, FileSystemError> {
        let sanitized = sanitize_relative_path(raw_dir);
        let joined = self.canonical_base.join(&sanitized);

        if let Err(err) = fs::create_dir_all(&joined).await {
            return Err(FileSystemError::PathResolutionError(err));
        }
        let canonical = match fs::canonicalize(&joined).await {
            Ok(canonical) => canonical,
            Err(err) => {
                return Err(FileSystemError::PathResolutionError(err));
            }
        };

        if !canonical.starts_with(&self.canonical_base) {
            return Err(FileSystemError::PathResolutionError(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("directory escapes base: {raw_dir:?}"),
            )));
        }

        Ok(canonical)
    }
}

/// Available bytes on the disk holding `path`, which needn't exist and is never created: the
/// nearest existing ancestor stands in for it. The same lookup as
/// [`PathResolver::get_free_space`], so a caller's pre-check and the download's own agree.
pub async fn free_space_at(path: &Path) -> Result<u64, FileSystemError> {
    free_space_of(&nearest_existing_ancestor(path).await?)
}

fn free_space_of(canonical: &Path) -> Result<u64, FileSystemError> {
    let disks = Disks::new_with_refreshed_list();
    let entries = disks
        .list()
        .iter()
        .map(|disk| (disk.mount_point(), disk.available_space()));

    pick_disk(canonical, entries)
        .ok_or_else(|| FileSystemError::NoDiskMatchingPath(canonical.to_path_buf()))
}

/// The canonical form of the longest prefix of `path` that exists. A relative path that runs out
/// of components resolves against the working directory, as `PathResolver::new` does.
async fn nearest_existing_ancestor(path: &Path) -> Result<PathBuf, FileSystemError> {
    for ancestor in path.ancestors() {
        let candidate = if ancestor.as_os_str().is_empty() {
            Path::new(".")
        } else {
            ancestor
        };
        match fs::canonicalize(candidate).await {
            Ok(canonical) => return Ok(canonical),
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(FileSystemError::PathResolutionError(e)),
        }
    }
    Err(FileSystemError::PathResolutionError(io::Error::new(
        io::ErrorKind::NotFound,
        format!("no existing ancestor of {path:?}"),
    )))
}

/// Available bytes of the disk holding `base`: the one with the longest mount point that is a
/// prefix of it. `/` prefixes every path, so taking the first match reads the wrong disk.
fn pick_disk<'a>(base: &Path, disks: impl IntoIterator<Item = (&'a Path, u64)>) -> Option<u64> {
    disks
        .into_iter()
        // `starts_with` compares components, so `/media/game` doesn't match `/media/gamedisk`.
        .filter(|(mount, _)| base.starts_with(mount))
        // On equal lengths `max_by_key` keeps the last, the later mount of an over-mounted path.
        .max_by_key(|(mount, _)| mount.components().count())
        .map(|(_, available)| available)
}

fn normalize_separators(input: &str) -> std::borrow::Cow<'_, str> {
    if input.contains('\\') {
        std::borrow::Cow::Owned(input.replace('\\', "/"))
    } else {
        std::borrow::Cow::Borrowed(input)
    }
}

pub fn sanitize_relative_path(input: &str) -> PathBuf {
    let normalized = normalize_separators(input);
    let path = Path::new(normalized.as_ref());
    let mut safe = PathBuf::new();

    for component in path.components() {
        if let Component::Normal(part) = component {
            let sanitized = sanitize_filename(&part.to_string_lossy());
            if !sanitized.is_empty() {
                safe.push(sanitized);
            }
        }
    }
    safe
}

pub fn sanitize_filename(name: &str) -> String {
    let mut result: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '|' | '?' | '*' | '\0' => '_',
            '/' | '\\' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();

    result = result.trim_end_matches([' ', '.']).to_string();
    result = result.trim_start().to_string();

    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let stem = result.split('.').next().unwrap_or("");
    if RESERVED.contains(&stem.to_uppercase().as_str()) {
        result = format!("_{result}");
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    /// In `/proc/mounts` order, as `sysinfo` lists them: `/` comes first.
    fn mounts() -> Vec<(&'static Path, u64)> {
        vec![
            (Path::new("/"), 1),
            (Path::new("/home"), 2),
            (Path::new("/media/gamedisk"), 3),
            (Path::new("/media/game"), 4),
        ]
    }

    #[test]
    fn picks_the_longest_matching_mount() {
        let table = [
            ("/media/gamedisk/Games/Foo", Some(3)),
            ("/media/gamedisk", Some(3)),
            ("/media/game/Foo", Some(4)),
            ("/home/user/Games", Some(2)),
            ("/opt/games", Some(1)),
        ];
        for (base, expected) in table {
            assert_eq!(pick_disk(Path::new(base), mounts()), expected, "{base}");
        }
    }

    #[test]
    fn no_matching_mount_is_none() {
        let without_root = mounts().into_iter().skip(1);
        assert_eq!(pick_disk(Path::new("/opt/games"), without_root), None);
        assert_eq!(pick_disk(Path::new("relative/dir"), mounts()), None);
    }

    #[test]
    fn an_over_mount_takes_the_later_entry() {
        let disks = [
            (Path::new("/"), 1),
            (Path::new("/mnt"), 5),
            (Path::new("/mnt"), 6),
        ];
        assert_eq!(pick_disk(Path::new("/mnt/x"), disks), Some(6));
    }

    #[tokio::test]
    async fn nearest_existing_ancestor_skips_missing_components() {
        let tmp = TempDir::new();
        let canonical_tmp = std::fs::canonicalize(tmp.path()).unwrap();
        let missing = tmp.path().join("a/b/c");
        assert_eq!(
            nearest_existing_ancestor(&missing).await.unwrap(),
            canonical_tmp
        );
        assert_eq!(
            nearest_existing_ancestor(tmp.path()).await.unwrap(),
            canonical_tmp
        );
    }

    #[tokio::test]
    async fn free_space_at_creates_nothing() {
        let tmp = TempDir::new();
        let target = tmp.path().join("a/b");
        free_space_at(&target).await.unwrap();
        assert!(!tmp.path().join("a").exists());
    }
}
