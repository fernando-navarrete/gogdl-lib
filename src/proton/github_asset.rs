use serde::Deserialize;

/// A single downloadable file attached to a
/// [`ProtonGeRelease`](crate::ProtonGeRelease).
///
/// This is a deliberately narrow subset of GitHub's asset object: `size`,
/// `content_type` and `digest` (the `sha256:`-prefixed checksum GitHub
/// publishes for newer releases) are not deserialized here, so verifying a
/// downloaded tarball's checksum isn't possible from this type alone yet.
///
/// **Picking the right asset:** each release ships a Linux tarball, an ARM
/// (`aarch64`) tarball, and a detached `.sha512sum` file per tarball — the
/// one to download is whichever `name` ends in `.tar.gz` and does not
/// contain `aarch64`.
#[derive(Deserialize)]
pub struct GithubAsset {
    /// The asset's filename, e.g. `"GE-Proton11-6.tar.gz"`.
    pub name: String,
    /// Direct download URL — redirects to
    /// `objects.githubusercontent.com`.
    pub browser_download_url: String,
}
