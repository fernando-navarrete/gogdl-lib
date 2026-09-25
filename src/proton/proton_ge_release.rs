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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProtonGeReleasesPage;

    fn release(assets: &[&str]) -> ProtonGeRelease {
        let assets: Vec<_> = assets
            .iter()
            .map(|name| {
                serde_json::json!({
                    "name": name,
                    "browser_download_url": format!("https://example.invalid/{name}"),
                    "size": name.len(),
                })
            })
            .collect();
        serde_json::from_value(serde_json::json!({
            "url": "https://api.github.com/repos/o/r/releases/1",
            "tag_name": "GE-Proton0-0",
            "id": 1,
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "published_at": "2026-01-01T00:00:00Z",
            "assets": assets,
        }))
        .unwrap()
    }

    #[test]
    fn a_captured_releases_page_deserializes() {
        let page: ProtonGeReleasesPage = serde_json::from_str(include_str!(
            "../../tests/fixtures/proton_ge_releases_page.json"
        ))
        .unwrap();
        let tags: Vec<_> = page
            .releases()
            .iter()
            .map(|r| r.tag_name.as_str())
            .collect();
        assert_eq!(tags, ["GE-Proton11-7", "GE-Proton11-6"]);
        assert!(page.releases()[0].published_at > page.releases()[1].published_at);
        assert_eq!(page.releases()[0].assets.len(), 4);
    }

    #[test]
    fn a_captured_release_picks_the_x86_64_tarball() {
        let release: ProtonGeRelease =
            serde_json::from_str(include_str!("../../tests/fixtures/proton_ge_release.json"))
                .unwrap();
        // The capture lists the aarch64 tarball and both `.sha512sum` files
        // before the x86_64 tarball.
        assert_eq!(release.assets[0].name, "GE-Proton11-7-aarch64.sha512sum");
        let asset = release.get_suitable_asset().unwrap();
        assert_eq!(asset.name, "GE-Proton11-7-x86_64.tar.gz");
        assert_eq!(release.get_release_size().unwrap(), asset.size);
    }

    #[test]
    fn without_a_linux_tarball_there_is_no_suitable_asset() {
        let release = release(&[
            "a-aarch64.tar.gz",
            "a-x86_64.sha512sum",
            "a-aarch64.sha512sum",
        ]);
        assert!(matches!(
            release.get_suitable_asset(),
            Err(ProtonError::NoSuitableAsset(tag)) if tag == "GE-Proton0-0"
        ));
        assert!(release.get_release_size().is_err());
    }
}
