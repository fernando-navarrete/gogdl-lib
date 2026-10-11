use std::{
    collections::{BTreeSet, HashSet},
    io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::{
    downloader::{DownloadError, ProductBundle},
    fs::{FileSystemError, install_relative_path},
};

/// What [`GogDl::remove_products`](crate::GogDl::remove_products) deleted.
///
/// Counts only what that call removed: files already gone, and files a product being kept also
/// lists, are in neither number.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProductRemoval {
    /// Files deleted from the install.
    pub files_removed: u64,
    /// Their combined size on disk, in bytes.
    pub bytes_freed: u64,
}

fn removal_error(err: io::Error) -> DownloadError {
    DownloadError::FileSystemError(FileSystemError::FileRemovalError(err))
}

/// The key two manifest paths share when they name the same file: the install-relative path,
/// lowercased.
fn key(raw: &str) -> Option<(String, PathBuf)> {
    let relative = install_relative_path(raw).ok()?;
    Some((relative.to_string_lossy().to_lowercase(), relative))
}

/// Deletes from `base` the files only `remove`'s manifests list, then the directories that left
/// empty.
///
/// Every step is a sequential `tokio::fs` call inside this future, so dropping it stops it where
/// it is, and a second call finishes: it deletes what is left and prunes the same directories.
pub async fn remove_files(
    base: &str,
    keep: &[ProductBundle],
    remove: &[ProductBundle],
) -> Result<ProductRemoval, DownloadError> {
    let base = match fs::canonicalize(base).await {
        Ok(base) => base,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(ProductRemoval::default()),
        Err(err) => return Err(removal_error(err)),
    };

    let kept: HashSet<String> = keep
        .iter()
        .flat_map(|bundle| &bundle.product_files)
        .filter_map(|file| key(&file.path).map(|(key, _)| key))
        .collect();

    let mut seen = HashSet::new();
    let mut removal = ProductRemoval::default();
    let mut dirs = BTreeSet::new();

    for file in remove.iter().flat_map(|bundle| &bundle.product_files) {
        // An entry with no chunks is a directory marker, not a file.
        if file.chunks.is_none() {
            continue;
        }
        let Some((key, relative)) = key(&file.path) else {
            continue;
        };
        if kept.contains(&key) || !seen.insert(key) {
            continue;
        }
        let (Some(parent), Some(name)) = (relative.parent(), relative.file_name()) else {
            continue;
        };

        // Resolve the parent so a symlinked directory can't lead outside the install.
        let parent = match fs::canonicalize(base.join(parent)).await {
            Ok(parent) => parent,
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(err) => return Err(removal_error(err)),
        };
        if !parent.starts_with(&base) {
            continue;
        }
        let target = parent.join(name);

        let metadata = match fs::symlink_metadata(&target).await {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                dirs.insert(parent);
                continue;
            }
            Err(err) => return Err(removal_error(err)),
        };
        dirs.insert(parent);
        if metadata.is_dir() {
            continue;
        }
        match fs::remove_file(&target).await {
            Ok(()) => {
                removal.files_removed += 1;
                removal.bytes_freed += metadata.len();
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(removal_error(err)),
        }
    }

    prune(&base, dirs).await?;
    Ok(removal)
}

/// Removes each of `dirs`, and their ancestors below `base`, that is empty, deepest first.
async fn prune(base: &Path, dirs: BTreeSet<PathBuf>) -> Result<(), DownloadError> {
    let mut all: BTreeSet<&Path> = BTreeSet::new();
    for dir in &dirs {
        all.extend(
            dir.ancestors()
                .take_while(|ancestor| *ancestor != base && ancestor.starts_with(base)),
        );
    }
    let mut all: Vec<&Path> = all.into_iter().collect();
    all.sort_by_key(|dir| std::cmp::Reverse(dir.components().count()));

    for dir in all {
        match fs::remove_dir(dir).await {
            Ok(()) => {}
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty
                ) => {}
            Err(err) => return Err(removal_error(err)),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        depot::{Chunk, DepotFile},
        test_support::TempDir,
    };

    fn file(path: &str) -> DepotFile {
        DepotFile {
            md5: None,
            path: path.to_string(),
            chunks: Some(vec![Chunk {
                md5: String::new(),
                size: 0,
                compressed_md5: String::new(),
                compressed_size: 0,
            }]),
            file_type: "DepotFile".to_string(),
        }
    }

    fn bundle(id: &str, paths: &[&str]) -> Vec<ProductBundle> {
        vec![ProductBundle {
            product_id: id.to_string(),
            product_files: paths.iter().map(|p| file(p)).collect(),
        }]
    }

    fn write(root: &Path, relative: &str, bytes: &[u8]) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn base(tmp: &TempDir) -> &str {
        tmp.path().to_str().unwrap()
    }

    #[tokio::test]
    async fn a_dlc_sharing_files_with_the_base_game_keeps_them() {
        let tmp = TempDir::new();
        write(tmp.path(), "DATA/TEXT/EN/DLC01_FRONTEND.TXT", b"shared");
        write(tmp.path(), "DLC01/pack.bin", b"12345");
        write(tmp.path(), "DLC01/sub/more.bin", b"123");
        write(tmp.path(), "game.exe", b"exe");

        let keep = bundle("1", &["game.exe", "DATA\\TEXT\\EN\\DLC01_FRONTEND.TXT"]);
        let remove = bundle(
            "2",
            &[
                "data\\text\\en\\dlc01_frontend.txt",
                "DLC01\\pack.bin",
                "DLC01\\sub\\more.bin",
            ],
        );
        let removal = remove_files(base(&tmp), &keep, &remove).await.unwrap();

        assert_eq!(
            removal,
            ProductRemoval {
                files_removed: 2,
                bytes_freed: 8
            }
        );
        assert!(tmp.path().join("DATA/TEXT/EN/DLC01_FRONTEND.TXT").exists());
        assert!(tmp.path().join("game.exe").exists());
        assert!(!tmp.path().join("DLC01").exists());
    }

    #[tokio::test]
    async fn a_symlinked_directory_never_leads_outside_the_install() {
        let tmp = TempDir::new();
        let outside = TempDir::new();
        write(outside.path(), "secret.txt", b"keep me");
        std::fs::create_dir_all(tmp.path().join("install")).unwrap();
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("install/DLC")).unwrap();

        let remove = bundle("2", &["DLC/secret.txt"]);
        let removal = remove_files(tmp.path().join("install").to_str().unwrap(), &[], &remove)
            .await
            .unwrap();

        assert_eq!(removal, ProductRemoval::default());
        assert!(outside.path().join("secret.txt").exists());
    }

    #[tokio::test]
    async fn dot_dot_components_stay_inside_the_install() {
        let tmp = TempDir::new();
        std::fs::create_dir_all(tmp.path().join("install")).unwrap();
        write(tmp.path(), "victim.txt", b"outside");
        write(tmp.path(), "install/victim.txt", b"inside");

        let remove = bundle("2", &["..\\..\\victim.txt"]);
        let removal = remove_files(tmp.path().join("install").to_str().unwrap(), &[], &remove)
            .await
            .unwrap();

        assert_eq!(removal.files_removed, 1);
        assert!(tmp.path().join("victim.txt").exists());
        assert!(!tmp.path().join("install/victim.txt").exists());
    }

    #[tokio::test]
    async fn missing_files_and_a_missing_install_are_success() {
        let tmp = TempDir::new();
        let remove = bundle("2", &["gone/a.bin"]);
        assert_eq!(
            remove_files(base(&tmp), &[], &remove).await.unwrap(),
            ProductRemoval::default()
        );

        let absent = tmp.path().join("never");
        assert_eq!(
            remove_files(absent.to_str().unwrap(), &[], &remove)
                .await
                .unwrap(),
            ProductRemoval::default()
        );
        assert!(!absent.exists());
    }

    #[tokio::test]
    async fn a_directory_with_other_files_stays_and_the_base_is_never_removed() {
        let tmp = TempDir::new();
        write(tmp.path(), "DLC/pack.bin", b"1");
        write(tmp.path(), "DLC/user_config.ini", b"mine");
        write(tmp.path(), "top.bin", b"1");

        let remove = bundle("2", &["DLC/pack.bin", "top.bin"]);
        let removal = remove_files(base(&tmp), &[], &remove).await.unwrap();

        assert_eq!(removal.files_removed, 2);
        assert!(tmp.path().join("DLC/user_config.ini").exists());
        assert!(tmp.path().exists());
    }

    /// A dropped call leaves some files removed and its directories behind; the next call
    /// removes the rest, prunes, and counts only what it removed itself.
    #[tokio::test]
    async fn a_second_call_finishes_what_a_dropped_one_left() {
        let tmp = TempDir::new();
        write(tmp.path(), "DLC/a/one.bin", b"11");
        write(tmp.path(), "DLC/b/two.bin", b"2222");
        std::fs::remove_file(tmp.path().join("DLC/a/one.bin")).unwrap();

        let remove = bundle("2", &["DLC/a/one.bin", "DLC/b/two.bin"]);
        let removal = remove_files(base(&tmp), &[], &remove).await.unwrap();

        assert_eq!(
            removal,
            ProductRemoval {
                files_removed: 1,
                bytes_freed: 4
            }
        );
        assert!(!tmp.path().join("DLC").exists());
    }
}
