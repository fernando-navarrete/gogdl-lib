use serde::Deserialize;

use crate::proton::{ProtonGeRelease, ProtonManager, error::ProtonError};

/// One page of Proton-GE releases, as returned by
/// [`GogDl::get_proton_releases`](crate::GogDl::get_proton_releases).
/// Deserializes GitHub's response directly (`#[serde(transparent)]`), which
/// is a bare JSON array rather than an object.
///
/// `releases` is currently private with no accessor, so a consumer cannot
/// yet read anything out of a page they receive — the type exists to be
/// returned, not inspected.
#[derive(Deserialize)]
#[serde(transparent)]
pub struct ProtonGeReleasesPage {
    releases: Vec<ProtonGeRelease>,
}

impl ProtonGeReleasesPage {
    /// Not reachable from outside the crate — `ProtonManager` is not
    /// exported. Call
    /// [`GogDl::get_proton_releases`](crate::GogDl::get_proton_releases)
    /// instead, which delegates here internally.
    pub async fn get_releases_page(
        page: u32,
        per_page: u32,
        proton_manager: &ProtonManager,
    ) -> Result<Self, ProtonError> {
        let url = format!(
            "https://api.github.com/repos/GloriousEggroll/proton-ge-custom/releases?page={}&per_page={}",
            page, per_page
        );

        let releases_page: ProtonGeReleasesPage =
            proton_manager.client.fetch(&url, false, false).await?;

        Ok(releases_page)
    }
}
