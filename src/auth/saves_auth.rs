use serde::{Deserialize, Serialize};

/// Token pair scoped to one game's cloud-save storage on
/// `cloudstorage.gog.com`. Minted from the user's regular refresh token
/// exchanged against that game's own `client_id`/`client_secret` (see
/// `AuthManager::get_cloud_saves_tokens`) — distinct from `Auth`, which
/// authenticates against the rest of the GOG API.
///
/// No `Debug` derive, matching `Auth`: both carry a bearer token that
/// shouldn't end up in a log line via `{:?}`.
#[derive(Deserialize, Serialize, Clone)]
pub struct SavesAuth {
    pub access_token: String,
    pub user_id: String,
    #[serde(skip)]
    pub client_id: String,
}
