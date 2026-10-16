// iPhone Link on its own: one window (phone.rs, src/phone/). Shared holds
// what phone.rs takes of the app, with the names Wakuwaku's Shared has (app,
// dir, log, phone), so that phone.rs and phone/ go into Wakuwaku as they are
// (docs/DEVELOPMENT.md, "合进 Wakuwaku").
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod data;
mod phone;

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Manager, RunEvent};

pub struct Shared {
    pub app: AppHandle,
    pub dir: PathBuf,
    pub phone: Mutex<phone::Phone>,
    // debug.log, written while a file debug.on is in the data folder.
    log: Mutex<Option<File>>,
}

impl Shared {
    pub fn log(&self, line: &str) {
        let ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let whole = format!("{ms} {line}\n");
        if cfg!(debug_assertions) {
            eprint!("{whole}");
        }
        if let Some(f) = self.log.lock().unwrap().as_mut() {
            let _ = f.write_all(whole.as_bytes());
        }
    }
}

pub(crate) fn shared(app: &AppHandle) -> Arc<Shared> {
    app.state::<Arc<Shared>>().inner().clone()
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            phone::phone_state,
            phone::phone_setup,
            phone::phone_devices,
            phone::phone_connect,
            phone::phone_disconnect,
            phone::phone_stream,
            phone::phone_seen,
            phone::phone_size,
            phone::phone_tap,
            phone::phone_press,
            phone::phone_drag,
            phone::phone_scroll,
            phone::phone_type,
            phone::phone_button,
            phone::phone_quality,
            phone::phone_clip_push,
            phone::phone_clip_pull,
            phone::phone_screenshot,
            phone::phone_photos,
        ])
        .setup(|app| {
            let dir = data::folder();
            let _ = std::fs::create_dir_all(&dir);
            let log = dir.join("debug.on").exists().then(|| OpenOptions::new().create(true).append(true).open(dir.join("debug.log")).ok()).flatten();
            let sh = Arc::new(Shared { app: app.handle().clone(), dir, phone: Mutex::default(), log: Mutex::new(log) });
            app.manage(sh.clone());
            sh.log(&format!("iPhone Link {} starting, data in {}", env!("CARGO_PKG_VERSION"), sh.dir.display()));
            phone::open(&sh)?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("iPhone Link could not start")
        .run(|app, event| {
            // go-ios's children never outlive the app.
            if let RunEvent::Exit = event {
                phone::disconnect(&shared(app));
            }
        });
}
