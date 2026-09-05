use crate::{ProtonError, ProtonGeRelease, client::HttpClient};

pub struct ProtonDownloader {
    pub client: HttpClient,
}

impl ProtonDownloader {
    pub fn new(client: HttpClient) -> Self {
        Self { client }
    }
    pub async fn download_proton_release(
        &self,
        release: &ProtonGeRelease,
    ) -> Result<(), ProtonError> {
        let download_url = match release
            .assets
            .iter()
            .find(|asset| asset.name.contains("x86_64") && asset.name.ends_with(".tar.gz"))
        {
            Some(asset) => asset.browser_download_url.clone(),
            None => return Err(ProtonError::NoSuitableAsset(release.tag_name.clone())),
        };
        todo!()
    }
}
