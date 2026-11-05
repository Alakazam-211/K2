//! TEST ONLY. K2's `k2 app init --template tauri --voice` files, built into a
//! real Tauri 2 app (stock crates), checking what a call needs from the
//! webview: WKWebView on macOS, WebView2 on Windows.
//!
//! The window loads a page from a local server on 127.0.0.1: a remote origin
//! (not tauri://), with the CSP the template's README gives. The page reports
//! back to that server; this prints one `K2CHECK` JSON line and a PASS/FAIL
//! line per check, then exits 0 only if every required check passed.
//!
//! The window is hidden, as when a person hides or minimises the app. Env:
//!   K2_CHECK_THROTTLE=default   leave the template's backgroundThrottling off
//!                               (the control run: the hidden page may stall)
//!   K2_CHECK_STAMP=<word>       written to IndexedDB; the run prints the one
//!                               the previous run wrote (persistence)
//!   K2_CHECK_TIMEOUT=<secs>     how long to wait for the page (default 150)
//!   K2_CHECK_PORT=<port>        the page's port (default: any free one); keep
//!                               it the same across runs for the IndexedDB check
//!   K2_CHECK_MIC=1              this Mac has a mic and a person answers the
//!                               macOS prompt: the mic checks become required

#[allow(dead_code)]
mod k2_voice;

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::{WebviewUrl, WebviewWindowBuilder};

/// Every media permission request the webview made, and the answer.
static PERMISSIONS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The template README's CSP, as the App's server would send it.
const CSP_VOICE: &str =
    "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self' blob:; connect-src 'self' ipc: http://ipc.localhost; media-src 'self' blob: mediastream:";
/// The same without 'wasm-unsafe-eval': WebAssembly must be refused.
const CSP_NO_WASM: &str = "default-src 'self'; script-src 'self'; worker-src 'self' blob:; connect-src 'self'";

/// `(i32, i32) -> i32` add, exported as `add`.
const ADD_WASM: &[u8] = &[
    0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x07, 0x01, 0x60, 0x02, 0x7f, 0x7f, 0x01, 0x7f, 0x03, 0x02, 0x01, 0x00, 0x07, 0x07, 0x01,
    0x03, 0x61, 0x64, 0x64, 0x00, 0x00, 0x0a, 0x09, 0x01, 0x07, 0x00, 0x20, 0x00, 0x20, 0x01, 0x6a, 0x0b,
];

type Results = Arc<Mutex<BTreeMap<String, Value>>>;

fn page(path: &str) -> Option<(&'static str, &'static [u8], &'static str)> {
    let html = "text/html; charset=utf-8";
    let js = "text/javascript";
    Some(match path {
        "/" => (html, include_bytes!("../../page/check.html"), CSP_VOICE),
        "/nowasm" => (html, include_bytes!("../../page/nowasm.html"), CSP_NO_WASM),
        "/other" => (html, include_bytes!("../../page/other.html"), CSP_VOICE),
        "/check.js" => (js, include_bytes!("../../page/check.js"), CSP_VOICE),
        "/nowasm.js" => (js, include_bytes!("../../page/nowasm.js"), CSP_NO_WASM),
        "/other.js" => (js, include_bytes!("../../page/other.js"), CSP_VOICE),
        "/noise-worklet.js" => (js, include_bytes!("../../page/noise-worklet.js"), CSP_VOICE),
        "/e2ee-worker.js" => (js, include_bytes!("../../page/e2ee-worker.js"), CSP_VOICE),
        "/add.wasm" => ("application/wasm", ADD_WASM, CSP_VOICE),
        _ => return None,
    })
}

fn respond(s: &mut TcpStream, status: &str, ctype: &str, csp: Option<&str>, body: &[u8]) {
    let mut head = format!("HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n", body.len());
    if let Some(c) = csp {
        head.push_str(&format!("Content-Security-Policy: {c}\r\n"));
    }
    head.push_str("\r\n");
    let _ = s.write_all(head.as_bytes());
    let _ = s.write_all(body);
}

fn serve(mut s: TcpStream, results: &Results) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut words = head.split_whitespace();
    let (method, target) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
    let path = target.split('?').next().unwrap_or("");
    if method == "POST" && path.starts_with("/result/") {
        let len: usize = head
            .lines()
            .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0)))
            .unwrap_or(0);
        while buf.len() < head_end + len {
            match s.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }
        let body = &buf[head_end..(head_end + len).min(buf.len())];
        let v: Value = serde_json::from_slice(body).unwrap_or_else(|e| json!({ "unparsed": e.to_string() }));
        results.lock().unwrap().insert(path["/result/".len()..].to_string(), v);
        respond(&mut s, "200 OK", "application/json", None, b"{}");
        return;
    }
    match page(path) {
        Some((ctype, body, csp)) => respond(&mut s, "200 OK", ctype, Some(csp), body),
        None => respond(&mut s, "404 Not Found", "text/plain", None, b"not found"),
    }
}

fn get<'a>(v: &'a Value, path: &str) -> &'a Value {
    path.split('.').fold(v, |v, k| &v[k])
}

/// The checks: (name, required, passed, detail). `mic`: this Mac has a mic
/// and a person answers macOS's prompt (K2_CHECK_MIC=1); without one,
/// macOS never answers getUserMedia and the hook is never asked.
fn verdict(r: &BTreeMap<String, Value>, mic: bool) -> Vec<(&'static str, bool, bool, String)> {
    let main = r.get("main").cloned().unwrap_or(Value::Null);
    let nowasm = r.get("nowasm").cloned().unwrap_or(Value::Null);
    let other = r.get("other").cloned().unwrap_or(Value::Null);
    let s = |v: &Value| v.to_string();
    let perms = PERMISSIONS.lock().unwrap().clone();
    // Opus sends 50 packets a second; the call runs 10 s with the window hidden.
    let packets = main["rtc"]["rtpPackets"].as_u64().unwrap_or(0);
    let asked = |what: &str| perms.iter().any(|p| p.starts_with("Microphone") && p.ends_with(what));
    vec![
        ("page reported", true, !main.is_null(), if main.is_null() { "no answer from the page".into() } else { "yes".into() }),
        ("secure context on a remote origin", true, main["secureContext"] == true, s(&main["secureContext"])),
        ("navigator.mediaDevices.getUserMedia", true, main["getUserMedia"] == true, s(&main["getUserMedia"])),
        ("RTCPeerConnection", true, main["rtc"]["peerConnection"] == true, s(&main["rtc"]["peerConnection"])),
        ("WebRTC loopback (ICE, DTLS, data channel)", true, main["rtc"]["dataChannel"] == "pong", s(&main["rtc"])),
        ("RTCRtpScriptTransform (E2EE)", true, main["rtc"]["scriptTransform"] == true, s(&main["rtc"]["scriptTransform"])),
        ("AudioWorklet module from the same origin", true, main["worklet"]["frames"].as_u64().unwrap_or(0) > 0, s(&main["worklet"])),
        ("WebAssembly in the AudioWorklet", true, main["worklet"]["wasmInWorklet"] == 42, s(&get(&main, "worklet.wasmInWorklet"))),
        ("WebAssembly.instantiateStreaming under the CSP", true, main["wasm"] == 42, s(&main["wasm"])),
        ("the CSP without 'wasm-unsafe-eval' refuses WebAssembly", true, nowasm["refused"] == true, s(&nowasm)),
        ("IndexedDB write", true, main["idb"]["wrote"] == true, s(&main["idb"])),
        ("the page is hidden", true, main["timers"]["visibility"] == "hidden", s(&main["timers"]["visibility"])),
        (
            "call audio keeps flowing in the hidden window, through the E2EE transform",
            true,
            packets >= 400 && main["rtc"]["transformFrames"]["recv"].as_u64().unwrap_or(0) >= 400,
            format!("{packets} RTP packets in {} ms, transform frames {} (floor 400)", main["rtc"]["callMs"], main["rtc"]["transformFrames"]),
        ),
        ("AudioContext starts without a tap", true, main["audioContext"] == "running", s(&main["audioContext"])),
        ("getUserMedia on the App's origin", mic, main["mic"]["ok"] == true, s(&main["mic"])),
        (
            "a page on another origin is refused the mic",
            mic,
            other["getUserMedia"]["error"].as_str().is_some_and(|e| e.starts_with("NotAllowedError")),
            s(&other),
        ),
        ("k2_voice.rs allowed the App's origin and refused the other", mic, asked(": Allow") && asked(": Deny"), format!("{perms:?}")),
        ("a hidden page's 100 ms timer (info: WebKit slows it)", false, main["timers"]["ticks"].as_u64().unwrap_or(0) >= 25, s(&main["timers"])),
        ("module sent to the AudioWorklet (info)", false, main["worklet"]["moduleInWorklet"] == 3, s(&get(&main, "worklet.moduleInWorklet"))),
    ]
}

fn main() {
    // A fixed port across runs: IndexedDB is kept per origin, port included.
    let port: u16 = std::env::var("K2_CHECK_PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let listener = TcpListener::bind(("127.0.0.1", port)).expect("bind 127.0.0.1");
    let port = listener.local_addr().expect("addr").port();
    let results: Results = Arc::new(Mutex::new(BTreeMap::new()));
    {
        let results = results.clone();
        std::thread::spawn(move || {
            for s in listener.incoming().flatten() {
                let results = results.clone();
                std::thread::spawn(move || serve(s, &results));
            }
        });
    }
    let throttle_default = std::env::var("K2_CHECK_THROTTLE").is_ok_and(|v| v == "default");
    let mic = std::env::var("K2_CHECK_MIC").is_ok_and(|v| v == "1");
    let stamp = std::env::var("K2_CHECK_STAMP").unwrap_or_else(|_| "none".into());
    let wait = Duration::from_secs(std::env::var("K2_CHECK_TIMEOUT").ok().and_then(|v| v.parse().ok()).unwrap_or(150));
    let origin = format!("http://127.0.0.1:{port}");
    let app_origin: &'static str = Box::leak(origin.clone().into_boxed_str());

    tauri::Builder::default()
        // The template's hook, with this run's origin in place of APP_ORIGINS.
        .on_permission_request(move |webview, kind| {
            let url = webview.url().ok();
            let answer = k2_voice::decide(url.as_ref(), kind, &[app_origin]);
            let from = url.map(|u| u.origin().ascii_serialization()).unwrap_or_else(|| "?".into());
            PERMISSIONS.lock().unwrap().push(format!("{kind:?} from {from}: {answer:?}"));
            answer
        })
        .setup(move |app| {
            let mut cfg = app.config().app.windows[0].clone();
            cfg.url = WebviewUrl::External(format!("{origin}/?stamp={stamp}").parse().expect("url"));
            cfg.visible = false;
            if throttle_default {
                cfg.background_throttling = None;
            }
            WebviewWindowBuilder::from_config(app.handle(), &cfg)?.build()?;
            let results = results.clone();
            std::thread::spawn(move || {
                let start = Instant::now();
                while start.elapsed() < wait {
                    if ["main", "nowasm", "other"].iter().all(|k| results.lock().unwrap().contains_key(*k)) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
                let r = results.lock().unwrap().clone();
                let checks = verdict(&r, mic);
                let all = json!({
                    "throttle": if throttle_default { "default" } else { "disabled" },
                    "results": r,
                    "permissions": PERMISSIONS.lock().unwrap().clone(),
                    "checks": checks.iter().map(|(n, req, ok, d)| json!({ "name": n, "required": req, "ok": ok, "detail": d })).collect::<Vec<_>>(),
                });
                println!("K2CHECK {all}");
                let mut failed = 0;
                for (name, required, ok, detail) in &checks {
                    let tag = match (ok, required) {
                        (true, _) => "PASS",
                        (false, true) => "FAIL",
                        (false, false) => "info",
                    };
                    if !ok && *required {
                        failed += 1;
                    }
                    println!("{tag:4}  {name}: {detail}");
                }
                println!("{} required check(s) failed", failed);
                std::process::exit(if failed == 0 { 0 } else { 1 });
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("tauri run");
}
