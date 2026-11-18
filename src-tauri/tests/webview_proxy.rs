//! The Linux web view never sends a loopback request through a proxy.
//!
//! Loopback (127.0.0.1, ::1, localhost) and plain http never use a proxy;
//! HTTPS to a remote K2 may use the person's proxy (a CONNECT tunnel hides
//! the request; a company proxy that intercepts TLS is the organisation's
//! own policy).
//!
//! A real WebKitGTK page under Xvfb, one child process per case and page
//! origin (http://localhost, file://, tauri://localhost). The page fetches
//! 127.0.0.1, 127.0.0.2 and localhost, opens a WebSocket to 127.0.0.1, and
//! fetches a remote host (k2-remote.example.test) that only the proxy stub
//! answers. A counting proxy stub and a stub server record what reached
//! them.
//!
//!   env      every proxy variable set to the stub, no_proxy unset, the
//!            session set by linux_webview_proxy::apply_once: the server
//!            gets every loopback request, the proxy only the remote one.
//!   gnome    no proxy variable; GNOME's proxy (manual, its ignore list
//!            emptied) in the test's own keyfile settings: the same.
//!   none     no proxy at all: nothing is set, nothing reaches the stub.
//!   bite     the env case with the loopback hosts left out of the ignore
//!            list: the loopback requests go to the proxy stub.
//!
//! Linux only, with Xvfb on PATH (or a DISPLAY). Settings never touch the
//! machine's: the GNOME case uses a keyfile in a temp XDG_CONFIG_HOME, the
//! others an in-memory settings backend.
//! Run with: cargo test -p k2 --test webview_proxy

#[cfg(not(target_os = "linux"))]
fn main() {
    println!("webview_proxy: Linux web view only (WebKitGTK); nothing to run on this OS");
}

#[cfg(target_os = "linux")]
fn main() {
    linux::main();
}

#[cfg(target_os = "linux")]
mod linux {
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};
    use std::process::{Command, Stdio};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    const CHILD: &str = "WEBVIEW_PROXY_TEST_CHILD";
    const REMOTE: &str = "k2-remote.example.test";
    const PROXY_VARS: [&str; 6] = ["http_proxy", "HTTP_PROXY", "https_proxy", "HTTPS_PROXY", "all_proxy", "ALL_PROXY"];
    const ORIGINS: [&str; 3] = ["http", "file", "tauri"];

    type Log = Arc<Mutex<Vec<String>>>;

    pub fn main() {
        if let Ok(spec) = std::env::var(CHILD) {
            return child(&spec);
        }
        let _xvfb = display();
        let proxy_log: Log = Default::default();
        let server_log: Log = Default::default();
        let proxy_port = proxy_stub(proxy_log.clone());
        let server_port = server_stub(server_log.clone());
        let proxy = format!("http://127.0.0.1:{proxy_port}");
        let keyfile_home = tempfile::tempdir().expect("settings dir");
        write_gnome_keyfile(keyfile_home.path(), proxy_port);

        let mut problems = Vec::new();
        for case in ["env", "gnome", "none", "bite"] {
            for origin in ORIGINS {
                proxy_log.lock().expect("log").clear();
                server_log.lock().expect("log").clear();
                let mut cmd = Command::new(std::env::current_exe().expect("test binary"));
                cmd.env(CHILD, format!("{case} {origin} {server_port}"))
                    .env_remove("no_proxy")
                    .env_remove("NO_PROXY")
                    .env("NO_AT_BRIDGE", "1")
                    .env("LIBGL_ALWAYS_SOFTWARE", "1")
                    .env("WEBKIT_DISABLE_COMPOSITING_MODE", "1")
                    .stdin(Stdio::null());
                for var in PROXY_VARS {
                    cmd.env_remove(var);
                }
                match case {
                    "env" | "bite" => {
                        for var in PROXY_VARS {
                            cmd.env(var, &proxy);
                        }
                        cmd.env("GSETTINGS_BACKEND", "memory");
                    }
                    "gnome" => {
                        cmd.env("GSETTINGS_BACKEND", "keyfile")
                            .env("XDG_CONFIG_HOME", keyfile_home.path())
                            .env("XDG_CURRENT_DESKTOP", "GNOME")
                            .env("GIO_USE_PROXY_RESOLVER", "gnome");
                    }
                    _ => {
                        cmd.env("GSETTINGS_BACKEND", "memory");
                    }
                }
                let out = cmd.output().expect("the child ran");
                std::thread::sleep(Duration::from_millis(300));
                let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                let result = stdout.lines().find(|l| l.starts_with("RESULT ")).unwrap_or("no RESULT line").to_string();
                let p = proxy_log.lock().expect("log").clone();
                let s = server_log.lock().expect("log").clone();
                let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                let resolver = stderr.lines().find(|l| l.contains(": resolver ")).unwrap_or("").to_string();
                println!("{case:5} {origin:5}: {result}\n        {resolver}\n        proxy {p:?}\n        server {s:?}");
                if !out.status.success() {
                    problems.push(format!(
                        "{case} {origin}: the child failed ({}): {}",
                        out.status,
                        String::from_utf8_lossy(&out.stderr).lines().rev().take(8).collect::<Vec<_>>().join(" | ")
                    ));
                    continue;
                }
                let loopback_at_server =
                    ["GET /x1", "GET /x2", "GET /x3", "WS /ws"].iter().all(|w| s.iter().any(|l| l.starts_with(w)));
                let loopback_at_proxy = p.iter().any(|l| l.contains("127.0.0.") || l.contains("localhost"));
                let remote_at_proxy = p.iter().any(|l| l.starts_with(&format!("GET http://{REMOTE}/remote ")));
                match case {
                    "env" | "gnome" => {
                        if !loopback_at_server || loopback_at_proxy {
                            problems.push(format!("{case} {origin}: loopback must reach the server and never the proxy (proxy {p:?}, server {s:?})"));
                        }
                        if !remote_at_proxy || p.len() != 1 {
                            problems.push(format!("{case} {origin}: the remote host must go to the person's proxy, alone (proxy {p:?})"));
                        }
                    }
                    "none" => {
                        if !loopback_at_server || !p.is_empty() {
                            problems.push(format!("{case} {origin}: with no proxy nothing reaches the stub (proxy {p:?}, server {s:?})"));
                        }
                    }
                    _ => {
                        if !loopback_at_proxy {
                            problems.push(format!("{case} {origin}: without the loopback hosts the proxy must get the loopback requests (proxy {p:?})"));
                        }
                    }
                }
            }
        }
        if !problems.is_empty() {
            for p in &problems {
                eprintln!("FAIL: {p}");
            }
            panic!("webview_proxy: {} problem(s)", problems.len());
        }
        println!("webview_proxy: every case and origin as expected");
    }

    /// Xvfb on a free display, unless the run already has a DISPLAY.
    fn display() -> Option<std::process::Child> {
        if std::env::var_os("DISPLAY").is_some() {
            return None;
        }
        let n = 90 + std::process::id() % 100;
        let child = Command::new("Xvfb")
            .args([format!(":{n}"), "-screen".into(), "0".into(), "1024x768x24".into()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("Xvfb on PATH (this test needs it on Linux)");
        std::env::set_var("DISPLAY", format!(":{n}"));
        std::thread::sleep(Duration::from_secs(2));
        Some(child)
    }

    /// GNOME's proxy in the keyfile backend: manual, http and https to the
    /// stub, its own ignore list emptied (the hosts K2 adds are the test).
    fn write_gnome_keyfile(home: &std::path::Path, proxy_port: u16) {
        let dir = home.join("glib-2.0/settings");
        std::fs::create_dir_all(&dir).expect("keyfile dir");
        std::fs::write(
            dir.join("keyfile"),
            format!(
                "[system/proxy]\nmode='manual'\nignore-hosts=@as []\n\n\
                 [system/proxy/http]\nhost='127.0.0.1'\nport={proxy_port}\n\n\
                 [system/proxy/https]\nhost='127.0.0.1'\nport={proxy_port}\n"
            ),
        )
        .expect("keyfile");
    }

    /// The person's proxy: logs each request line; answers the remote host
    /// with 200, anything else with 502.
    fn proxy_stub(log: Log) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind proxy stub");
        let port = listener.local_addr().expect("addr").port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let log = log.clone();
                std::thread::spawn(move || {
                    let line = read_head(&stream).0;
                    log.lock().expect("log").push(line.clone());
                    let reply: &[u8] = if line.starts_with(&format!("GET http://{REMOTE}/")) {
                        b"HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: 6\r\nConnection: close\r\n\r\nremote"
                    } else {
                        b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    };
                    let mut s = stream;
                    let _ = s.write_all(reply);
                });
            }
        });
        port
    }

    /// The stub server, on 127.0.0.1 and 127.0.0.2 at one port: logs
    /// `GET <path>` for a request and `WS <path>` for a WebSocket upgrade.
    fn server_stub(log: Log) -> u16 {
        let first = TcpListener::bind("127.0.0.1:0").expect("bind server stub");
        let port = first.local_addr().expect("addr").port();
        let second = TcpListener::bind(("127.0.0.2", port)).expect("bind server stub on 127.0.0.2");
        for listener in [first, second] {
            let log = log.clone();
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    let log = log.clone();
                    std::thread::spawn(move || serve(stream, log));
                }
            });
        }
        port
    }

    fn serve(stream: TcpStream, log: Log) {
        let mut peek = [0u8; 2048];
        let n = stream.peek(&mut peek).unwrap_or(0);
        let head = String::from_utf8_lossy(&peek[..n]).to_ascii_lowercase();
        if head.contains("upgrade: websocket") {
            let path = head.split_whitespace().nth(1).unwrap_or("?").to_string();
            match tungstenite::accept(stream) {
                Ok(mut ws) => {
                    log.lock().expect("log").push(format!("WS {path}"));
                    let _ = ws.close(None);
                    let _ = ws.flush();
                }
                Err(e) => log.lock().expect("log").push(format!("WS-FAILED {path}: {e}")),
            }
            return;
        }
        let (line, _) = read_head(&stream);
        let path = line.split_whitespace().nth(1).unwrap_or("?").to_string();
        log.lock().expect("log").push(format!("GET {path}"));
        let mut s = stream;
        let _ = s.write_all(b"HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
    }

    /// The request line and the rest of the head.
    fn read_head(stream: &TcpStream) -> (String, String) {
        // A stub: a read that fails ends the head, and the request line
        // (empty then) is what gets logged.
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let mut reader = BufReader::new(stream.try_clone().expect("clone"));
        let mut line = String::new();
        let _ = reader.read_line(&mut line);
        let mut rest = String::new();
        loop {
            let mut h = String::new();
            match reader.read_line(&mut h) {
                Ok(0) | Err(_) => break,
                Ok(_) if h == "\r\n" => break,
                Ok(_) => rest.push_str(&h),
            }
        }
        (line.trim_end().to_string(), rest)
    }

    /// One case and origin, on GTK's main thread.
    fn child(spec: &str) {
        use gtk::prelude::*;
        use k2_lib::linux_webview_proxy as wp;
        use webkit2gtk::{URISchemeRequestExt, WebContext, WebContextExt, WebView, WebViewExt};

        let mut parts = spec.split_whitespace();
        let (case, origin, port) = (parts.next().expect("case"), parts.next().expect("origin"), parts.next().expect("port"));
        gtk::init().expect("gtk init");
        let ctx = WebContext::default().expect("web context");
        let view = WebView::with_context(&ctx);
        match case {
            "env" | "gnome" => {
                use webkit2gtk::gio::prelude::*;
                let resolver = webkit2gtk::gio::ProxyResolver::default();
                let mode = webkit2gtk::gio::Settings::new("org.gnome.system.proxy").string("mode");
                eprintln!(
                    "{case}: resolver {} mode {mode} lookup {:?}",
                    resolver.type_().name(),
                    resolver.lookup("http://k2-proxy-probe.invalid/", None::<&webkit2gtk::gio::Cancellable>)
                );
                assert!(wp::system_plan_with(&wp::LOOPBACK_IGNORE).is_some(), "{case}: the person's proxy must give a plan");
                wp::apply_once(&view);
            }
            "none" => {
                assert_eq!(wp::system_plan_with(&wp::LOOPBACK_IGNORE), None, "no proxy: nothing to set");
                wp::apply_once(&view);
            }
            "bite" => {
                let plan = wp::system_plan_with(&[]).expect("the person's proxy");
                wp::apply_plan(&view, &plan).expect("apply");
            }
            other => panic!("unknown case {other}"),
        }
        let html = format!(
            r#"<!doctype html><html><body><script>
const get = async (u) => {{ try {{ const r = await fetch(u, {{mode: "no-cors", cache: "no-store"}}); return r.type; }} catch (e) {{ return "error"; }} }};
(async () => {{
  const a = await get("http://127.0.0.1:{port}/x1");
  const b = await get("http://127.0.0.2:{port}/x2");
  const c = await get("http://localhost:{port}/x3");
  const r = await get("http://{REMOTE}/remote");
  const w = await new Promise((res) => {{
    try {{
      const s = new WebSocket("ws://127.0.0.1:{port}/ws");
      s.onopen = () => {{ s.close(); res("open"); }};
      s.onerror = () => res("error");
      setTimeout(() => res("timeout"), 8000);
    }} catch (e) {{ res("throw"); }}
  }});
  document.title = "done origin=" + location.origin + " x1=" + a + " x2=" + b + " x3=" + c + " remote=" + r + " ws=" + w;
}})();
</script></body></html>"#
        );
        if origin == "tauri" {
            let page = html.clone();
            ctx.register_uri_scheme("tauri", move |req| {
                let bytes = webkit2gtk::glib::Bytes::from_owned(page.clone().into_bytes());
                let len = bytes.len() as i64;
                let stream = webkit2gtk::gio::MemoryInputStream::from_bytes(&bytes);
                req.finish(&stream, len, Some("text/html"));
            });
        }
        let win = gtk::Window::new(gtk::WindowType::Toplevel);
        win.add(&view);
        win.show_all();
        view.connect_title_notify(|v| {
            if let Some(t) = v.title() {
                if t.starts_with("done") {
                    println!("RESULT {t}");
                    gtk::main_quit();
                }
            }
        });
        match origin {
            "http" => view.load_html(&html, Some(&format!("http://localhost:{port}/"))),
            "file" => view.load_html(&html, Some("file:///tmp/")),
            "tauri" => view.load_uri("tauri://localhost/"),
            other => panic!("unknown origin {other}"),
        }
        webkit2gtk::glib::timeout_add_seconds_local(45, || {
            println!("RESULT timeout");
            gtk::main_quit();
            webkit2gtk::glib::ControlFlow::Break
        });
        gtk::main();
    }
}
