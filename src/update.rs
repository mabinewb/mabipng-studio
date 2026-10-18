//! Read-only checks against the project's public GitHub releases.
use anyhow::{Result, ensure};
use semver::Version;
use serde::Deserialize;
use std::io::Read;
use std::time::Duration;

const API: &str = "https://api.github.com/repos/mabinewb/mabipng-studio/releases/latest";
const MAX_RESPONSE: u64 = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    pub url: String,
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
}

fn newer_release(bytes: &[u8], current: &str) -> Result<Option<Release>> {
    let release: ApiRelease = serde_json::from_slice(bytes)?;
    if release.draft || release.prerelease {
        return Ok(None);
    }
    let version = Version::parse(
        release
            .tag_name
            .strip_prefix('v')
            .unwrap_or(&release.tag_name),
    )?;
    if !version.pre.is_empty()
        || version <= Version::parse(current)?
        || !release
            .assets
            .iter()
            .any(|asset| asset.name == "mabipng-studio-windows-x64.exe")
    {
        return Ok(None);
    }
    Ok(Some(Release {
        version,
        // Do not open an arbitrary URL supplied in the network response.
        url: format!(
            "https://github.com/mabinewb/mabipng-studio/releases/tag/{}",
            release.tag_name
        ),
    }))
}

pub fn check_latest() -> Result<Option<Release>> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(8)))
        .build()
        .into();
    let mut response = agent
        .get(API)
        .header(
            "User-Agent",
            concat!("MabiPNG-Studio/", env!("CARGO_PKG_VERSION")),
        )
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .call()?;
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(MAX_RESPONSE + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_RESPONSE,
        "Update response exceeds size limit"
    );
    newer_release(&bytes, env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(tag: &str, draft: bool, prerelease: bool, asset: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "tag_name": tag, "draft": draft, "prerelease": prerelease,
            "assets": [{ "name": asset }], "html_url": "https://example.invalid"
        }))
        .unwrap()
    }

    #[test]
    fn compares_numeric_versions_and_ignores_unpublished_or_unusable_releases() {
        let asset = "mabipng-studio-windows-x64.exe";
        let release = newer_release(&payload("v1.10.0", false, false, asset), "1.9.0")
            .unwrap()
            .unwrap();
        assert_eq!(
            release.url,
            "https://github.com/mabinewb/mabipng-studio/releases/tag/v1.10.0"
        );
        for (tag, draft, pre, name) in [
            ("v1.1.1", false, false, asset),
            ("v1.1.0", false, false, asset),
            ("v1.2.0", true, false, asset),
            ("v1.2.0", false, true, asset),
            ("v1.2.0-beta.1", false, false, asset),
            ("v1.2.0", false, false, "source.zip"),
        ] {
            assert!(
                newer_release(&payload(tag, draft, pre, name), "1.1.1")
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[test]
    fn invalid_responses_fail_without_producing_a_download_link() {
        assert!(newer_release(b"not JSON", "1.1.1").is_err());
        assert!(newer_release(b"{}", "1.1.1").is_err());
        assert!(
            newer_release(
                &payload(
                    "../../other",
                    false,
                    false,
                    "mabipng-studio-windows-x64.exe"
                ),
                "1.1.1"
            )
            .is_err()
        );
    }

    #[test]
    #[ignore = "requires a live GitHub connection; run explicitly before release"]
    fn live_github_check() {
        check_latest().expect("GitHub update check failed");
    }
}
