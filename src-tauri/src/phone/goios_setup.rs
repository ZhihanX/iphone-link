// go-ios got, on the person's click (as Wakuwaku gets LibreHardwareMonitor:
// lhm_setup.rs): its release v1.3.2 on GitHub (the API's releases/tags), the
// zip for this system ("go-ios-win.zip" and so on), downloaded only with the
// SHA-256 GitHub gives for it ("digest": "sha256:…") and checked against it
// (none given: not downloaded), unpacked (zip.rs: every name checked, no
// path out of the folder) into <data>/goios/v1.3.2/.
//
// One version, pinned: its commands and their output are what goios.rs was
// written against, and a newer go-ios changes them (the tunnel's agent port
// moved after 1.3.2). Moving to another is changing VERSION and trying
// goios.rs's steps on a phone.
//
// For tests: IPHONE_LINK_GOIOS_RELEASE, the release's JSON from elsewhere;
// IPHONE_LINK_GOIOS, an `ios` already here, used as it is.
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use sha2::{Digest, Sha256};

pub const VERSION: &str = "v1.3.2";
const RELEASE: &str = "https://api.github.com/repos/danielpaulus/go-ios/releases/tags/v1.3.2";
const MAX_ZIP: u64 = 64 << 20;

pub fn asset_name() -> &'static str {
    if cfg!(windows) {
        "go-ios-win.zip"
    } else if cfg!(target_os = "macos") {
        "go-ios-mac.zip"
    } else {
        "go-ios-linux.zip"
    }
}

// Its program's names in the zip: "ios.exe" in Windows's; Linux's has one
// for each processor ("ios-amd64", "ios-arm64").
fn exe_names() -> Vec<String> {
    if cfg!(windows) {
        return vec!["ios.exe".into()];
    }
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    };
    vec![format!("ios-{arch}"), "ios".into()]
}

pub fn folder(data: &Path) -> PathBuf {
    data.join("goios").join(VERSION)
}

// The `ios` in a folder, at its top or a level or two down (the zip's own
// folders).
fn find_exe(dir: &Path, depth: u32) -> Option<PathBuf> {
    if let Some(here) = exe_names().iter().map(|n| dir.join(n)).find(|p| p.is_file()) {
        return Some(here);
    }
    if depth == 0 {
        return None;
    }
    let mut subs: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    subs.sort();
    subs.iter().find_map(|d| find_exe(d, depth - 1))
}

// go-ios if it is here.
pub fn installed(data: &Path) -> Option<PathBuf> {
    if let Ok(path) = std::env::var("IPHONE_LINK_GOIOS") {
        return Some(path.into());
    }
    find_exe(&folder(data), 2)
}

// The asset to download from a release (GitHub's JSON): its URL and its
// SHA-256 in hex; an error code else.
pub fn choose(release: &Value, name: &str) -> Result<(String, String), &'static str> {
    let asset = release.get("assets").and_then(Value::as_array).and_then(|a| a.iter().find(|x| x.get("name").and_then(Value::as_str) == Some(name))).ok_or("noAsset")?;
    let url = asset.get("browser_download_url").and_then(Value::as_str).ok_or("noAsset")?;
    let digest = asset.get("digest").and_then(Value::as_str).and_then(|d| d.strip_prefix("sha256:")).filter(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit())).ok_or("noDigest")?;
    Ok((url.to_string(), digest.to_ascii_lowercase()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub struct SetupError {
    pub code: &'static str,
    pub detail: String,
}

fn fail(code: &'static str, detail: impl ToString) -> SetupError {
    SetupError { code, detail: detail.to_string() }
}

// Through the proxy the environment names, if one (ureq's own); merged into
// Wakuwaku, through its proxy.rs as lhm_setup.rs does.
fn agent(seconds: u64) -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(seconds))).build().new_agent()
}

// go-ios downloaded, checked and unpacked; its `ios`. `progress` hears the
// download's percent.
pub fn install(data: &Path, progress: impl Fn(u32)) -> Result<PathBuf, SetupError> {
    let release_url = std::env::var("IPHONE_LINK_GOIOS_RELEASE").unwrap_or_else(|_| RELEASE.to_string());
    let release: Value = agent(30)
        .get(&release_url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| fail("release", e))?
        .body_mut()
        .read_json()
        .map_err(|e| fail("release", e))?;
    let (url, sha) = choose(&release, asset_name()).map_err(|code| fail(code, asset_name()))?;

    let mut resp = agent(600).get(&url).call().map_err(|e| fail("download", e))?;
    let total: u64 = resp.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut reader = resp.body_mut().with_config().limit(MAX_ZIP).reader();
    let mut zip = Vec::with_capacity(total as usize);
    let mut chunk = vec![0u8; 256 << 10];
    let mut said = u32::MAX;
    loop {
        let n = reader.read(&mut chunk).map_err(|e| fail("download", e))?;
        if n == 0 {
            break;
        }
        zip.extend_from_slice(&chunk[..n]);
        if let Some(pct) = (zip.len() as u64 * 100).checked_div(total) {
            let pct = pct.min(100) as u32;
            if pct != said {
                said = pct;
                progress(pct);
            }
        }
    }
    let got = hex(&Sha256::digest(&zip));
    if got != sha {
        return Err(fail("checksum", format!("{got} ≠ {sha}")));
    }

    let dir = folder(data);
    let stage = dir.with_extension("new");
    let _ = std::fs::remove_dir_all(&stage);
    super::zip::extract(&zip, &stage).map_err(|e| fail("unzip", e))?;
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::rename(&stage, &dir).map_err(|e| fail("unzip", e))?;
    let exe = find_exe(&dir, 2).ok_or_else(|| fail("unzip", format!("no {} in {}", exe_names().join(" or "), asset_name())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755));
    }
    Ok(exe)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SHA: &str = "939c6bcaafed183a92afb9f79cc11b1f935fa6389bfc94d3902e3f52c4dff3fe";

    #[test]
    fn the_asset_is_taken_only_with_its_digest() {
        let release = json!({ "assets": [
            { "name": "go-ios-linux.zip", "browser_download_url": "https://x/l.zip" },
            { "name": "go-ios-win.zip", "browser_download_url": "https://x/w.zip", "digest": format!("sha256:{}", SHA.to_uppercase()) },
        ]});
        assert_eq!(choose(&release, "go-ios-win.zip"), Ok(("https://x/w.zip".into(), SHA.into())));
        assert_eq!(choose(&release, "go-ios-linux.zip"), Err("noDigest"));
        assert_eq!(choose(&release, "go-ios-mac.zip"), Err("noAsset"));
        assert_eq!(choose(&json!({}), "go-ios-win.zip"), Err("noAsset"));
    }

    #[test]
    fn the_exe_is_found_in_the_zips_folders() {
        let dir = std::env::temp_dir().join(format!("iphone-link-find-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("go-ios-win")).unwrap();
        assert_eq!(find_exe(&dir, 2), None);
        let name = &exe_names()[0];
        std::fs::write(dir.join("go-ios-win").join(name), b"").unwrap();
        assert_eq!(find_exe(&dir, 2), Some(dir.join("go-ios-win").join(name)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // The whole of it against a release served somewhere: IPHONE_LINK_TEST_RELEASES
    // a URL whose release.json names this system's zip with its right
    // digest and bad.json the same zip with a wrong one. Then go-ios run.
    #[test]
    #[ignore = "needs IPHONE_LINK_TEST_RELEASES (docs/DEVELOPMENT.md)"]
    fn a_release_is_downloaded_checked_unpacked_and_runs() {
        let base = std::env::var("IPHONE_LINK_TEST_RELEASES").expect("IPHONE_LINK_TEST_RELEASES");
        let data = std::env::temp_dir().join(format!("iphone-link-setup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);

        std::env::set_var("IPHONE_LINK_GOIOS_RELEASE", format!("{base}/bad.json"));
        let e = install(&data, |_| {}).map(drop).expect_err("a wrong digest refused");
        assert_eq!(e.code, "checksum");
        assert_eq!(installed(&data), None, "nothing kept");

        std::env::set_var("IPHONE_LINK_GOIOS_RELEASE", format!("{base}/release.json"));
        let said = std::sync::Mutex::new(Vec::new());
        let exe = install(&data, |p| said.lock().unwrap().push(p)).unwrap_or_else(|e| panic!("{} {}", e.code, e.detail));
        assert_eq!(installed(&data), Some(exe.clone()));
        assert_eq!(said.lock().unwrap().last(), Some(&100));
        let out = std::process::Command::new(&exe).arg("version").output().unwrap();
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("1.3.2"), "{:?}", out);
        let _ = std::fs::remove_dir_all(&data);
    }
}

