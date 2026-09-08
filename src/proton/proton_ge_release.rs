use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::{
    ProtonError,
    proton::{GithubAsset, ProtonManager},
};

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
            .find(|asset| asset.name.ends_with(".tar.gz") && !asset.name.contains("aarch64"))
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

    /// Fetches a single release from
    /// `GET /repos/GloriousEggroll/proton-ge-custom/releases/tags/{tag}`,
    /// with the same `Accept`/`User-Agent`/`X-GitHub-Api-Version` headers as
    /// [`ProtonGeReleasesPage`](crate::ProtonGeReleasesPage)'s page fetch.
    /// Not cached. See
    /// [`GogDl::get_proton_release_by_tag`](crate::GogDl::get_proton_release_by_tag)
    /// for the public-facing contract, including the `404`-on-unknown-`tag`
    /// behavior that surfaces here as a [`ProtonError`].
    pub async fn get_by_tag(
        tag: &str,
        proton_manager: &ProtonManager,
    ) -> Result<ProtonGeRelease, ProtonError> {
        let url = format!(
            "https://api.github.com/repos/GloriousEggroll/proton-ge-custom/releases/tags/{}",
            tag
        );
        let release: ProtonGeRelease = proton_manager
            .client
            .fetch(
                &url,
                false,
                false,
                Some(&[
                    ("Accept", "application/vnd.github.v3+json"),
                    ("User-Agent", "gogdl"),
                    ("X-GitHub-Api-Version", "2026-03-10"),
                ]),
            )
            .await?;

        Ok(release)
    }
}
