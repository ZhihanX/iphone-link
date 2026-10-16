// WebDriverAgent's HTTP, on the phone at 8100 (forwarded over USB to a port
// here by goios.rs). Only what appium/WebDriverAgent has had for years: a
// session, the screen's size in points, W3C actions for every touch (taps,
// presses, drags: one way for all, the one WDA keeps working), its buttons,
// typing, the pasteboard, a screenshot and the MJPEG server's settings.
//
// A session WDA forgot (it restarts its session when the runner restarts)
// is made again once, and the call tried again.
use std::sync::Mutex;
use std::time::Duration;

use base64::Engine;
use serde_json::{json, Value};

// The biggest answer read: a screenshot, a PNG of the whole screen in
// base64 (an iPhone Pro Max's is about 8 MB).
const MAX_BODY: u64 = 64 << 20;

pub struct Wda {
    base: String,
    agent: ureq::Agent,
    session: Mutex<Option<String>>,
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

// One point of a drag: where, and how long after the point before (ms).
pub type Step = (f64, f64, u64);

pub fn agent(seconds: u64) -> ureq::Agent {
    // Never through a proxy: it is a port on this computer (or the phone's
    // own address on the same network). Statuses read, not raised: WDA says
    // what went wrong in its JSON.
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(seconds)))
        .proxy(None)
        .http_status_as_error(false)
        .build()
        .new_agent()
}

impl Wda {
    pub fn new(base: &str) -> Wda {
        Wda { base: base.trim_end_matches('/').to_string(), agent: agent(30), session: Mutex::new(None) }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, String> {
        let url = format!("{}{}", self.base, path);
        let send = || match (method, body) {
            ("GET", _) => self.agent.get(&url).call(),
            (_, Some(body)) => self.agent.post(&url).send_json(body),
            (_, None) => self.agent.post(&url).send_empty(),
        };
        // A kept connection the other end had closed (a server that closes
        // after each answer, or an idle one dropped): sent again once, on a
        // new one. The request never reached it, so nothing is done twice.
        let sent = match send() {
            Err(e) if stale(&e) => send(),
            other => other,
        };
        let mut resp = sent.map_err(|e| format!("WDA ({url}): {e}"))?;
        let status = resp.status().as_u16();
        let text = resp.body_mut().with_config().limit(MAX_BODY).read_to_string().map_err(|e| format!("WDA ({url}): {e}"))?;
        let data: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if let Some(err) = error_of(status, &data) {
            return Err(err);
        }
        if status >= 400 {
            return Err(format!("WDA {status}: {}", text.chars().take(300).collect::<String>()));
        }
        Ok(data)
    }

    // A call in the session, the session made again once should WDA have
    // forgotten it.
    fn in_session(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, String> {
        let sid = self.session_id()?;
        match self.call(method, &format!("/session/{sid}{path}"), body) {
            Err(e) if forgot_session(&e) => {
                *self.session.lock().unwrap() = None;
                let sid = self.session_id()?;
                self.call(method, &format!("/session/{sid}{path}"), body)
            }
            other => other,
        }
    }

    pub fn ready(&self) -> bool {
        let quick = agent(2);
        quick.get(&format!("{}/status", self.base)).call().is_ok_and(|r| r.status().as_u16() == 200)
    }

    pub fn session_id(&self) -> Result<String, String> {
        let mut held = self.session.lock().unwrap();
        if let Some(sid) = held.as_ref() {
            return Ok(sid.clone());
        }
        let data = self.call("POST", "/session", Some(&json!({ "capabilities": { "alwaysMatch": {} } })))?;
        let sid = session_of(&data).ok_or_else(|| format!("WDA gave no session: {data}"))?;
        *held = Some(sid.clone());
        Ok(sid)
    }

    pub fn window_size(&self) -> Result<Size, String> {
        let v = self.in_session("GET", "/window/size", None)?;
        size_of(&v).ok_or_else(|| format!("WDA gave no size: {v}"))
    }

    pub fn settings(&self, settings: Value) -> Result<(), String> {
        self.in_session("POST", "/appium/settings", Some(&json!({ "settings": settings }))).map(drop)
    }

    // The MJPEG stream: frames a second, JPEG quality (1-100) and the
    // frame's size in percent of the screen's pixels; its frames turned
    // as the screen is (mjpegFixOrientation), so a landscape frame is wide.
    pub fn mjpeg(&self, fps: u32, quality: u32, scale: u32) -> Result<(), String> {
        self.settings(json!({
            "mjpegServerFramerate": fps,
            "mjpegServerScreenshotQuality": quality,
            "mjpegScalingFactor": scale,
            "mjpegFixOrientation": true,
        }))
    }

    fn perform(&self, actions: Vec<Value>) -> Result<(), String> {
        self.in_session("POST", "/actions", Some(&touch(actions))).map(drop)
    }

    pub fn tap(&self, x: f64, y: f64) -> Result<(), String> {
        self.perform(press_at(x, y, 30))
    }

    pub fn long_press(&self, x: f64, y: f64, ms: u64) -> Result<(), String> {
        self.perform(press_at(x, y, ms))
    }

    pub fn drag(&self, steps: &[Step], hold_ms: u64) -> Result<(), String> {
        if steps.is_empty() {
            return Ok(());
        }
        self.perform(drag_actions(steps, hold_ms))
    }

    pub fn swipe(&self, from: (f64, f64), to: (f64, f64), ms: u64, hold_ms: u64) -> Result<(), String> {
        let mut actions = drag_actions(&[(from.0, from.1, 0), (to.0, to.1, ms)], 0);
        if hold_ms > 0 {
            let up = actions.pop().expect("a pointerUp");
            actions.push(json!({ "type": "pause", "duration": hold_ms }));
            actions.push(up);
        }
        self.perform(actions)
    }

    pub fn home(&self) -> Result<(), String> {
        self.call("POST", "/wda/homescreen", None).map(drop)
    }

    // "home", "volumeUp" or "volumeDown".
    pub fn press_button(&self, name: &str) -> Result<(), String> {
        self.in_session("POST", "/wda/pressButton", Some(&json!({ "name": name }))).map(drop)
    }

    pub fn locked(&self) -> Result<bool, String> {
        Ok(self.in_session("GET", "/wda/locked", None)?.get("value").and_then(Value::as_bool).unwrap_or(false))
    }

    pub fn lock(&self) -> Result<(), String> {
        self.in_session("POST", "/wda/lock", None).map(drop)
    }

    pub fn unlock(&self) -> Result<(), String> {
        self.in_session("POST", "/wda/unlock", None).map(drop)
    }

    // Into the field that has the keyboard: "\u{8}" deletes, "\n" is return.
    pub fn type_text(&self, text: &str) -> Result<(), String> {
        if text.is_empty() {
            return Ok(());
        }
        let keys: Vec<String> = text.chars().map(String::from).collect();
        self.in_session("POST", "/wda/keys", Some(&json!({ "value": keys }))).map(drop)
    }

    pub fn set_pasteboard(&self, text: &str) -> Result<(), String> {
        let content = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
        self.in_session("POST", "/wda/setPasteboard", Some(&json!({ "content": content, "contentType": "plaintext" }))).map(drop)
    }

    pub fn pasteboard(&self) -> Result<String, String> {
        let v = self.in_session("POST", "/wda/getPasteboard", Some(&json!({ "contentType": "plaintext" })))?;
        let b64 = v.get("value").and_then(Value::as_str).unwrap_or("");
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64).map_err(|e| format!("pasteboard: {e}"))?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    // The screen at its full size, as a PNG.
    pub fn screenshot(&self) -> Result<Vec<u8>, String> {
        let v = self.call("GET", "/screenshot", None)?;
        let b64 = v.get("value").and_then(Value::as_str).ok_or("WDA gave no screenshot")?;
        base64::engine::general_purpose::STANDARD.decode(b64).map_err(|e| format!("screenshot: {e}"))
    }
}

fn stale(e: &ureq::Error) -> bool {
    let text = e.to_string().to_lowercase();
    ["disconnected", "connection reset", "broken pipe", "connection aborted"].iter().any(|s| text.contains(s))
}

// WDA's error, from a W3C error body ({value: {error, message}}), if it is one.
fn error_of(status: u16, data: &Value) -> Option<String> {
    let value = data.get("value")?;
    let error = value.get("error")?.as_str()?;
    let message = value.get("message").and_then(Value::as_str).unwrap_or("");
    Some(format!("WDA {status} {error}: {message}"))
}

// WDA saying it does not know the session (not any error whose URL has
// "/session/" in it: a lost connection is not a forgotten session).
fn forgot_session(error: &str) -> bool {
    let e = error.to_lowercase();
    e.contains("invalid session id") || e.contains("no such session") || e.contains("session does not exist")
}

// A new session's id: at the top (older WDA) or in its value (W3C).
fn session_of(data: &Value) -> Option<String> {
    data.get("sessionId").and_then(Value::as_str).or_else(|| data.get("value")?.get("sessionId")?.as_str()).map(String::from)
}

fn size_of(data: &Value) -> Option<Size> {
    let v = data.get("value")?;
    Some(Size { width: v.get("width")?.as_f64()?, height: v.get("height")?.as_f64()? })
}

fn touch(actions: Vec<Value>) -> Value {
    json!({
        "actions": [{
            "type": "pointer",
            "id": "finger1",
            "parameters": { "pointerType": "touch" },
            "actions": actions,
        }]
    })
}

fn press_at(x: f64, y: f64, ms: u64) -> Vec<Value> {
    vec![
        json!({ "type": "pointerMove", "duration": 0, "x": x.round(), "y": y.round() }),
        json!({ "type": "pointerDown", "button": 0 }),
        json!({ "type": "pause", "duration": ms }),
        json!({ "type": "pointerUp", "button": 0 }),
    ]
}

// Down at the first step, held there hold_ms (a press before the drag: an
// icon picked up), along the others each in its own time, up at the last.
fn drag_actions(steps: &[Step], hold_ms: u64) -> Vec<Value> {
    let (x0, y0, _) = steps[0];
    let mut out = vec![
        json!({ "type": "pointerMove", "duration": 0, "x": x0.round(), "y": y0.round() }),
        json!({ "type": "pointerDown", "button": 0 }),
    ];
    if hold_ms > 0 {
        out.push(json!({ "type": "pause", "duration": hold_ms }));
    }
    for &(x, y, dt) in &steps[1..] {
        out.push(json!({ "type": "pointerMove", "duration": dt.max(1), "x": x.round(), "y": y.round() }));
    }
    out.push(json!({ "type": "pointerUp", "button": 0 }));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_is_found_where_either_wda_puts_it() {
        assert_eq!(session_of(&json!({ "sessionId": "A", "value": {} })).as_deref(), Some("A"));
        assert_eq!(session_of(&json!({ "value": { "sessionId": "B" } })).as_deref(), Some("B"));
        assert_eq!(session_of(&json!({ "value": {} })), None);
    }

    #[test]
    fn errors_are_read_from_the_w3c_body() {
        let e = error_of(404, &json!({ "value": { "error": "invalid session id", "message": "gone" } })).unwrap();
        assert!(e.contains("invalid session id") && e.contains("gone"));
        assert_eq!(error_of(200, &json!({ "value": { "width": 1 } })), None);
        assert_eq!(error_of(200, &json!({ "value": null })), None);
    }

    #[test]
    fn a_drag_goes_down_holds_moves_and_lifts() {
        let a = drag_actions(&[(10.4, 20.6, 0), (30.0, 40.0, 25), (50.0, 60.0, 0)], 400);
        let kinds: Vec<&str> = a.iter().map(|v| v["type"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["pointerMove", "pointerDown", "pause", "pointerMove", "pointerMove", "pointerUp"]);
        assert_eq!(a[0]["x"], json!(10.0));
        assert_eq!(a[0]["y"], json!(21.0));
        assert_eq!(a[2]["duration"], json!(400));
        // Never a move of no time: WDA takes 0 as "jump", which is no drag.
        assert_eq!(a[4]["duration"], json!(1));
    }

    // A WDA of a few lines on a port here: each request's method, path and
    // body kept; answers from `answer`.
    // Each request: its method, path and body.
    type Seen = std::sync::Arc<Mutex<Vec<(String, String, String)>>>;

    pub fn fake_wda(answer: fn(&str, &str) -> (u16, String)) -> (String, Seen) {
        use std::io::{BufRead, BufReader, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let kept = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                if reader.read_line(&mut first).is_err() {
                    continue;
                }
                let mut len = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = line.to_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; len];
                reader.read_exact(&mut body).unwrap();
                let mut parts = first.split_whitespace();
                let (method, path) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
                let body = String::from_utf8_lossy(&body).into_owned();
                let (status, text) = answer(&method, &path);
                kept.lock().unwrap().push((method, path, body));
                let mut out = stream;
                let _ = write!(out, "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len());
            }
        });
        (base, seen)
    }

    #[test]
    fn a_tap_opens_a_session_once_and_sends_one_touch() {
        let (base, seen) = fake_wda(|method, path| match (method, path) {
            ("POST", "/session") => (200, r#"{"value":{"sessionId":"S1"},"sessionId":"S1"}"#.into()),
            ("GET", "/session/S1/window/size") => (200, r#"{"value":{"width":390,"height":844}}"#.into()),
            _ => (200, r#"{"value":null}"#.into()),
        });
        let w = Wda::new(&base);
        assert_eq!(w.window_size().unwrap(), Size { width: 390.0, height: 844.0 });
        w.tap(195.2, 422.7).unwrap();
        w.type_text("你好\u{8}").unwrap();
        let seen = seen.lock().unwrap();
        let paths: Vec<&str> = seen.iter().map(|(_, p, _)| p.as_str()).collect();
        assert_eq!(paths, ["/session", "/session/S1/window/size", "/session/S1/actions", "/session/S1/wda/keys"]);
        let touch: Value = serde_json::from_str(&seen[2].2).unwrap();
        assert_eq!(touch["actions"][0]["actions"][0]["x"], json!(195.0));
        assert_eq!(touch["actions"][0]["actions"][0]["y"], json!(423.0));
        let keys: Value = serde_json::from_str(&seen[3].2).unwrap();
        assert_eq!(keys["value"], json!(["你", "好", "\u{8}"]));
    }

    #[test]
    fn a_forgotten_session_is_made_again_and_the_call_tried_again() {
        static MADE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let (base, seen) = fake_wda(|method, path| match (method, path) {
            ("POST", "/session") => {
                let n = MADE.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                (200, format!(r#"{{"value":{{"sessionId":"S{n}"}}}}"#))
            }
            ("POST", "/session/S1/wda/lock") => (404, r#"{"value":{"error":"invalid session id","message":"gone"}}"#.into()),
            _ => (200, r#"{"value":null}"#.into()),
        });
        let w = Wda::new(&base);
        w.lock().unwrap();
        let paths: Vec<String> = seen.lock().unwrap().iter().map(|(_, p, _)| p.clone()).collect();
        assert_eq!(paths, ["/session", "/session/S1/wda/lock", "/session", "/session/S2/wda/lock"]);
    }

    #[test]
    fn a_wda_error_is_said_not_retried() {
        let (base, seen) = fake_wda(|method, path| match (method, path) {
            ("POST", "/session") => (200, r#"{"sessionId":"S1","value":{}}"#.into()),
            _ => (500, r#"{"value":{"error":"unknown error","message":"no keyboard"}}"#.into()),
        });
        let e = Wda::new(&base).type_text("a").unwrap_err();
        assert!(e.contains("no keyboard"), "{e}");
        assert_eq!(seen.lock().unwrap().len(), 2);
    }
}

