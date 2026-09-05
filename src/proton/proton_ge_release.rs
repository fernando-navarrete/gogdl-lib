use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::proton::GithubAsset;

#[derive(Deserialize)]
pub struct ProtonGeRelease {
    pub url: String,
    pub tag_name: String,
    pub id: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub published_at: DateTime<Utc>,
    pub assets: Vec<GithubAsset>,
}
