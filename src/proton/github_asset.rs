use serde::Deserialize;

#[derive(Deserialize)]
pub struct GithubAsset {
    pub name: String,
    pub browser_download_url: String,
}
