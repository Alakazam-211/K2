//! A published `--cmd` service started right after its stop (0.46.7): the
//! stop returns only once the service's process tree has exited, so the
//! port is free and nothing answers on it, and the next start never meets
//! "bind clash: … already accepting connections". On Windows the stop ends
//! a Job Object, which only starts the end; the stop now waits for it.
//!
//! Several services at once, several cycles each, so the machine is busy
//! while each stop and start runs.
//!
//! ISOLATION: a temp `$HOME`, the in-memory test DB, plain python HTTP
//! servers on test-range loopback ports, no tunnel.

#[path = "support/app_fixture.rs"]
mod app_fixture;

use app_fixture::*;
use serde_json::json;

const SERVICES: usize = 6;
const CYCLES: usize = 3;

const PY: &str = r#"import http.server, os, sys, socketserver
TAG = open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "tag.txt")).read().strip()
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass
    def do_GET(self):
        body = TAG.encode()
        self.send_response(200); self.send_header("Content-Length", str(len(body))); self.end_headers(); self.wfile.write(body)
class S(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True
S(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
"#;

/// sh on Unix (exec, so the service is the child), cmd.exe on Windows.
fn service_cmd(port: u16) -> String {
    if cfg!(windows) {
        format!("{} server.py {port}", k2_core::test_env::PYTHON)
    } else {
        format!("exec python3 server.py {port}")
    }
}

fn answers(port: u16) -> Option<String> {
    http_get(port, "/").map(|(_, b)| b.trim().to_string())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_service_starts_right_after_its_stop_under_load() {
    require_python3();
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let ws = seed_ws(&env, "stopstart");
    let mut tasks = Vec::new();
    for i in 0..SERVICES {
        let folder = ws.path.join(format!("svc{i}"));
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("server.py"), PY).unwrap();
        let tag = format!("svc{i}");
        std::fs::write(folder.join("tag.txt"), &tag).unwrap();
        let project = ws.id.clone();
        tasks.push(tokio::spawn(async move {
            let svc_port = k2_core::test_env::loopback_test_port();
            let name = tag.clone();
            let r = post(port, "/cli/publish/run", OWNER, json!({"name": name, "project": project, "cmd": service_cmd(svc_port), "cwd": folder, "port": svc_port, "noTunnel": true})).await;
            assert_eq!(r.status, 200, "{name} run: {}", r.body);
            wait_until(&format!("{name} answers"), 30, || answers(svc_port).as_deref() == Some(tag.as_str())).await;
            for cycle in 0..CYCLES {
                let r = post(port, "/cli/publish/stop", OWNER, json!({"name": name, "project": project})).await;
                assert_eq!(r.status, 200, "{name} stop {cycle}: {}", r.body);
                let after_stop = tokio::task::spawn_blocking(move || answers(svc_port)).await.unwrap();
                assert_eq!(after_stop, None, "{name} stop {cycle}: nothing answers once the stop returns");
                let r = post(port, "/cli/publish/start", OWNER, json!({"name": name, "project": project})).await;
                assert_eq!(r.status, 200, "{name} start right after stop {cycle}: {}", r.body);
                wait_until(&format!("{name} answers again ({cycle})"), 30, || answers(svc_port).as_deref() == Some(tag.as_str())).await;
            }
            let r = post(port, "/cli/publish/stop", OWNER, json!({"name": name, "project": project})).await;
            assert_eq!(r.status, 200, "{name} final stop: {}", r.body);
        }));
    }
    for t in tasks {
        t.await.expect("service task");
    }
}
