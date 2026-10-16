// The phone: an iPhone's screen in a window of ours, and the mouse and the
// keyboard on it (src/phone/). Nothing on the phone but Apple's own XCTest
// running WebDriverAgent (WDA), which the person installs once (README); go-ios
// (phone/goios.rs, got by phone/goios_setup.rs on their click) finds the phone
// on USB, makes iOS 17+'s tunnel, mounts the developer image, starts WDA and
// forwards two ports here to the phone's: WDA's HTTP (8100: touches, keys,
// buttons) and its MJPEG server (9100: the screen).
//
// The page sends where it was touched as fractions of the frame (0..1, the
// frame being the screen as it is turned); here they become WDA's points
// (the screen's size in points, asked again when the page sees the frame
// turn). Frames go to the page over a channel (phone_stream), raw JPEG bytes,
// at most two unseen at a time: a page slower than the stream skips frames
// rather than falling behind.
//
// What it needs of the app: Shared's app, dir (its data folder), log() and
// phone (a Mutex<Phone>), and its window labelled "phone" (capabilities).
// Errors go to the page as { error: code, detail }, the code worded by the
// page (src/shared/i18n.js, phone.err.*).
pub mod goios;
pub mod goios_setup;
pub mod mjpeg;
pub mod wda;
mod zip;

use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, Manager};

use crate::{shared, Shared};
use goios::{Device, GoIos, Kept};
use wda::{Size, Step, Wda};

pub const WINDOW: &str = "phone";
// Frames sent the page has not drawn yet; past it, frames are skipped.
const IN_FLIGHT: usize = 2;
const WDA_READY: Duration = Duration::from_secs(90);

#[derive(Default)]
pub struct Phone {
    conn: Option<Arc<Conn>>,
    connecting: bool,
}

struct Conn {
    wda: Arc<Wda>,
    mjpeg: String,
    device: Option<Device>,
    size: Mutex<Size>,
    stop: Arc<AtomicBool>,
    in_flight: Arc<AtomicUsize>,
    // go-ios's tunnel, forward and WDA, stopped with the connection (end).
    kept: Vec<Kept>,
}

impl Conn {
    // Its stream stopped and its go-ios with it, now: not left to the
    // drop, as the stream's thread and a command in flight hold it too.
    fn end(&self) {
        self.stop.store(true, Ordering::Relaxed);
        for k in &self.kept {
            k.stop();
        }
    }
}

fn err(code: &str, detail: impl ToString) -> Value {
    json!({ "error": code, "detail": detail.to_string() })
}

fn log(sh: &Shared, line: &str) {
    sh.log(&format!("phone: {line}"));
    let _ = sh.app.emit_to(WINDOW, "phone:log", line);
}

fn step(sh: &Shared, name: &str) {
    log(sh, &format!("step {name}"));
    let _ = sh.app.emit_to(WINDOW, "phone:step", name);
}

fn goios(sh: &Shared) -> Option<GoIos> {
    let exe = goios_setup::installed(&sh.dir)?;
    let dir = sh.dir.join("goios");
    let _ = std::fs::create_dir_all(&dir);
    Some(GoIos { exe, dir })
}

fn conn(sh: &Shared) -> Option<Arc<Conn>> {
    sh.phone.lock().unwrap().conn.clone()
}

// Blocking work off the command's thread.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tauri::async_runtime::spawn_blocking(f).await.expect("a phone task")
}

// A port no one listens on here, for a forward.
fn free_port() -> Option<u16> {
    TcpListener::bind(("127.0.0.1", 0)).ok()?.local_addr().ok().map(|a| a.port())
}

// The quality the page picked: frames a second, JPEG quality, scale (%).
fn quality(name: &str) -> (u32, u32, u32) {
    match name {
        "smooth" => (30, 40, 40),
        "hd" => (24, 85, 75),
        _ => (30, 60, 50),
    }
}

// UTC as yyyymmdd-hhmmss, for file names (no time zone to hand without one
// more crate; merged into Wakuwaku, widgets.rs's local time).
fn stamp() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}{m:02}{d:02}-{:02}{:02}{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn pictures(sh: &Shared) -> std::path::PathBuf {
    let base = sh.app.path().picture_dir().unwrap_or_else(|_| sh.dir.clone());
    base.join("iPhone Link")
}

// --- Connecting ---------------------------------------------------------------------

// The steps to a WDA that answers, over USB: the tunnel (iOS 17+), the
// forward, WDA itself if it does not answer already (the image mounted, its
// runner found, started, waited for).
fn connect_usb(sh: &Shared, g: &GoIos, device: &Device, bundle: Option<String>) -> Result<(Arc<Wda>, String, Vec<Kept>), Value> {
    let lg: goios::Log = {
        let app = sh.app.clone();
        Arc::new(move |line: &str| log(&shared(&app), line))
    };
    let mut kept = Vec::new();
    if device.ios_major() >= 17 {
        step(sh, "tunnel");
        match g.tunnel(&lg) {
            Ok(Some(k)) => kept.push(k),
            Ok(None) => {}
            Err(e) => return Err(err("tunnel", e)),
        }
    }

    step(sh, "forward");
    let (wda_port, mjpeg_port) = (free_port().unwrap_or(8100), free_port().unwrap_or(9100));
    kept.push(g.forward(&device.udid, &[(wda_port, 8100), (mjpeg_port, 9100)], &lg).map_err(|e| err("forward", e))?);
    let wda = Arc::new(Wda::new(&format!("http://127.0.0.1:{wda_port}")));
    let mjpeg = format!("http://127.0.0.1:{mjpeg_port}");
    std::thread::sleep(Duration::from_millis(800));
    if wda.ready() {
        log(sh, "WDA answers already");
        return Ok((wda, mjpeg, kept));
    }

    step(sh, "mount");
    // Mounted already, or not needed: go on, and WDA says if it was.
    match g.mount(&device.udid) {
        Ok(out) => log(sh, &format!("mount: {}", out.trim())),
        Err(e) => log(sh, &format!("mount: {e}")),
    }

    step(sh, "findWda");
    let bundle = match bundle.filter(|b| !b.trim().is_empty()) {
        Some(b) => b.trim().to_string(),
        None => {
            let apps = g.user_apps(&device.udid).map_err(|e| err("apps", e))?;
            match goios::find_wda(&apps) {
                Some(b) => b,
                None => {
                    // What is there instead, for whoever reads the log: a
                    // runner under a name we do not know is in it.
                    log(sh, &format!("no WebDriverAgentRunner among the phone's apps:\n{}", apps.trim()));
                    return Err(err("noWda", ""));
                }
            }
        }
    };
    log(sh, &format!("WDA: {bundle}"));

    step(sh, "startWda");
    let runner = g.run_wda(&device.udid, &bundle, &lg).map_err(|e| err("startWda", e))?;
    let until = Instant::now() + WDA_READY;
    while Instant::now() < until {
        if wda.ready() {
            kept.push(runner);
            return Ok((wda, mjpeg, kept));
        }
        if !runner.alive() {
            return Err(err("wdaExited", runner.last_words()));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err(err("wdaTimeout", runner.last_words()))
}

fn connect(sh: &Shared, udid: Option<String>, direct: Option<String>, bundle: Option<String>, q: &str) -> Value {
    let (wda, mjpeg, device, kept) = match direct.filter(|d| !d.trim().is_empty()) {
        // WDA started some other way (or on the phone's address over Wi-Fi):
        // its MJPEG server on its port 9100.
        Some(url) => {
            let wda = Arc::new(Wda::new(url.trim()));
            if !wda.ready() {
                return err("unreachable", wda.base());
            }
            let host = wda.base().rsplit_once(':').map(|(h, _)| h.to_string()).unwrap_or_else(|| wda.base().to_string());
            (wda, format!("{host}:9100"), None, Vec::new())
        }
        None => {
            let Some(g) = goios(sh) else { return err("noGoIos", "") };
            let devices = match g.devices() {
                Ok(d) => d,
                Err(e) => return err("usbmux", e),
            };
            let Some(device) = devices.into_iter().find(|d| udid.as_deref().is_none_or(|u| u == d.udid)) else {
                return err("noDevice", "");
            };
            match connect_usb(sh, &g, &device, bundle) {
                Ok((wda, mjpeg, kept)) => (wda, mjpeg, Some(device), kept),
                Err(e) => return e,
            }
        }
    };

    step(sh, "session");
    let size = match wda.session_id().and_then(|_| wda.window_size()) {
        Ok(s) => s,
        Err(e) => return err("wda", e),
    };
    let (fps, quality, scale) = quality(q);
    if let Err(e) = wda.mjpeg(fps, quality, scale) {
        log(sh, &format!("MJPEG settings not taken (the screen still comes): {e}"));
    }
    let c = Arc::new(Conn {
        wda,
        mjpeg,
        device: device.clone(),
        size: Mutex::new(size),
        stop: Arc::new(AtomicBool::new(false)),
        in_flight: Arc::new(AtomicUsize::new(0)),
        kept,
    });
    sh.phone.lock().unwrap().conn = Some(c);
    step(sh, "ready");
    json!({ "ok": true, "device": device, "size": size })
}

// The phone's window, made or brought to the front. Its own data folder for
// WebView2 as Wakuwaku's windows have (crate::webview_dir there).
pub fn open(sh: &Shared) -> tauri::Result<()> {
    if let Some(w) = sh.app.get_webview_window(WINDOW) {
        let _ = w.unminimize();
        return w.set_focus();
    }
    tauri::WebviewWindowBuilder::new(&sh.app, WINDOW, tauri::WebviewUrl::App("phone/index.html".into()))
        .title("iPhone Link")
        .inner_size(920.0, 960.0)
        .min_inner_size(560.0, 620.0)
        .data_directory(sh.dir.join("webview"))
        .build()
        .map(drop)
}

pub fn disconnect(sh: &Shared) {
    let gone = sh.phone.lock().unwrap().conn.take();
    if let Some(c) = gone {
        c.end();
        let names: Vec<&str> = c.kept.iter().map(|k| k.name.as_str()).collect();
        log(sh, &format!("disconnected, go-ios stopped: {}", names.join(", ")));
    }
}

// --- From the page ----------------------------------------------------------------

#[tauri::command]
pub async fn phone_state(app: AppHandle) -> Value {
    let sh = shared(&app);
    let c = conn(&sh);
    json!({
        "goios": goios_setup::installed(&sh.dir).map(|p| p.display().to_string()),
        "goiosVersion": goios_setup::VERSION,
        "connected": c.is_some(),
        "device": c.as_ref().and_then(|c| c.device.clone()),
        "size": c.as_ref().map(|c| *c.size.lock().unwrap()),
    })
}

#[tauri::command]
pub async fn phone_setup(app: AppHandle) -> Value {
    blocking(move || {
        let sh = shared(&app);
        log(&sh, &format!("getting go-ios {}", goios_setup::VERSION));
        let progress = |pct: u32| {
            let _ = sh.app.emit_to(WINDOW, "phone:setup", pct);
        };
        match goios_setup::install(&sh.dir, progress) {
            Ok(exe) => {
                log(&sh, &format!("go-ios at {}", exe.display()));
                json!({ "ok": true, "path": exe.display().to_string() })
            }
            Err(e) => {
                log(&sh, &format!("go-ios not got: {} {}", e.code, e.detail));
                err(e.code, e.detail)
            }
        }
    })
    .await
}

#[tauri::command]
pub async fn phone_devices(app: AppHandle) -> Value {
    blocking(move || {
        let sh = shared(&app);
        let Some(g) = goios(&sh) else { return err("noGoIos", "") };
        match g.devices() {
            Ok(list) => json!({ "devices": list }),
            Err(e) => {
                log(&sh, &format!("devices: {e}"));
                err("usbmux", e)
            }
        }
    })
    .await
}

#[tauri::command]
pub async fn phone_connect(app: AppHandle, udid: Option<String>, direct: Option<String>, bundle: Option<String>, quality: Option<String>) -> Value {
    {
        let sh = shared(&app);
        let mut p = sh.phone.lock().unwrap();
        if p.connecting {
            return err("busy", "");
        }
        p.connecting = true;
    }
    disconnect(&shared(&app));
    let done = app.clone();
    let out = blocking(move || connect(&shared(&app), udid, direct, bundle, quality.as_deref().unwrap_or("standard"))).await;
    shared(&done).phone.lock().unwrap().connecting = false;
    out
}

#[tauri::command]
pub async fn phone_disconnect(app: AppHandle) {
    blocking(move || disconnect(&shared(&app))).await
}

// The screen, frame by frame, to the page's channel, till the connection
// ends or the page is gone.
#[tauri::command]
pub async fn phone_stream(app: AppHandle, frames: Channel<InvokeResponseBody>) -> Value {
    let sh = shared(&app);
    let Some(c) = conn(&sh) else { return err("notConnected", "") };
    // What the thread needs, not the connection itself: it must not keep
    // the connection (and so its go-ios) alive.
    let (url, wda, stop, in_flight) = (c.mjpeg.clone(), c.wda.clone(), c.stop.clone(), c.in_flight.clone());
    drop(c);
    let app2 = app.clone();
    std::thread::Builder::new()
        .name("phone frames".into())
        .spawn(move || {
            let frame = |bytes: Vec<u8>| {
                if in_flight.load(Ordering::Relaxed) >= IN_FLIGHT {
                    return true;
                }
                in_flight.fetch_add(1, Ordering::Relaxed);
                frames.send(InvokeResponseBody::Raw(bytes)).is_ok()
            };
            let status = |s: &str| {
                let _ = app2.emit_to(WINDOW, "phone:stream", s);
            };
            mjpeg::run(&url, wda, stop, frame, status);
        })
        .expect("a thread");
    json!({ "ok": true })
}

// A frame drawn: room for the next.
#[tauri::command]
pub async fn phone_seen(app: AppHandle) {
    if let Some(c) = conn(&shared(&app)) {
        let _ = c.in_flight.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| Some(n.saturating_sub(1)));
    }
}

// The screen's size in points asked again: the page saw the frame turn.
#[tauri::command]
pub async fn phone_size(app: AppHandle) -> Value {
    blocking(move || {
        let Some(c) = conn(&shared(&app)) else { return err("notConnected", "") };
        match c.wda.window_size() {
            Ok(s) => {
                *c.size.lock().unwrap() = s;
                json!({ "size": s })
            }
            Err(e) => err("wda", e),
        }
    })
    .await
}

// One thing done with WDA, in points from the page's fractions.
async fn with_wda(app: AppHandle, f: impl FnOnce(&Wda, Size) -> Result<Value, String> + Send + 'static) -> Value {
    blocking(move || {
        let sh = shared(&app);
        let Some(c) = conn(&sh) else { return err("notConnected", "") };
        let size = *c.size.lock().unwrap();
        f(&c.wda, size).unwrap_or_else(|e| {
            log(&sh, &e);
            err("wda", e)
        })
    })
    .await
}

fn ok(_: ()) -> Value {
    json!({ "ok": true })
}

#[tauri::command]
pub async fn phone_tap(app: AppHandle, x: f64, y: f64) -> Value {
    with_wda(app, move |w, s| w.tap(x * s.width, y * s.height).map(ok)).await
}

#[tauri::command]
pub async fn phone_press(app: AppHandle, x: f64, y: f64, ms: u64) -> Value {
    with_wda(app, move |w, s| w.long_press(x * s.width, y * s.height, ms.clamp(300, 3000)).map(ok)).await
}

// A drag along the page's steps ([x, y, ms since the step before]).
#[tauri::command]
pub async fn phone_drag(app: AppHandle, steps: Vec<(f64, f64, u64)>, hold: u64) -> Value {
    with_wda(app, move |w, s| {
        let pts: Vec<Step> = steps.iter().map(|&(x, y, dt)| (x * s.width, y * s.height, dt.min(2000))).collect();
        w.drag(&pts, hold.min(2000)).map(ok)
    })
    .await
}

// The wheel: a notch (120) a sixth of the screen, at most most of it.
pub fn scroll_to(y: f64, delta: f64) -> f64 {
    let dist = (delta / 120.0 * 0.15).clamp(-0.6, 0.6);
    (y + dist).clamp(0.02, 0.98)
}

#[tauri::command]
pub async fn phone_scroll(app: AppHandle, x: f64, y: f64, delta: f64) -> Value {
    with_wda(app, move |w, s| {
        let to = scroll_to(y, delta);
        w.swipe((x * s.width, y * s.height), (x * s.width, to * s.height), 300, 0).map(ok)
    })
    .await
}

#[tauri::command]
pub async fn phone_type(app: AppHandle, text: String) -> Value {
    with_wda(app, move |w, _| w.type_text(&text).map(ok)).await
}

// The phone's buttons, and the gestures that stand for the ones it has none of.
#[tauri::command]
pub async fn phone_button(app: AppHandle, name: String) -> Value {
    with_wda(app, move |w, s| {
        let (wd, ht) = (s.width, s.height);
        match name.as_str() {
            "home" => w.home(),
            "volumeUp" | "volumeDown" => w.press_button(&name),
            "lock" => {
                if w.locked()? {
                    w.unlock()
                } else {
                    w.lock()
                }
            }
            // Up from the bottom edge and held: the app switcher (Face ID phones).
            "switcher" => w.swipe((wd * 0.5, ht - 2.0), (wd * 0.5, ht * 0.6), 300, 700),
            "notifications" => w.swipe((wd * 0.25, 2.0), (wd * 0.25, ht * 0.7), 300, 0),
            "control" => w.swipe((wd * 0.9, 2.0), (wd * 0.9, ht * 0.6), 300, 0),
            other => Err(format!("no button {other}")),
        }
        .map(ok)
    })
    .await
}

#[tauri::command]
pub async fn phone_quality(app: AppHandle, name: String) -> Value {
    let (fps, q, scale) = quality(&name);
    with_wda(app, move |w, _| w.mjpeg(fps, q, scale).map(ok)).await
}

#[tauri::command]
pub async fn phone_clip_push(app: AppHandle, text: String) -> Value {
    with_wda(app, move |w, _| w.set_pasteboard(&text).map(ok)).await
}

#[tauri::command]
pub async fn phone_clip_pull(app: AppHandle) -> Value {
    with_wda(app, move |w, _| w.pasteboard().map(|text| json!({ "text": text }))).await
}

// The screen at its full size into Pictures\iPhone Link.
#[tauri::command]
pub async fn phone_screenshot(app: AppHandle) -> Value {
    let dir = pictures(&shared(&app));
    with_wda(app, move |w, _| {
        let png = w.screenshot()?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(format!("iPhone-{}.png", stamp()));
        std::fs::write(&path, png).map_err(|e| e.to_string())?;
        Ok(json!({ "path": path.display().to_string() }))
    })
    .await
}

// The camera roll (/DCIM) copied whole into Pictures\iPhone Link\Photos-….
#[tauri::command]
pub async fn phone_photos(app: AppHandle) -> Value {
    blocking(move || {
        let sh = shared(&app);
        let Some(g) = goios(&sh) else { return err("noGoIos", "") };
        let Some(udid) = conn(&sh).and_then(|c| c.device.as_ref().map(|d| d.udid.clone())) else {
            return err("usbOnly", "");
        };
        let to = pictures(&sh).join(format!("Photos-{}", stamp()));
        log(&sh, &format!("photos to {}", to.display()));
        match g.pull(&udid, "/DCIM", &to) {
            Ok(_) => json!({ "path": to.display().to_string() }),
            Err(e) => err("photos", e),
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wheel_notch_scrolls_a_little_and_never_off_the_screen() {
        assert!((scroll_to(0.5, 120.0) - 0.65).abs() < 1e-9);
        assert!((scroll_to(0.5, -120.0) - 0.35).abs() < 1e-9);
        assert_eq!(scroll_to(0.5, 12_000.0), 0.98);
        assert_eq!(scroll_to(0.1, -12_000.0), 0.02);
    }

    #[test]
    fn stamps_are_utc_dates() {
        let s = stamp();
        assert_eq!(s.len(), 15);
        assert_eq!(&s[8..9], "-");
        assert!(s.starts_with("20"));
    }

    #[test]
    fn qualities_fall_back_to_standard() {
        assert_eq!(quality("smooth"), (30, 40, 40));
        assert_eq!(quality("whatever"), quality("standard"));
    }
}
