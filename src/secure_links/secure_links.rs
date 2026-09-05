use serde::{Deserialize, Serialize};
use url::Url;

use crate::secure_links::{SecureLinksManager, error::SecureLinksError};

/// The CDN's presigned-URL parameters for one endpoint, as returned by the
/// secure-link endpoint. Substituted into [`UrlFormat::url_format`]'s
/// placeholders by [`UrlFormat::parse_url`].
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CdnUrlParams {
    /// Base CDN URL the chunk path is resolved against.
    pub base_url: String,
    /// Path component of the presigned URL.
    pub path: String,
    /// Presigned-URL auth token.
    pub token: String,
    /// Absolute Unix timestamp the link expires at, if the CDN provided one.
    /// Checked by [`is_valid`](Self::is_valid); `None` means expiry can only
    /// be detected reactively, via a CDN 401.
    pub expires_at: Option<u64>,
    /// CDN sharding-depth hint, substituted into `{dirs}` if the URL template
    /// uses it.
    pub dirs: Option<u64>,
    /// Link lifetime in seconds, if the CDN provided one instead of (or
    /// alongside) `expires_at`. Parsed but currently unused: unlike
    /// `expires_at` this is a duration, not a deadline, and nothing records
    /// when the link was issued to add it to — see [`is_valid`](Self::is_valid).
    pub ttl: Option<u64>,
    /// CDN-specific source identifier, substituted into `{source}` if the
    /// URL template uses it.
    pub source: Option<String>,
    /// Secondary auth token some CDN endpoints require, substituted into
    /// `{gog_token}` if the URL template uses it.
    pub gog_token: Option<String>,
    /// Locale hint, substituted into `{l}` if the URL template uses it.
    pub l: Option<String>,
}

/// One CDN endpoint offering a given chunk, with the priority to pick among
/// several (see [`SecureLinks::get_highest_priority_url`]).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct UrlFormat {
    /// Human-readable CDN endpoint name, as reported by GOG.
    pub endpoint_name: String,
    /// URL template with `{path}`/`{token}`/`{base_url}`/... placeholders,
    /// filled in by [`parse_url`](Self::parse_url).
    pub url_format: String,
    /// Higher wins — see [`SecureLinks::get_highest_priority_url`].
    pub priority: u64,
    pub parameters: CdnUrlParams,
}

/// The set of CDN endpoints available for one product's chunks, as returned
/// by the secure-link endpoint and cached by
/// [`SecureLinksManager`](crate::secure_links::SecureLinksManager).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SecureLinks {
    /// GOG product ID these links serve.
    pub product_id: u64,
    /// Available CDN endpoints; pick with
    /// [`get_highest_priority_url`](Self::get_highest_priority_url).
    pub urls: Vec<UrlFormat>,
}

impl SecureLinks {
    /// Fetches a fresh secure-link set for `game_id` from GOG. Callers
    /// should generally go through
    /// [`SecureLinksManager::get_secure_links`](crate::secure_links::SecureLinksManager::get_secure_links)
    /// instead, which caches the result and checks expiry first.
    pub async fn get_secure_links(
        secure_links_manager: &SecureLinksManager,
        game_id: &str,
    ) -> Result<SecureLinks, SecureLinksError> {
        let url = format!(
            "https://content-system.gog.com/products/{}/secure_link?generation=2&_version=2&path=/",
            game_id
        );
        let secure_links: SecureLinks =
            secure_links_manager.client.fetch(&url, false, true).await?;

        Ok(secure_links)
    }
    /// The endpoint chunk downloads should use: the [`UrlFormat`] with the
    /// highest `priority`. Errs with [`SecureLinksError::NoSecureLink`] if
    /// `urls` is empty.
    pub fn get_highest_priority_url(&self) -> Result<&UrlFormat, SecureLinksError> {
        self.urls
            .iter()
            .max_by_key(|url| url.priority)
            .ok_or(SecureLinksError::NoSecureLink)
    }
    /// Whether this link set is still usable: it has a highest-priority URL,
    /// and that URL's [`CdnUrlParams::is_valid`] says it hasn't expired.
    /// `false` (rather than an error) for an empty `urls`, since either way
    /// the caller's only recourse is to fetch a fresh set.
    pub fn is_valid(&self) -> bool {
        self.get_highest_priority_url()
            .is_ok_and(|url| url.parameters.is_valid())
    }
}

impl UrlFormat {
    /// Builds the URL for a redistributable (non-depot) file identified by
    /// `chunk_hash`, sharded into the CDN's two-level hash-prefix directory
    /// layout.
    pub fn parse_url_redist(&self, chunk_hash: &str) -> String {
        let url = format!(
            "https://gog-cdn-fastly.gog.com/content-system/v2/dependencies/store/{}/{}/{}",
            &chunk_hash[0..2],
            &chunk_hash[2..4],
            chunk_hash
        );
        url
    }
    /// Builds the URL for a depot file's chunk identified by `chunk_hash`:
    /// fills in `url_format`'s placeholders from `parameters` and appends the
    /// hash-prefix-sharded chunk path to the URL's path component (ahead of
    /// any query string).
    pub fn parse_url(&self, chunk_hash: &str) -> String {
        let mut url = self.url_format.clone();
        url = url.replace("{path}", &self.parameters.path);
        url = url.replace("{token}", &self.parameters.token);
        url = url.replace("{base_url}", &self.parameters.base_url);

        if let Some(expires_at) = self.parameters.expires_at {
            url = url.replace("{expires_at}", &expires_at.to_string());
        }
        if let Some(dirs) = self.parameters.dirs {
            url = url.replace("{dirs}", &dirs.to_string());
        }
        if let Some(ttl) = self.parameters.ttl {
            url = url.replace("{ttl}", &ttl.to_string());
        }
        if let Some(source) = &self.parameters.source {
            url = url.replace("{source}", source);
        }
        if let Some(gog_token) = &self.parameters.gog_token {
            url = url.replace("{gog_token}", gog_token);
        }
        if let Some(l) = &self.parameters.l {
            url = url.replace("{l}", l);
        }
        let galaxy_path = format!("{}/{}/{}", &chunk_hash[0..2], &chunk_hash[2..4], chunk_hash);

        // Properly insert chunk path into URL path component (before query string)
        if let Ok(mut parsed_url) = Url::parse(&url) {
            let current_path = parsed_url.path().trim_end_matches('/');
            let new_path = format!("{}/{}", current_path, galaxy_path);
            parsed_url.set_path(&new_path);
            parsed_url.to_string()
        } else {
            // Fallback to simple concatenation if URL parsing fails
            format!("{}/{}", url.trim_end_matches('/'), galaxy_path)
        }
    }
}

impl CdnUrlParams {
    /// Whether this link is still usable, with a 60-second margin subtracted
    /// from `expires_at` for the same reason [`Auth::is_valid`](crate::Auth::is_valid)
    /// subtracts one from a token's expiry: to absorb clock skew and the
    /// in-flight request itself.
    ///
    /// Best-effort: `expires_at` isn't guaranteed to be present. `ttl` is
    /// parsed but not checked here — it's a lifetime in seconds, not a
    /// deadline, and nothing records when the link was issued to add it to.
    /// A link with neither field, or with `expires_at` still 60s+ out, is
    /// treated as valid; expiry with no `expires_at` can still be detected
    /// reactively, via a CDN 401 (see
    /// [`SecureLinksManager::invalidate_secure_links`](crate::secure_links::SecureLinksManager::invalidate_secure_links)).
    pub fn is_valid(&self) -> bool {
        match self.expires_at {
            Some(expires_at) => (expires_at as i64) - 60 > chrono::Utc::now().timestamp(),
            None => true,
        }
    }
}
