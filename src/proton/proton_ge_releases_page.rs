use serde::Deserialize;

use crate::proton::{ProtonGeRelease, ProtonManager, error::ProtonError};

#[derive(Deserialize)]
#[serde(transparent)]
pub struct ProtonGeReleasesPage {
    releases: Vec<ProtonGeRelease>,
}

impl ProtonGeReleasesPage {
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
