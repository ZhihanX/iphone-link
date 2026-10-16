// Where its data is kept: go-ios and what go-ios keeps (pair records,
// developer images), WebView2's folder, debug.log. IPHONE_LINK_DATA (tests);
// else iphone-link-data beside the exe, as Wakuwaku keeps wakuwaku-data (its
// data.rs): one folder, removed with the program.
use std::path::PathBuf;

pub fn folder() -> PathBuf {
    if let Ok(dir) = std::env::var("IPHONE_LINK_DATA") {
        return dir.into();
    }
    let exe = std::env::current_exe().unwrap_or_default();
    exe.parent().map(|p| p.join("iphone-link-data")).unwrap_or_else(|| "iphone-link-data".into())
}
