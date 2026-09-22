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
        let disks = Disks::new_with_refreshed_list();

        let disk = disks
            .list()
            .iter()
            .find(|&disk| self.canonical_base.starts_with(disk.mount_point()));

        match disk {
            None => {
                return Err(FileSystemError::NoDiskMatchingPath(
                    self.canonical_base.clone(),
                ));
            }
            Some(disk) => {
                let free_space = disk.available_space();
                Ok(free_space)
            }
        }
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
