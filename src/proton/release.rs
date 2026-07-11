use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc::UnboundedSender;

use crate::proton::{error::ProtonError, proton_manager::ProtonManager};

/// GloriousEggroll's Proton-GE builds; releases are fetched from this repo's
/// GitHub Releases API.
const PROTON_GE_REPO: &str = "GloriousEggroll/proton-ge-custom";

/// One asset attached to a GitHub release. Only the fields Proton-GE download
/// needs are modeled; unknown fields are ignored by serde.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Asset {
    pub browser_download_url: String,
    pub name: String,
    pub content_type: String,
    pub digest: Option<String>,
    pub size: u64,
}

/// One Proton-GE release, as returned by GitHub's releases API.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Release {
    pub url: String,
    pub tag_name: String,
    pub assets: Vec<Asset>,
}

/// Cumulative transfer progress for a Proton-GE release download, sent over
/// the caller's `mpsc::UnboundedSender<ProtonProgress>` as the download
/// proceeds. `total` is 0 if the server didn't report a size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtonProgress {
    pub transferred: u64,
    pub total: u64,
}

fn verify_sha256(data: &[u8], expected_digest: &str) -> Result<(), ProtonError> {
    // GitHub formats asset digests as `sha256:<hex>`; strip the algorithm
    // prefix before comparing against our own hash.
    let expected_hex = expected_digest.strip_prefix("sha256:").unwrap_or(expected_digest);
    let actual = hex::encode(Sha256::digest(data));
    if actual != expected_hex {
        Err(ProtonError::HashMismatch {
            expected: expected_hex.to_string(),
            actual,
        })
    } else {
        Ok(())
    }
}

impl Release {
    /// The Proton-GE tarball we want is the `*.tar.gz` asset for x86_64 —
    /// releases also carry an `-aarch64.tar.gz` build (for ARM hosts) and a
    /// detached checksum file, neither of which we want here.
    fn tarball_asset(&self) -> Option<&Asset> {
        self.assets
            .iter()
            .find(|asset| asset.name.ends_with(".tar.gz") && !asset.name.contains("aarch64"))
    }
    pub fn get_download_link(&self) -> Option<&str> {
        self.tarball_asset().map(|asset| asset.browser_download_url.as_str())
    }
    pub fn get_download_size(&self) -> Option<u64> {
        self.tarball_asset().map(|asset| asset.size)
    }
    pub fn get_checksum(&self) -> Option<&str> {
        self.tarball_asset().and_then(|asset| asset.digest.as_deref())
    }
    /// Lists Proton-GE releases from GitHub, one page at a time. GitHub
    /// rejects unauthenticated requests without a `User-Agent`, so both that
    /// and an explicit `Accept` are sent alongside every request.
    pub async fn get_releases(
        manager: &ProtonManager,
        page: i32,
    ) -> Result<Vec<Release>, ProtonError> {
        let url = format!("https://api.github.com/repos/{PROTON_GE_REPO}/releases?page={page}");
        let releases = manager
            .client
            .get_json_with_headers(
                &url,
                &[
                    ("User-Agent", "gogdl-lib2"),
                    ("Accept", "application/vnd.github.v3+json"),
                ],
            )
            .await?;
        Ok(releases)
    }
    /// Downloads this release's Proton-GE tarball, verifies its SHA-256
    /// checksum (if GitHub provided one), and extracts it to `path`.
    /// Extraction runs on a blocking thread since a Proton-GE tarball is
    /// hundreds of MB and gzip/tar decoding is CPU- and IO-bound, not async.
    pub async fn download(
        &self,
        manager: &ProtonManager,
        path: &Path,
        tx: UnboundedSender<ProtonProgress>,
    ) -> Result<(), ProtonError> {
        let url = self
            .get_download_link()
            .ok_or(ProtonError::NoDownloadAsset)?
            .to_string();

        let bytes = manager
            .client
            .download_bytes(&url, &[("User-Agent", "gogdl-lib2")], move |transferred, total| {
                tx.send(ProtonProgress { transferred, total }).ok();
            })
            .await?;

        if let Some(checksum) = self.get_checksum() {
            verify_sha256(&bytes, checksum)?;
        }

        let path = path.to_path_buf();
        extract_tarball(bytes, path).await
    }
}

async fn extract_tarball(bytes: Vec<u8>, path: PathBuf) -> Result<(), ProtonError> {
    tokio::task::spawn_blocking(move || {
        let gz = GzDecoder::new(&bytes[..]);
        let mut archive = tar::Archive::new(gz);
        archive.unpack(&path)?;
        Ok::<(), std::io::Error>(())
    })
    .await
    .map_err(|join_err| ProtonError::Io(std::io::Error::other(join_err.to_string())))??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release_with_assets(assets: Vec<(&str, &str, Option<&str>)>) -> Release {
        Release {
            url: String::new(),
            tag_name: "GE-Proton1-1".to_string(),
            assets: assets
                .into_iter()
                .map(|(name, url, digest)| Asset {
                    browser_download_url: url.to_string(),
                    name: name.to_string(),
                    content_type: "application/octet-stream".to_string(),
                    digest: digest.map(|d| d.to_string()),
                    size: 1024,
                })
                .collect(),
        }
    }

    #[test]
    fn get_download_link_picks_the_gz_asset() {
        let release = release_with_assets(vec![
            ("GE-Proton1-1.sha512sum", "https://example.com/sha512sum", None),
            ("GE-Proton1-1.tar.gz", "https://example.com/tarball", Some("sha256:abc")),
        ]);
        assert_eq!(release.get_download_link(), Some("https://example.com/tarball"));
        assert_eq!(release.get_checksum(), Some("sha256:abc"));
    }

    #[test]
    fn get_download_link_skips_the_aarch64_tarball() {
        let release = release_with_assets(vec![
            ("GE-Proton1-1-aarch64.sha512sum", "https://example.com/aarch64-sha512sum", None),
            ("GE-Proton1-1-aarch64.tar.gz", "https://example.com/aarch64-tarball", Some("sha256:aarch64")),
            ("GE-Proton1-1.sha512sum", "https://example.com/sha512sum", None),
            ("GE-Proton1-1.tar.gz", "https://example.com/tarball", Some("sha256:abc")),
        ]);
        assert_eq!(release.get_download_link(), Some("https://example.com/tarball"));
        assert_eq!(release.get_checksum(), Some("sha256:abc"));
    }

    #[test]
    fn get_download_link_returns_none_without_a_gz_asset() {
        let release = release_with_assets(vec![("checksum.sha512sum", "https://example.com/x", None)]);
        assert_eq!(release.get_download_link(), None);
    }

    #[test]
    fn verify_sha256_ok_when_matching() {
        let data = b"hello world";
        let hex = hex::encode(Sha256::digest(data));
        assert!(verify_sha256(data, &format!("sha256:{hex}")).is_ok());
    }

    #[test]
    fn verify_sha256_errors_when_mismatched() {
        let data = b"hello world";
        let result = verify_sha256(data, "sha256:0000000000000000000000000000000000000000000000000000000000000000");
        match result {
            Err(ProtonError::HashMismatch { expected, actual }) => {
                assert_ne!(expected, actual);
            }
            other => panic!("expected HashMismatch, got {other:?}"),
        }
    }
}
