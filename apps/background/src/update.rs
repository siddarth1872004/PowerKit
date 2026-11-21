//! Checking GitHub for a newer release, and replacing the running executable with it.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;

pub const LATEST_URL: &str = "https://api.github.com/repos/siddarth1872004/PowerKit/releases/latest";
/// The release file that is the new `powerkit.exe`.
pub const ASSET: &str = "powerkit.exe";
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    /// The release's web page.
    pub page: String,
    pub download: String,
    pub size: u64,
    /// The file's SHA-256 as GitHub reports it, checked before the file is installed.
    pub sha256: [u8; 32],
}

/// "v1.2.3", "1.2" or "1.2.3-beta" as numbers; pre-release and build suffixes are
/// ignored.
pub fn parse_version(s: &str) -> Option<[u64; 3]> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let core = s.split(['-', '+']).next()?;
    let mut out = [0u64; 3];
    let mut parts = core.split('.');
    for slot in &mut out {
        match parts.next() {
            Some(p) => *slot = p.parse().ok()?,
            None => break,
        }
    }
    if parts.next().is_some() || core.is_empty() {
        return None;
    }
    Some(out)
}

pub fn is_newer(latest: &str, current: &str) -> bool {
    matches!((parse_version(latest), parse_version(current)), (Some(l), Some(c)) if l > c)
}

/// The parts of a GitHub "latest release" answer the updater needs.
pub fn parse_release(v: &Value) -> Result<Release> {
    let tag = v["tag_name"].as_str().ok_or_else(|| anyhow!("release has no tag"))?;
    let version = tag.trim_start_matches(['v', 'V']).to_string();
    if parse_version(&version).is_none() {
        bail!("release tag {tag:?} is not a version");
    }
    let asset = v["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|a| a["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(ASSET)))
        .ok_or_else(|| anyhow!("release {tag} has no {ASSET}"))?;
    Ok(Release {
        version,
        page: v["html_url"].as_str().unwrap_or_default().to_string(),
        download: asset["browser_download_url"]
            .as_str()
            .ok_or_else(|| anyhow!("{ASSET} has no download address"))?
            .to_string(),
        size: asset["size"].as_u64().unwrap_or(0),
        sha256: asset["digest"]
            .as_str()
            .and_then(parse_digest)
            .ok_or_else(|| anyhow!("release {tag} does not say what {ASSET} should contain"))?,
    })
}

/// "sha256:" and 64 hex digits, as GitHub reports an asset's digest.
fn parse_digest(s: &str) -> Option<[u8; 32]> {
    let hex = s.strip_prefix("sha256:")?;
    if hex.len() != 64 || !hex.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// The newest release, when it is newer than this build.
#[cfg(windows)]
pub fn check() -> Result<Option<Release>> {
    let body = powerkit_platform::http::get(LATEST_URL, "application/vnd.github+json", 1 << 20)
        .context("could not reach GitHub")?;
    let v: Value = serde_json::from_slice(&body).context("GitHub sent an unreadable answer")?;
    let r = parse_release(&v)?;
    Ok(is_newer(&r.version, CURRENT).then_some(r))
}

#[cfg(windows)]
fn old_path(exe: &std::path::Path) -> std::path::PathBuf {
    exe.with_extension("old.exe")
}

/// Download the release and put it where this executable is. Windows lets a running
/// program be renamed but not overwritten, so this one moves aside to
/// `powerkit.old.exe`, which the next start deletes. Returns the path to start.
#[cfg(windows)]
pub fn install(r: &Release) -> Result<std::path::PathBuf> {
    let exe = std::env::current_exe().context("cannot find this program's path")?;
    let bytes = powerkit_platform::http::get(&r.download, "application/octet-stream", 256 << 20)
        .with_context(|| format!("could not download PowerKit {}", r.version))?;
    if (r.size != 0 && bytes.len() as u64 != r.size) || !bytes.starts_with(b"MZ") {
        bail!("the downloaded file is incomplete or not a program");
    }
    if powerkit_platform::crypto::sha256(&bytes).context("checking the download")? != r.sha256 {
        bail!("the downloaded file is not the one GitHub published");
    }
    let new = exe.with_extension("new.exe");
    let old = old_path(&exe);
    std::fs::write(&new, &bytes).with_context(|| format!("could not write {}", new.display()))?;
    match std::fs::remove_file(&old) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            let _ = std::fs::remove_file(&new);
            return Err(e).with_context(|| format!("could not remove {}", old.display()));
        }
        _ => {}
    }
    if let Err(e) = std::fs::rename(&exe, &old) {
        let _ = std::fs::remove_file(&new);
        return Err(e).with_context(|| format!("could not move {} aside", exe.display()));
    }
    if let Err(e) = std::fs::rename(&new, &exe) {
        // Put the working program back so PowerKit still starts next time.
        if let Err(back) = std::fs::rename(&old, &exe) {
            tracing::error!("could not restore {}: {back}", exe.display());
        }
        return Err(e).with_context(|| format!("could not put the new version at {}", exe.display()));
    }
    Ok(exe)
}

/// Delete the executable a previous update moved aside.
#[cfg(windows)]
pub fn remove_leftover() {
    let Ok(exe) = std::env::current_exe() else { return };
    let old = old_path(&exe);
    match std::fs::remove_file(&old) {
        Ok(()) => tracing::info!("removed {}", old.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        // The old process may still be exiting; the next start tries again.
        Err(e) => tracing::warn!("could not remove {}: {e}", old.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn versions_compare_numerically() {
        assert_eq!(parse_version("v1.2.3"), Some([1, 2, 3]));
        assert_eq!(parse_version("0.10"), Some([0, 10, 0]));
        assert_eq!(parse_version("2.0.0-beta.1"), Some([2, 0, 0]));
        assert_eq!(parse_version("1.2.3.4"), None);
        assert_eq!(parse_version("nightly"), None);
        assert_eq!(parse_version(""), None);
        assert!(is_newer("v0.10.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("garbage", "0.1.0"));
    }

    #[test]
    fn release_answer_is_parsed() {
        let v = json!({
            "tag_name": "v0.2.0",
            "html_url": "https://github.com/x/y/releases/tag/v0.2.0",
            "assets": [
                { "name": "notes.txt", "browser_download_url": "https://a/notes.txt", "size": 3 },
                { "name": "PowerKit.exe", "browser_download_url": "https://a/powerkit.exe", "size": 1234,
                  "digest": "sha256:00ff0000000000000000000000000000000000000000000000000000000000a1" }
            ]
        });
        let r = parse_release(&v).unwrap();
        assert_eq!(
            r,
            Release {
                version: "0.2.0".into(),
                page: "https://github.com/x/y/releases/tag/v0.2.0".into(),
                download: "https://a/powerkit.exe".into(),
                size: 1234,
                sha256: {
                    let mut d = [0; 32];
                    d[1] = 0xff;
                    d[31] = 0xa1;
                    d
                },
            }
        );
        let unchecked = json!({ "tag_name": "v0.2.0", "assets": [
            { "name": "powerkit.exe", "browser_download_url": "https://a/powerkit.exe", "digest": "md5:00" }
        ]});
        assert!(
            parse_release(&unchecked).is_err(),
            "an update that cannot be checked is not offered"
        );
        let no_asset = json!({ "tag_name": "v0.2.0", "assets": [] });
        assert!(parse_release(&no_asset)
            .unwrap_err()
            .to_string()
            .contains("powerkit.exe"));
        assert!(parse_release(&json!({ "tag_name": "latest" })).is_err());
    }
}
