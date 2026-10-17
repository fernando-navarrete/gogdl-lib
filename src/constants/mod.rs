pub const LOGIN_URL: &str = "https://auth.gog.com/auth?client_id=46899977096215655&redirect_uri=https://embed.gog.com/on_login_success?origin=client&response_type=code&layout=client2";
pub const TOKEN_URL: &str = "https://auth.gog.com/token";
pub const CLIENT_ID: &str = "46899977096215655";
pub const CLIENT_SECRET: &str = "9d85c43b1482497dbbce61f6e4aa173a433796eeae2ca8c5f6129f2dc4de46d9";
pub const REDIRECT_URI: &str = "https://embed.gog.com/on_login_success?origin=client";
pub const CLOUD_STORAGE_URL: &str = "https://cloudstorage.gog.com/v1/";
/// Headers every GitHub API request carries: GitHub rejects a request with no
/// `User-Agent`, and the API version is pinned.
pub(crate) const GITHUB_API_HEADERS: [(&str, &str); 3] = [
    ("Accept", "application/vnd.github.v3+json"),
    ("User-Agent", "gogdl"),
    ("X-GitHub-Api-Version", "2026-03-10"),
];
