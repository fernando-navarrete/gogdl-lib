//! Sanitizes and resolves untrusted relative paths (e.g. from a GOG depot
//! manifest) against a fixed install base directory, caching per-directory
//! validation so repeated calls for files in the same directory are cheap
//! and race-free under concurrent access.
//!
//! Add to Cargo.toml:
//!   dashmap = "6"
//!   tokio = { version = "1", features = ["fs", "sync", "rt"] }

use dashmap::DashMap;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use tokio::sync::OnceCell;

/// Resolves raw (untrusted) directory + filename strings from a manifest
/// into safe, validated paths under a fixed base directory.
///
/// Cheap to call per-file: directory validation (create_dir_all +
/// canonicalize + containment check) happens at most once per unique
/// raw directory string, memoized via `OnceCell`. Concurrent callers
/// resolving the same directory await the same in-flight validation
/// instead of racing.
pub struct PathResolver {
    canonical_base: PathBuf,
    dir_cache: DashMap<String, Arc<OnceCell<PathBuf>>>,
}

impl PathResolver {
    /// Creates a new resolver rooted at `base`. Creates `base` if it
    /// doesn't exist yet.
    pub async fn new(base: PathBuf) -> io::Result<Self> {
        fs::create_dir_all(&base).await?;
        let canonical_base = fs::canonicalize(&base).await?;
        Ok(Self {
            canonical_base,
            dir_cache: DashMap::new(),
        })
    }

    /// Resolves a single file's full path given its raw directory and
    /// filename strings as they appear in the manifest. Safe to call
    /// once per `DownloadableFile`, including concurrently from multiple
    /// tasks (e.g. inside a `buffer_unordered` stream).
    pub async fn resolve(&self, raw_dir: &str, raw_filename: &str) -> io::Result<PathBuf> {
        let cell = self
            .dir_cache
            .entry(raw_dir.to_string())
            .or_insert_with(|| Arc::new(OnceCell::new()))
            .clone();

        let validated_dir: &PathBuf = cell
            .get_or_try_init(|| self.validate_directory(raw_dir))
            .await?;

        let safe_filename = sanitize_filename(raw_filename);
        if safe_filename.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("empty filename after sanitizing: {raw_filename:?}"),
            ));
        }

        Ok(validated_dir.join(safe_filename))
    }

    /// Resolves a single file's full path given one combined relative
    /// path as it appears in the manifest (e.g. "docs/manuals/readme.txt").
    /// Splits internally into parent directory + filename, so directory
    /// validation is still cached per-parent even though callers only
    /// ever pass one string per file.
    pub async fn resolve_path(&self, raw_relative_path: &str) -> io::Result<PathBuf> {
        let normalized = normalize_separators(raw_relative_path);
        let path = Path::new(normalized.as_ref());

        let raw_dir = path
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();

        let raw_filename = path
            .file_name()
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("path has no filename component: {raw_relative_path:?}"),
                )
            })?
            .to_string_lossy()
            .into_owned();

        self.resolve(&raw_dir, &raw_filename).await
    }

    /// Resolves a single file's path for reading/verification purposes,
    /// WITHOUT creating any directories — used for importing an
    /// already-installed game, where files are expected to already exist.
    /// Returns `Ok(None)` if the file doesn't exist rather than erroring,
    /// so callers can treat "missing" as a normal verification outcome.
    pub async fn resolve_existing_path(
        &self,
        raw_relative_path: &str,
    ) -> io::Result<Option<PathBuf>> {
        let normalized = normalize_separators(raw_relative_path);
        let path = Path::new(normalized.as_ref());

        let raw_dir = path
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let raw_filename = path
            .file_name()
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("path has no filename component: {raw_relative_path:?}"),
                )
            })?
            .to_string_lossy()
            .into_owned();

        let sanitized_dir = sanitize_relative_path(&raw_dir);
        let safe_filename = sanitize_filename(&raw_filename);
        if safe_filename.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("empty filename after sanitizing: {raw_filename:?}"),
            ));
        }

        let candidate = self
            .canonical_base
            .join(&sanitized_dir)
            .join(&safe_filename);

        if !fs::try_exists(&candidate).await? {
            return Ok(None);
        }

        // canonicalize requires the path to exist, which we've just
        // confirmed — this also resolves any symlinks before the
        // containment check.
        let canonical = fs::canonicalize(&candidate).await?;
        if !canonical.starts_with(&self.canonical_base) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("path escapes base directory: {raw_relative_path:?}"),
            ));
        }

        Ok(Some(canonical))
    }

    /// Validates (and creates) a single directory once. Only reachable
    /// through the `OnceCell` in `resolve`, so this body runs at most
    /// once per unique `raw_dir`, even under concurrent access.
    async fn validate_directory(&self, raw_dir: &str) -> io::Result<PathBuf> {
        let sanitized = sanitize_relative_path(raw_dir);
        let joined = self.canonical_base.join(&sanitized);

        fs::create_dir_all(&joined).await?;
        let canonical = fs::canonicalize(&joined).await?;

        if !canonical.starts_with(&self.canonical_base) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("directory escapes base: {raw_dir:?}"),
            ));
        }

        Ok(canonical)
    }
}

/// Converts Windows-style backslash separators to forward slashes.
/// GOG depot manifests are Windows-authored and use `\` as the path
/// separator, but `std::path::Path` on Unix does not treat `\` as a
/// separator at all — without this normalization, a whole nested path
/// like `movies\ep1\file.bin` is parsed as one giant single component,
/// which then gets flattened by `sanitize_filename`'s separator-stripping
/// (which exists to guard against a *single* component accidentally
/// containing a separator, not to do this splitting job).
fn normalize_separators(input: &str) -> std::borrow::Cow<'_, str> {
    if input.contains('\\') {
        std::borrow::Cow::Owned(input.replace('\\', "/"))
    } else {
        std::borrow::Cow::Borrowed(input)
    }
}

/// Sanitizes a relative path made of possibly-multiple components (e.g.
/// "docs/manuals/../../secret") by dropping any component that isn't a
/// plain name: `..`, `.`, root prefixes, and drive prefixes are all
/// discarded rather than interpreted, which is what actually prevents
/// path traversal.
fn sanitize_relative_path(input: &str) -> PathBuf {
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
        // RootDir, ParentDir (..), CurDir (.), Prefix (C:\) are all dropped.
    }

    safe
}

/// Sanitizes a single path component (one file or directory name, no
/// separators expected). Strips characters invalid on Windows/FAT and
/// control characters, trims trailing dots/spaces, and disarms
/// Windows-reserved device names.
fn sanitize_filename(name: &str) -> String {
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

    #[tokio::test]
    async fn resolves_simple_file() {
        let tmp = std::env::temp_dir().join(format!("resolver_test_{}", std::process::id()));
        let resolver = PathResolver::new(tmp.clone()).await.unwrap();

        let path = resolver.resolve("bin", "game.exe").await.unwrap();
        assert!(path.starts_with(&resolver.canonical_base));
        assert_eq!(path.file_name().unwrap(), "game.exe");

        let _ = fs::remove_dir_all(&tmp).await;
    }

    #[tokio::test]
    async fn rejects_path_traversal() {
        let tmp = std::env::temp_dir().join(format!("resolver_test_trav_{}", std::process::id()));
        let resolver = PathResolver::new(tmp.clone()).await.unwrap();

        // "../../../etc" should collapse to just "etc" as a single
        // sanitized component, never escaping the base.
        let path = resolver.resolve("../../../etc", "passwd").await.unwrap();
        assert!(path.starts_with(&resolver.canonical_base));

        let _ = fs::remove_dir_all(&tmp).await;
    }

    #[tokio::test]
    async fn root_level_files_use_base_dir() {
        let tmp = std::env::temp_dir().join(format!("resolver_test_root_{}", std::process::id()));
        let resolver = PathResolver::new(tmp.clone()).await.unwrap();

        let path = resolver.resolve("", "readme.txt").await.unwrap();
        assert_eq!(path.parent().unwrap(), resolver.canonical_base);

        let _ = fs::remove_dir_all(&tmp).await;
    }

    #[tokio::test]
    async fn sanitizes_reserved_windows_names() {
        let tmp = std::env::temp_dir().join(format!("resolver_test_res_{}", std::process::id()));
        let resolver = PathResolver::new(tmp.clone()).await.unwrap();

        let path = resolver.resolve("data", "CON.dat").await.unwrap();
        assert_eq!(path.file_name().unwrap(), "_CON.dat");

        let _ = fs::remove_dir_all(&tmp).await;
    }

    #[tokio::test]
    async fn concurrent_resolves_same_dir_dont_race() {
        let tmp = std::env::temp_dir().join(format!("resolver_test_conc_{}", std::process::id()));
        let resolver = Arc::new(PathResolver::new(tmp.clone()).await.unwrap());

        let mut handles = vec![];
        for i in 0..20 {
            let resolver = Arc::clone(&resolver);
            handles.push(tokio::spawn(async move {
                resolver
                    .resolve("shared_dir", &format!("file_{i}.bin"))
                    .await
                    .unwrap()
            }));
        }
        for h in handles {
            h.await.unwrap();
        }

        let _ = fs::remove_dir_all(&tmp).await;
    }
}
