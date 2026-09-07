use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::{ProtonError, proton::GithubAsset};

/// A single Proton-GE release, as returned inside
/// [`ProtonGeReleasesPage`](crate::ProtonGeReleasesPage). Mirrors (a subset
/// of) GitHub's release object directly — see
/// [`GogDl::get_proton_releases`](crate::GogDl::get_proton_releases).
#[derive(Deserialize)]
pub struct ProtonGeRelease {
    /// GitHub's API URL for this release (not a browser-facing page).
    pub url: String,
    /// The release's tag, e.g. `"GE-Proton11-6"` — this is the
    /// human-readable Proton-GE version string.
    pub tag_name: String,
    /// GitHub's numeric release ID.
    pub id: i32,
    /// When the underlying git tag was created.
    pub created_at: DateTime<Utc>,
    /// When the release was last edited on GitHub.
    pub updated_at: DateTime<Utc>,
    /// When the release itself was published — distinct from `created_at`,
    /// which is the tag's creation date.
    pub published_at: DateTime<Utc>,
    /// The downloadable files attached to this release: per-architecture
    /// tarballs and their detached checksum files. See [`GithubAsset`] for
    /// how to pick the right one.
    pub assets: Vec<GithubAsset>,
}

impl ProtonGeRelease {
    /// Returns the first suitable asset for this release, or an error if no
    /// suitable asset is found.
    pub fn get_suitable_asset(&self) -> Result<&GithubAsset, ProtonError> {
        let asset = match self
            .assets
            .iter()
            .find(|asset| asset.name.ends_with(".tar.gz") && asset.name.contains("x86_64"))
        {
            Some(asset) => asset,
            None => return Err(ProtonError::NoSuitableAsset(self.tag_name.clone())),
        };
        Ok(asset)
    }

    /// Returns the size of the release, in bytes.
    pub fn get_release_size(&self) -> Result<u64, ProtonError> {
        let asset = self.get_suitable_asset()?;
        Ok(asset.size)
    }
}
