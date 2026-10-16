// The phone's screen as WDA's MJPEG server sends it (the phone's 9100,
// forwarded here): one HTTP answer that never ends, JPEGs one after another,
// each after a few header lines (--BoundaryString, Content-type,
// Content-Length). Cut into frames by its Content-Length, or by the JPEG's
// own start and end marks where a frame came without one.
//
// The stream gone three times in a row with no frame (an old WDA without the
// MJPEG server, the port taken): screenshots instead, one after another,
// slower but a screen all the same, and the stream tried again every 15 s.
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::wda::Wda;

const SOI: [u8; 2] = [0xFF, 0xD8];
const EOI: [u8; 2] = [0xFF, 0xD9];
// What is kept waiting for a frame's start (its header lines) or end.
const MAX_HEADER: usize = 64 << 10;
const MAX_FRAME: usize = 8 << 20;

#[derive(Default)]
pub struct Splitter {
    buf: Vec<u8>,
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    hay.get(from..)?.windows(needle.len()).position(|w| w == needle).map(|i| i + from)
}

// The Content-Length in the header lines just before a frame.
fn content_length(header: &[u8]) -> Option<usize> {
    let text = String::from_utf8_lossy(header);
    text.lines().rev().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim().eq_ignore_ascii_case("content-length").then(|| value.trim().parse().ok()).flatten()
    })
}

impl Splitter {
    // The frames the bytes complete.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        self.buf.extend_from_slice(bytes);
        let mut frames = Vec::new();
        loop {
            let Some(start) = find(&self.buf, &SOI, 0) else {
                if self.buf.len() > MAX_HEADER {
                    let keep = self.buf.split_off(self.buf.len() - 1);
                    self.buf = keep;
                }
                break;
            };
            if let Some(len) = content_length(&self.buf[..start]) {
                if self.buf.len() < start + len {
                    break;
                }
                frames.push(self.buf[start..start + len].to_vec());
                self.buf.drain(..start + len);
                continue;
            }
            let Some(end) = find(&self.buf, &EOI, start + 2) else {
                if self.buf.len() > MAX_FRAME {
                    self.buf.clear();
                }
                break;
            };
            frames.push(self.buf[start..end + 2].to_vec());
            self.buf.drain(..end + 2);
        }
        frames
    }
}

// Frames to `frame` until `stop`; `frame` says false when the page is
// gone. What it falls back to, and back from, said to `status`.
pub fn run(url: &str, wda: Arc<Wda>, stop: Arc<AtomicBool>, frame: impl Fn(Vec<u8>) -> bool, status: impl Fn(&str)) {
    let mut failures = 0;
    while !stop.load(Ordering::Relaxed) {
        match stream(url, &stop, &frame) {
            Some(true) => failures = 0,
            Some(false) => return,
            None => failures += 1,
        }
        if stop.load(Ordering::Relaxed) {
            return;
        }
        if failures >= 3 {
            status("screenshots");
            if !screenshots(&wda, &stop, &frame) {
                return;
            }
            failures = 0;
            status("stream");
        } else {
            std::thread::sleep(Duration::from_millis(500));
        }
    }
}

// One connection to the stream, read till it ends: Some(true) if a frame
// came, Some(false) if the page is gone, None if none came.
fn stream(url: &str, stop: &AtomicBool, frame: &impl Fn(Vec<u8>) -> bool) -> Option<bool> {
    // No time limit on the whole answer: it never ends. One on connecting.
    let agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(3)))
        .timeout_recv_response(Some(Duration::from_secs(10)))
        .proxy(None)
        .build()
        .new_agent();
    let resp = agent.get(url).call().ok()?;
    let mut reader = resp.into_body().into_reader();
    let mut splitter = Splitter::default();
    let mut chunk = vec![0u8; 64 << 10];
    let mut got = false;
    while !stop.load(Ordering::Relaxed) {
        let n = match reader.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        for f in splitter.push(&chunk[..n]) {
            got = true;
            if !frame(f) {
                return Some(false);
            }
        }
    }
    got.then_some(true)
}

// Screenshots for 15 s; false if the page is gone.
fn screenshots(wda: &Wda, stop: &AtomicBool, frame: &impl Fn(Vec<u8>) -> bool) -> bool {
    let until = Instant::now() + Duration::from_secs(15);
    while !stop.load(Ordering::Relaxed) && Instant::now() < until {
        match wda.screenshot() {
            Ok(png) => {
                if !frame(png) {
                    return false;
                }
            }
            Err(_) => std::thread::sleep(Duration::from_secs(1)),
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg(fill: u8, len: usize) -> Vec<u8> {
        let mut v = SOI.to_vec();
        v.extend(std::iter::repeat_n(fill, len));
        v.extend(EOI);
        v
    }

    fn split_in(data: &[u8], size: usize) -> Vec<Vec<u8>> {
        let mut s = Splitter::default();
        data.chunks(size).flat_map(|c| s.push(c)).collect()
    }

    #[test]
    fn frames_are_cut_by_their_content_length_in_any_chunks() {
        let frames = [jpeg(b'A', 100), jpeg(b'B', 50), jpeg(b'C', 3)];
        let mut data = Vec::new();
        for f in &frames {
            data.extend(format!("--BoundaryString\r\nContent-type: image/jpg\r\nContent-Length: {}\r\n\r\n", f.len()).bytes());
            data.extend(f);
            data.extend(b"\r\n\r\n");
        }
        for size in [1, 2, 7, 64, 100_000] {
            assert_eq!(split_in(&data, size), frames, "chunks of {size}");
        }
    }

    #[test]
    fn frames_without_a_length_are_cut_by_their_marks() {
        let frames = [jpeg(b'A', 10), jpeg(b'B', 20)];
        let mut data = Vec::new();
        for f in &frames {
            data.extend(b"--BoundaryString\r\nContent-type: image/jpg\r\n\r\n");
            data.extend(f);
        }
        for size in [1, 3, 1000] {
            assert_eq!(split_in(&data, size), frames, "chunks of {size}");
        }
    }

    #[test]
    fn the_length_is_read_whatever_its_case() {
        assert_eq!(content_length(b"--B\r\ncontent-length:  42\r\n\r\n"), Some(42));
        assert_eq!(content_length(b"--B\r\nContent-type: image/jpg\r\n\r\n"), None);
    }

    #[test]
    fn the_stream_is_read_frame_by_frame_until_stopped() {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut req = [0u8; 1024];
            let _ = std::io::Read::read(&mut s, &mut req);
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary=--BoundaryString\r\n\r\n");
            for i in 0..50u8 {
                let f = jpeg(i, 1000);
                let head = format!("--BoundaryString\r\nContent-type: image/jpg\r\nContent-Length: {}\r\n\r\n", f.len());
                if s.write_all(head.as_bytes()).and_then(|_| s.write_all(&f)).and_then(|_| s.write_all(b"\r\n\r\n")).is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        let stop = Arc::new(AtomicBool::new(false));
        let got = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (s2, g2) = (stop.clone(), got.clone());
        let wda = Arc::new(Wda::new("http://127.0.0.1:9"));
        run(&url, wda, stop.clone(), move |f| {
            let mut g = g2.lock().unwrap();
            g.push(f);
            if g.len() == 3 {
                s2.store(true, Ordering::Relaxed);
            }
            true
        }, |_| {});
        let got = got.lock().unwrap();
        assert_eq!(got.len(), 3);
        assert_eq!(got[2], jpeg(2, 1000));
    }
}

