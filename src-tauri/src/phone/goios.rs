// The phone reached through go-ios (github.com/danielpaulus/go-ios, MIT): its
// `ios` program, got by goios_setup.rs, run for each step. Its answers are
// JSON on stdout; its log JSON lines on stderr, the last of which says why
// when it fails (exitIfError).
//
//   ios list --details                 the phones on USB: udid, iOS, model
//   ios devicename                     the name the person gave it
//   ios apps --list                    "bundleid name version" a line: WDA's bundle id
//   ios tunnel start --userspace       iOS 17+: the tunnel its developer services
//                                      need, in user space (no administrator, no
//                                      wintun.dll); the others find it by its agent
//                                      at 127.0.0.1:60105
//   ios image auto                     the developer disk image, downloaded once
//                                      (into basedir) and mounted
//   ios runwda                         WebDriverAgent started, kept while it runs
//   ios forward --port=a:b ...         ports here to the phone's, kept while it runs
//   ios fsync pull                     files off the phone (the camera roll: /DCIM)
//
// The ones kept running (Kept) are its children, stopped with the
// connection and when the app ends; their lines go to the log.
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

pub const AGENT_PORT: u16 = 60105;
pub const WDA_XCTEST: &str = "WebDriverAgentRunner.xctest";

pub type Log = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Device {
    pub udid: String,
    pub name: String,
    pub ios: String,
    pub model: String,
    pub connection: String,
}

impl Device {
    pub fn ios_major(&self) -> u32 {
        self.ios.split('.').next().and_then(|m| m.parse().ok()).unwrap_or(0)
    }
}

#[derive(Clone)]
pub struct GoIos {
    pub exe: PathBuf,
    // Where its pair records and developer images are kept, and its
    // working folder (the tunnel writes there).
    pub dir: PathBuf,
}

fn command(exe: &Path, dir: &Path, args: &[String]) -> Command {
    let mut cmd = Command::new(exe);
    cmd.args(args).current_dir(dir).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    // Never ENABLE_GO_IOS_AGENT: an agent it started itself would outlive us.
    cmd.env_remove("ENABLE_GO_IOS_AGENT");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

// Why a run failed, from its stderr: the last JSON log line's msg and
// err, else the last line as it is.
pub fn reason(stderr: &str) -> String {
    for line in stderr.lines().rev() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            let msg = v.get("msg").and_then(Value::as_str).unwrap_or("");
            let err = v.get("err").or_else(|| v.get("error")).and_then(Value::as_str).unwrap_or("");
            if !msg.is_empty() || !err.is_empty() {
                return [msg, err].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(": ");
            }
            continue;
        }
        return line.to_string();
    }
    String::new()
}

// The phones from `ios list --details`: {"deviceList":[{Udid, ProductType,
// ProductVersion, ConnectionType, ...}]}.
pub fn parse_list(out: &str) -> Vec<Device> {
    let v: Value = serde_json::from_str(out.trim()).unwrap_or(Value::Null);
    let s = |d: &Value, k: &str| d.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    v.get("deviceList")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter(|d| !s(d, "Udid").is_empty())
                .map(|d| Device { udid: s(d, "Udid"), name: String::new(), ios: s(d, "ProductVersion"), model: s(d, "ProductType"), connection: s(d, "ConnectionType") })
                .collect()
        })
        .unwrap_or_default()
}

// WebDriverAgent's runner among the apps (`ios apps --list`, a line each:
// "bundleid name version"): signed with a free Apple ID its bundle id is
// changed (Sideloadly makes it the team's own), so found by its name too
// ("WebDriverAgentRunner-Runner"); its bundle id taken, the one ending in
// .xctrunner first.
pub fn find_wda(apps: &str) -> Option<String> {
    let mut found: Vec<&str> = apps.lines().filter(|l| l.to_lowercase().contains("webdriveragent")).filter_map(|l| l.split_whitespace().next()).collect();
    found.sort_by_key(|id| (!id.ends_with(".xctrunner"), *id));
    found.first().map(|s| s.to_string())
}

impl GoIos {
    // One run to its end (or `limit`, then killed): its stdout, or why not.
    pub fn run(&self, args: &[String], limit: Duration) -> Result<String, String> {
        let mut child = command(&self.exe, &self.dir, args).spawn().map_err(|e| format!("{}: {e}", self.exe.display()))?;
        job::hold(&child);
        let mut out_pipe = child.stdout.take().expect("piped");
        let mut err_pipe = child.stderr.take().expect("piped");
        let out = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = out_pipe.read_to_string(&mut s);
            s
        });
        let err = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = err_pipe.read_to_string(&mut s);
            s
        });
        let until = Instant::now() + limit;
        let status = loop {
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                break status;
            }
            if Instant::now() > until {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("ios {}: timed out", args.first().map(String::as_str).unwrap_or("")));
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let (out, err) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
        if status.success() {
            Ok(out)
        } else {
            let why = reason(&err);
            Err(if why.is_empty() { format!("ios {} exited with {status}", args.join(" ")) } else { why })
        }
    }

    fn args(list: &[&str], udid: Option<&str>) -> Vec<String> {
        let mut v: Vec<String> = list.iter().map(|s| s.to_string()).collect();
        if let Some(udid) = udid {
            v.push(format!("--udid={udid}"));
        }
        v
    }

    pub fn devices(&self) -> Result<Vec<Device>, String> {
        let mut list = parse_list(&self.run(&Self::args(&["list", "--details"], None), Duration::from_secs(20))?);
        for d in &mut list {
            d.name = self
                .run(&Self::args(&["devicename"], Some(&d.udid)), Duration::from_secs(10))
                .ok()
                .and_then(|out| serde_json::from_str::<Value>(out.trim()).ok())
                .and_then(|v| v.get("devicename").and_then(Value::as_str).map(String::from))
                .unwrap_or_else(|| "iPhone".into());
        }
        Ok(list)
    }

    // The apps the person installed, as `ios apps --list` gives them.
    pub fn user_apps(&self, udid: &str) -> Result<String, String> {
        self.run(&Self::args(&["apps", "--list"], Some(udid)), Duration::from_secs(30))
    }

    pub fn agent_running() -> bool {
        super::wda::agent(1).get(&format!("http://127.0.0.1:{AGENT_PORT}/health")).call().is_ok_and(|r| r.status().as_u16() == 200)
    }

    fn agent_ready() -> bool {
        super::wda::agent(1).get(&format!("http://127.0.0.1:{AGENT_PORT}/ready")).call().is_ok_and(|r| r.status().as_u16() == 200)
    }

    // iOS 17+'s tunnel agent, unless one runs already (another go-ios's,
    // left running: used as it is). Ready when its first tunnels are up.
    pub fn tunnel(&self, log: &Log) -> Result<Option<Kept>, String> {
        if Self::agent_running() {
            log("tunnel: a go-ios agent runs already");
            return Ok(None);
        }
        let records = self.dir.join("pair-records");
        let _ = std::fs::create_dir_all(&records);
        let kept = self.keep("tunnel", &["tunnel".into(), "start".into(), "--userspace".into(), format!("--pair-record-path={}", records.display())], log)?;
        let until = Instant::now() + Duration::from_secs(40);
        while Instant::now() < until {
            if Self::agent_ready() {
                return Ok(Some(kept));
            }
            if !kept.alive() {
                return Err(kept.last_words());
            }
            std::thread::sleep(Duration::from_millis(300));
        }
        Err(format!("not ready in 40 s: {}", kept.last_words()))
    }

    pub fn mount(&self, udid: &str) -> Result<String, String> {
        let base = self.dir.join("devimages");
        let _ = std::fs::create_dir_all(&base);
        self.run(&Self::args(&["image", "auto", &format!("--basedir={}", base.display())], Some(udid)), Duration::from_secs(900))
    }

    pub fn forward(&self, udid: &str, ports: &[(u16, u16)], log: &Log) -> Result<Kept, String> {
        let mut args = vec!["forward".to_string()];
        args.extend(ports.iter().map(|(here, there)| format!("--port={here}:{there}")));
        args.push(format!("--udid={udid}"));
        self.keep("forward", &args, log)
    }

    pub fn run_wda(&self, udid: &str, bundle: &str, log: &Log) -> Result<Kept, String> {
        let args = Self::args(&["runwda", &format!("--bundleid={bundle}"), &format!("--testrunnerbundleid={bundle}"), &format!("--xctestconfig={WDA_XCTEST}")], Some(udid));
        self.keep("wda", &args, log)
    }

    pub fn pull(&self, udid: &str, from: &str, to: &Path) -> Result<String, String> {
        let args = Self::args(&["fsync", "pull", &format!("--srcPath={from}"), &format!("--dstPath={}", to.display())], Some(udid));
        self.run(&args, Duration::from_secs(6 * 3600))
    }

    pub fn keep(&self, name: &str, args: &[String], log: &Log) -> Result<Kept, String> {
        let mut child = command(&self.exe, &self.dir, args).spawn().map_err(|e| format!("{}: {e}", self.exe.display()))?;
        job::hold(&child);
        let last = Arc::new(Mutex::new(String::new()));
        for pipe in [child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>), child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>)].into_iter().flatten() {
            let (log, last, name) = (log.clone(), last.clone(), name.to_string());
            std::thread::spawn(move || {
                for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                    if line.trim().is_empty() {
                        continue;
                    }
                    log(&format!("[{name}] {line}"));
                    *last.lock().unwrap() = line;
                }
            });
        }
        Ok(Kept { name: name.to_string(), child: Mutex::new(child), last })
    }
}

// Every go-ios started here in one job that ends them all as it closes,
// which it does as this process ends however it ends: a crash, or Task
// Manager's End task, leaves no ios.exe holding a port or the phone's WDA
// (the window closed as usual stops them itself: phone::disconnect). Its
// kernel32 calls declared as Wakuwaku's detach.rs declares its own, so no
// crate is added. Jobs nest (Windows 8 and later): this one in whatever job
// the app runs in. Any call failing: the child just runs outside it.
#[cfg(windows)]
mod job {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    use std::sync::OnceLock;

    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;

    #[repr(C)]
    struct BasicLimits {
        per_process_user_time: i64,
        per_job_user_time: i64,
        flags: u32,
        min_working_set: usize,
        max_working_set: usize,
        active_processes: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    struct ExtendedLimits {
        basic: BasicLimits,
        io: [u64; 6],
        process_memory: usize,
        job_memory: usize,
        peak_process_memory: usize,
        peak_job_memory: usize,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateJobObjectW(attrs: *const c_void, name: *const u16) -> isize;
        fn SetInformationJobObject(job: isize, class: i32, info: *const c_void, length: u32) -> i32;
        fn AssignProcessToJobObject(job: isize, process: isize) -> i32;
    }

    // The job, made once and never closed by us: the system closes it with
    // the process.
    fn job() -> Option<isize> {
        static JOB: OnceLock<Option<isize>> = OnceLock::new();
        *JOB.get_or_init(|| {
            // SAFETY: a new unnamed job with default security; its limits
            // set from our own zeroed struct of the documented layout.
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if job == 0 {
                    return None;
                }
                let mut limits: ExtendedLimits = std::mem::zeroed();
                limits.basic.flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let set = SetInformationJobObject(job, JOB_OBJECT_EXTENDED_LIMIT_INFORMATION, (&limits as *const ExtendedLimits).cast(), std::mem::size_of::<ExtendedLimits>() as u32);
                (set != 0).then_some(job)
            }
        })
    }

    pub fn hold(child: &std::process::Child) {
        if let Some(job) = job() {
            // SAFETY: the child's process handle, alive while `child` is.
            unsafe { AssignProcessToJobObject(job, child.as_raw_handle() as isize) };
        }
    }
}

#[cfg(not(windows))]
mod job {
    pub fn hold(_: &std::process::Child) {}
}

// A go-ios kept running; stopped when dropped.
pub struct Kept {
    pub name: String,
    child: Mutex<Child>,
    last: Arc<Mutex<String>>,
}

impl Kept {
    pub fn alive(&self) -> bool {
        self.child.lock().unwrap().try_wait().is_ok_and(|s| s.is_none())
    }

    // Why it ended, from its last line.
    pub fn last_words(&self) -> String {
        reason(&self.last.lock().unwrap())
    }

    pub fn stop(&self) {
        let mut child = self.child.lock().unwrap();
        if child.try_wait().is_ok_and(|s| s.is_none()) {
            let _ = child.kill();
        }
        let _ = child.wait();
    }
}

impl Drop for Kept {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phones_are_read_from_the_detailed_list() {
        let out = r#"{"deviceList":[{"Udid":"00008110-A","ProductName":"iPhone OS","ProductType":"iPhone14,2","ProductVersion":"17.5.1","ConnectionType":"USB"},{"Udid":""}]}"#;
        let list = parse_list(out);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].udid, "00008110-A");
        assert_eq!(list[0].ios_major(), 17);
        assert_eq!(list[0].model, "iPhone14,2");
        assert!(parse_list("not json").is_empty());
    }

    #[test]
    fn wda_is_found_whatever_sideloadly_called_it() {
        let apps = "com.apple.mobilesafari Safari 17.0\ncom.foo.WebDriverAgentRunner Runner 1.0\ncom.foo.WebDriverAgentRunner.xctrunner WebDriverAgentRunner-Runner 1.0\n";
        assert_eq!(find_wda(apps).as_deref(), Some("com.foo.WebDriverAgentRunner.xctrunner"));
        // Its bundle id made the team's own: found by its name.
        let renamed = "com.tencent.xin WeChat 8.0\ncom.ABCDE12345.runner WebDriverAgentRunner-Runner 1.0\n";
        assert_eq!(find_wda(renamed).as_deref(), Some("com.ABCDE12345.runner"));
        assert_eq!(find_wda("com.apple.mobilesafari Safari 17.0\n"), None);
    }

    #[test]
    fn the_reason_is_the_last_log_line_that_says_one() {
        let err = "{\"level\":\"INFO\",\"msg\":\"starting\"}\n{\"level\":\"ERROR\",\"msg\":\"failed to forward port\",\"err\":\"address in use\"}\n\n";
        assert_eq!(reason(err), "failed to forward port: address in use");
        assert_eq!(reason("plain words\n"), "plain words");
        assert_eq!(reason(""), "");
    }

    // An `ios` that answers as go-ios 1.3.2 does, for the steps' parsing
    // and the children's stopping.
    #[cfg(unix)]
    fn fake_ios() -> GoIos {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("iphone-link-goios-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::create_dir_all(&dir);
        let exe = dir.join("ios");
        std::fs::write(
            &exe,
            r#"#!/bin/sh
case "$1" in
  list) echo '{"deviceList":[{"Udid":"U1","ProductName":"iPhone OS","ProductType":"iPhone15,2","ProductVersion":"17.4","ConnectionType":"USB"}]}' ;;
  devicename) echo '{"devicename":"Zhihan的iPhone"}' ;;
  apps) echo 'com.apple.Preferences Settings 1'; echo 'com.zh.WebDriverAgentRunner.xctrunner WebDriverAgentRunner-Runner 1.0' ;;
  image) echo '{"level":"ERROR","msg":"failed mounting image","err":"device locked"}' >&2; exit 1 ;;
  forward) echo '{"level":"INFO","msg":"Start listening on port 8100 forwarding to port 8100 on device"}' >&2; exec sleep 30 ;;
  *) exit 2 ;;
esac
"#,
        )
        .unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        GoIos { exe, dir }
    }

    #[cfg(unix)]
    #[test]
    fn the_steps_read_go_ios_as_it_answers() {
        let g = fake_ios();
        let list = g.devices().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Zhihan的iPhone");
        assert_eq!(list[0].ios_major(), 17);
        assert_eq!(find_wda(&g.user_apps("U1").unwrap()).as_deref(), Some("com.zh.WebDriverAgentRunner.xctrunner"));
        assert_eq!(g.mount("U1").unwrap_err(), "failed mounting image: device locked");
    }

    #[cfg(unix)]
    #[test]
    fn a_kept_go_ios_logs_its_lines_and_is_stopped() {
        let g = fake_ios();
        let lines = Arc::new(Mutex::new(Vec::<String>::new()));
        let l2 = lines.clone();
        let log: Log = Arc::new(move |s: &str| l2.lock().unwrap().push(s.to_string()));
        let kept = g.forward("U1", &[(18100, 8100)], &log).unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        while lines.lock().unwrap().is_empty() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(kept.alive());
        assert!(lines.lock().unwrap()[0].starts_with("[forward] "));
        assert!(kept.last_words().contains("Start listening"));
        kept.stop();
        assert!(!kept.alive());
    }

    #[cfg(unix)]
    #[test]
    fn a_run_past_its_time_is_killed() {
        let g = fake_ios();
        let started = Instant::now();
        let e = g.run(&["forward".into()], Duration::from_millis(300)).unwrap_err();
        assert!(e.contains("timed out"), "{e}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}

