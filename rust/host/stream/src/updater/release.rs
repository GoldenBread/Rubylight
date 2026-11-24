//! Release metadata parsing, repository allowlist, and HTTPS download client.

use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::time::Duration;

/// The repository was RamazanKara/Butterpollo until the product became
/// Rubylight. Before the rename only the old name answers; after it, the old
/// name redirects to an API address `client` refuses, so the new one is first.
pub(crate) const REPOSITORIES: [&str; 2] = ["RamazanKara/Rubylight", "RamazanKara/Butterpollo"];
/// Installer names, current first. Releases also carry the old name so that
/// hosts from before the rename find them.
const INSTALLERS: &[&str] = &["rubylight-setup", "butterpollo-setup"];
pub(super) const MAX_INSTALLER: u64 = 512 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Installer {
    pub version: String,
    pub url: String,
    pub digest: String,
    pub size: u64,
}

/// The release list from the first repository name GitHub knows.
pub async fn releases(client: &reqwest::Client) -> Result<reqwest::Response> {
    let mut repositories = REPOSITORIES.iter().peekable();
    while let Some(repository) = repositories.next() {
        let response = client
            .get(format!(
                "https://api.github.com/repos/{repository}/releases?per_page=30"
            ))
            .send()
            .await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND && repositories.peek().is_some() {
            continue;
        }
        return Ok(response.error_for_status()?);
    }
    unreachable!("REPOSITORIES is not empty")
}

pub fn installer(release: &Value) -> Result<Installer> {
    let tag = release["tag_name"]
        .as_str()
        .context("release has no version")?;
    let version = tag.trim_start_matches('v');
    if version.is_empty()
        || !version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-+".contains(&b))
    {
        bail!("invalid release version");
    }
    let assets = release["assets"]
        .as_array()
        .context("release has no assets")?;
    let (name, asset) = INSTALLERS
        .iter()
        .find_map(|prefix| {
            let name = format!("{prefix}-{version}.exe");
            let asset = assets.iter().find(|asset| asset["name"] == name)?;
            Some((name, asset))
        })
        .context("release has no Windows installer")?;
    let url = REPOSITORIES
        .iter()
        .map(|repository| format!("https://github.com/{repository}/releases/download/{tag}/{name}"))
        .find(|url| asset["browser_download_url"] == *url)
        .context("installer is not from the Rubylight release repository")?;
    let digest = asset["digest"]
        .as_str()
        .and_then(|s| s.strip_prefix("sha256:"))
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .context("GitHub has not supplied a SHA-256 digest for this installer")?;
    let size = asset["size"]
        .as_u64()
        .filter(|s| *s > 0 && *s <= MAX_INSTALLER)
        .context("installer size is invalid")?;
    Ok(Installer {
        version: tag.into(),
        url,
        digest: digest.to_ascii_lowercase(),
        size,
    })
}

pub fn client(timeout: Duration) -> Result<reqwest::Client> {
    client_with(Some(timeout))
}
/// The release client, with a deadline for each whole request or none (`None`, for the
/// installer download, which [`idle`] limits instead).
pub(super) fn client_with(timeout: Option<Duration>) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(15));
    if let Some(timeout) = timeout {
        builder = builder.timeout(timeout);
    }
    Ok(builder
        .user_agent("Rubylight")
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            let url = attempt.url();
            if attempt.previous().len() < 5
                && url.scheme() == "https"
                && matches!(
                    url.host_str(),
                    Some(
                        "github.com"
                            | "release-assets.githubusercontent.com"
                            | "objects.githubusercontent.com"
                    )
                )
            {
                attempt.follow()
            } else {
                attempt.error("unexpected release download redirect")
            }
        }))
        .build()?)
}
