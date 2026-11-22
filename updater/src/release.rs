//! The GitHub "latest release" answer, read and checked.

use serde::Deserialize;

use crate::{DOWNLOAD_PREFIX, RELEASES_PAGE};

/// How much of the release notes is kept for the window.
pub const NOTES_MAX_CHARS: usize = 600;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    /// Bytes as GitHub reports them (0 when unknown).
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// The tag as published, e.g. `v0.6.4`.
    pub tag: String,
    /// The version without the `v`, e.g. `0.6.4`.
    pub version: String,
    /// Release notes, truncated to [`NOTES_MAX_CHARS`].
    pub notes: String,
    /// The release's page on GitHub.
    pub page_url: String,
    /// Files that can be downloaded from this repository's releases only.
    pub assets: Vec<Asset>,
}

impl Release {
    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == name)
    }

    pub fn asset_names(&self) -> Vec<&str> {
        self.assets.iter().map(|a| a.name.as_str()).collect()
    }
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

/// A file name that is safe to create in a folder: no path, no `..`.
pub fn safe_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && name != "."
        && name != ".."
        && !name.starts_with('.')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'))
}

/// Reads the answer of `releases/latest`. Drafts and prereleases are
/// refused (GitHub does not return them there, but the answer is not
/// trusted for that); files that are not downloads of this repository's
/// releases, or have an unsafe name, are left out.
pub fn parse_latest(json: &[u8]) -> Result<Release, String> {
    let api: ApiRelease =
        serde_json::from_slice(json).map_err(|e| format!("unexpected answer from GitHub ({e})"))?;
    if api.draft || api.prerelease {
        return Err("the latest release is a draft or a prerelease".into());
    }
    let version = crate::version::parse(&api.tag_name)
        .ok_or_else(|| format!("the release tag {} is not a version", api.tag_name))?;
    let page_url = api
        .html_url
        .filter(|u| u.starts_with("https://github.com/"))
        .unwrap_or_else(|| RELEASES_PAGE.to_string());
    let assets = api
        .assets
        .into_iter()
        .filter(|a| safe_file_name(&a.name) && a.browser_download_url.starts_with(DOWNLOAD_PREFIX))
        .map(|a| Asset { name: a.name, url: a.browser_download_url, size: a.size })
        .collect();
    Ok(Release {
        tag: api.tag_name,
        version: version.to_string(),
        notes: truncate_notes(api.body.as_deref().unwrap_or(""), NOTES_MAX_CHARS),
        page_url,
        assets,
    })
}

/// The first `max_chars` characters of the notes (on a character boundary),
/// with "…" when something was cut.
pub fn truncate_notes(body: &str, max_chars: usize) -> String {
    let body = body.trim().replace("\r\n", "\n");
    match body.char_indices().nth(max_chars) {
        None => body,
        Some((cut, _)) => format!("{}…", body[..cut].trim_end()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "tag_name": "v0.6.4", "html_url": "https://github.com/collyn/lunaris/releases/tag/v0.6.4",
        "draft": false, "prerelease": false, "body": "Fixes\r\n- one",
        "assets": [
            {"name": "lunaris-agent_0.6.4_amd64.deb", "size": 10,
             "browser_download_url": "https://github.com/collyn/lunaris/releases/download/v0.6.4/lunaris-agent_0.6.4_amd64.deb"},
            {"name": "evil.deb", "size": 1, "browser_download_url": "https://example.com/evil.deb"},
            {"name": "../x", "size": 1, "browser_download_url": "https://github.com/collyn/lunaris/releases/download/v0.6.4/x"}
        ]}"#;

    #[test]
    fn reads_a_release() {
        let r = parse_latest(SAMPLE.as_bytes()).unwrap();
        assert_eq!(r.tag, "v0.6.4");
        assert_eq!(r.version, "0.6.4");
        assert_eq!(r.notes, "Fixes\n- one");
        assert_eq!(r.asset_names(), vec!["lunaris-agent_0.6.4_amd64.deb"]);
        assert_eq!(r.asset("lunaris-agent_0.6.4_amd64.deb").unwrap().size, 10);
    }

    #[test]
    fn refuses_prereleases_and_bad_tags() {
        let pre = SAMPLE.replace("\"prerelease\": false", "\"prerelease\": true");
        assert!(parse_latest(pre.as_bytes()).is_err());
        let bad = SAMPLE.replace("v0.6.4\", \"html_url", "nightly\", \"html_url");
        assert!(parse_latest(bad.as_bytes()).is_err());
        assert!(parse_latest(b"not json").is_err());
    }

    #[test]
    fn truncates_on_a_character_boundary() {
        assert_eq!(truncate_notes("short", 10), "short");
        assert_eq!(truncate_notes("Cập nhật mới", 4), "Cập…");
        assert_eq!(truncate_notes("abc def", 4), "abc…");
    }

    #[test]
    fn file_names() {
        assert!(safe_file_name("lunaris-agent-0.6.4-1.fc44.x86_64.rpm"));
        assert!(safe_file_name("SHA256SUMS.sig"));
        assert!(!safe_file_name("../SHA256SUMS"));
        assert!(!safe_file_name("a/b"));
        assert!(!safe_file_name(".hidden"));
        assert!(!safe_file_name(""));
    }
}
