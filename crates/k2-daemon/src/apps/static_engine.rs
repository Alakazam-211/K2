//! The **static** engine (prd-tauri-core-0467-v1 TC-2, vs-live TA-5/TA-6):
//! `host.mode: "static"` Apps run no App code. "Start" opens a loopback
//! listener on the instance's fresh port that serves the Seed's
//! `host.root` with K2's own file server (`crate::static_files`); "stop"
//! closes it (the supervisor drains its connections first). So the whole
//! pipeline runs unchanged: health gate, switch, drain, the warm previous
//! Seed through the watch window (instant rollback), rollback, labels,
//! custom domains and K2's link-file answers in front.
//!
//! - **No wall, no engine install.** Nothing of the App executes, so it
//!   works on macOS dev boxes and boxes without the guarded engine. The
//!   supervisor picks this engine **by the manifest's mode, before** the
//!   isolation check, so a static deploy is never recorded as a weaker
//!   wall (TA-5): its isolation reads `none (static: no App code)`.
//! - **In the daemon.** Instances don't outlive a daemon restart; the
//!   supervisor serves every live static App again at boot, before the
//!   App engine is waited on (TA-4), and in a hand-off's successor before
//!   it takes the front sockets.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use k2_core::deploy::engine::{
    AppSpec, CgroupStats, EngineCaps, EngineError, ExecStream, Isolation, Liveness, Prepared, Result, RunHandle, RuntimeEngine,
    SeedRef,
};
use k2_core::deploy::manifest::{self, Manifest, MANIFEST_FILE};
use k2_core::deploy::static_site::Site;
use k2_core::deploy::codes;

use crate::static_files::Served;

/// How `k2 app inspect` / `status` name a static App's wall.
pub const ISOLATION_NOTE: &str = "none (static: no App code)";
/// The engine reference prefix of a static instance.
const REF_PREFIX: &str = "static:";

pub struct StaticEngine;

/// The engine (one per process).
pub fn engine() -> &'static StaticEngine {
    static E: StaticEngine = StaticEngine;
    &E
}

/// Is `h` a static instance?
pub fn owns(h: &RunHandle) -> bool {
    h.engine_ref.starts_with(REF_PREFIX)
}

/// Is the stored Seed `hash` a static one (its recorded `host.mode`)?
/// Takes a held connection (callers that already hold the lock).
pub fn seed_is_static(conn: &rusqlite::Connection, hash: &str) -> bool {
    k2_core::deploy::store::get_seed(conn, hash).ok().flatten().is_some_and(|s| s.host_mode == "static")
}

struct Running {
    accept: tokio::task::JoinHandle<()>,
    cut: tokio::sync::watch::Sender<bool>,
}

fn running() -> &'static Mutex<HashMap<String, Running>> {
    static R: OnceLock<Mutex<HashMap<String, Running>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

/// What `prepare` resolved, kept for `start` (by deployment).
fn prepared() -> &'static Mutex<HashMap<String, Served>> {
    static P: OnceLock<Mutex<HashMap<String, Served>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The listeners' own small runtime (never a caller's, which may end).
fn runtime() -> Result<&'static tokio::runtime::Runtime> {
    static RT: OnceLock<Option<tokio::runtime::Runtime>> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("apps-static")
            .enable_all()
            .build()
            .map_err(|e| k2_core::log_debug!("[apps] static runtime: {e}"))
            .ok()
    })
    .as_ref()
    .ok_or_else(|| EngineError::new(codes::ENGINE_UNAVAILABLE, "K2's static file runtime couldn't start"))
}

/// The App's frame policy header lines (its published service row; K2's
/// App default when it has none). A stored value this daemon would never
/// write refuses the start rather than serve the App unguarded.
fn frame_lines(service_id: &str) -> Result<String> {
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::published_services::get_by_id(&conn, service_id).ok().flatten()
    };
    match row {
        None => Ok(k2_core::frame_policy::FramePolicy::app_default().header_lines()),
        Some(r) => k2_core::frame_policy::FramePolicy::from_stored(&r.frame_ancestors)
            .map(|p| p.header_lines())
            .map_err(|e| EngineError::new(codes::SEED_INVALID, format!("the App's frame policy: {e} (k2 publish frame {} --reset)", r.name))),
    }
}

/// Build what a static Seed serves (the manifest read from the Seed), with
/// the App's frame policy.
pub fn served_for(seed: &SeedRef, service_id: &str) -> Result<Served> {
    served_with(seed, &frame_lines(service_id)?)
}

/// [`served_for`] with the frame header lines given.
pub fn served_with(seed: &SeedRef, frame_lines: &str) -> Result<Served> {
    let text = std::fs::read_to_string(seed.path.join(MANIFEST_FILE))
        .map_err(|e| EngineError::new(codes::SEED_CORRUPT, format!("the Seed's {MANIFEST_FILE}: {e}")))?;
    let m: Manifest = manifest::parse(&text).map_err(|e| EngineError::new(codes::SEED_INVALID, format!("{MANIFEST_FILE}: {}", e.message)))?;
    if !m.is_static() {
        return Err(EngineError::new(codes::SEED_INVALID, "the static engine serves host.mode \"static\" Seeds only"));
    }
    let site = Site::from_manifest(&m, &seed.path);
    let root = seed.path.join(&site.root);
    if !root.is_dir() {
        return Err(EngineError::new(codes::SEED_INVALID, format!("host.root \"{}\" isn't a folder in this Seed", site.root.display())));
    }
    let served = Served::new(&root, site, &seed.hash, frame_lines)
        .map_err(|e| EngineError::new(codes::SEED_CORRUPT, format!("host.root: {e}")))?;
    // The jail: the root itself never leaves the Seed (a link out).
    let seed_canon = dunce::canonicalize(&seed.path).map_err(|e| EngineError::new(codes::SEED_CORRUPT, e.to_string()))?;
    if !served.root.starts_with(&seed_canon) {
        return Err(EngineError::new(codes::SEED_INVALID, "host.root leaves the Seed"));
    }
    Ok(served)
}

/// Open the loopback listener on `port` serving `served`; the handle.
pub fn serve(app_id: &str, deployment_id: &str, port: u16, served: Served) -> Result<RunHandle> {
    let rt = runtime()?;
    let std_l = std::net::TcpListener::bind(("127.0.0.1", port))
        .map_err(|e| EngineError::new(codes::ENGINE_UNAVAILABLE, format!("bind 127.0.0.1:{port}: {e}")))?;
    std_l.set_nonblocking(true).map_err(|e| EngineError::new(codes::ENGINE_UNAVAILABLE, e.to_string()))?;
    let _g = rt.enter();
    let l = tokio::net::TcpListener::from_std(std_l).map_err(|e| EngineError::new(codes::ENGINE_UNAVAILABLE, e.to_string()))?;
    let (cut, cut_rx) = tokio::sync::watch::channel(false);
    let served = std::sync::Arc::new(served);
    let accept = rt.spawn(async move {
        let mut stop = cut_rx.clone();
        loop {
            tokio::select! {
                a = l.accept() => {
                    let Ok((mut s, _)) = a else { continue };
                    let served = served.clone();
                    let rx = cut_rx.clone();
                    tokio::spawn(async move {
                        crate::static_files::serve_conn(&mut s, &served, rx).await;
                    });
                }
                _ = stop.changed() => break,
            }
        }
    });
    running().lock().unwrap_or_else(|e| e.into_inner()).insert(deployment_id.to_string(), Running { accept, cut });
    Ok(RunHandle {
        app_id: app_id.to_string(),
        deployment_id: deployment_id.to_string(),
        port,
        pid: None,
        engine_ref: format!("{REF_PREFIX}{deployment_id}"),
    })
}

impl RuntimeEngine for StaticEngine {
    fn isolation(&self) -> Isolation {
        Isolation::None
    }

    fn caps(&self) -> EngineCaps {
        // Nothing to enforce: no App code runs.
        EngineCaps::default()
    }

    fn prepare(&self, seed: &SeedRef, spec: &AppSpec) -> Result<Prepared> {
        let served = served_for(seed, &spec.service_id)?;
        prepared().lock().unwrap_or_else(|e| e.into_inner()).insert(spec.deployment_id.clone(), served);
        Ok(Prepared { workdir: seed.path.clone() })
    }

    fn start(&self, spec: &AppSpec, _p: &Prepared, port: u16) -> Result<RunHandle> {
        let served = prepared()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&spec.deployment_id)
            .ok_or_else(|| EngineError::new(codes::ENGINE_UNAVAILABLE, "start before prepare"))?;
        serve(&spec.app_id, &spec.deployment_id, port, served)
    }

    fn liveness(&self, h: &RunHandle) -> Liveness {
        match running().lock().unwrap_or_else(|e| e.into_inner()).get(&h.deployment_id) {
            Some(r) if !r.accept.is_finished() => Liveness::Running,
            _ => Liveness::Exited(None),
        }
    }

    fn stop(&self, h: &RunHandle, _grace: Duration) -> Result<()> {
        if let Some(r) = running().lock().unwrap_or_else(|e| e.into_inner()).remove(&h.deployment_id) {
            let _ = r.cut.send(true);
            r.accept.abort();
        }
        prepared().lock().unwrap_or_else(|e| e.into_inner()).remove(&h.deployment_id);
        Ok(())
    }

    fn exec(&self, h: &RunHandle, _argv: &[String], _tty: bool) -> Result<ExecStream> {
        Err(EngineError::new(
            codes::BAD_REQUEST,
            format!("{} is a static App: K2 serves its files and runs no App code, so there is nothing to exec into", h.app_id),
        ))
    }

    fn stats(&self, _h: &RunHandle) -> Result<CgroupStats> {
        Ok(CgroupStats::default())
    }

    fn adopt(&self) -> Result<Vec<RunHandle>> {
        // In-process: nothing outlives the daemon (the boot serves again).
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn a_static_instance_serves_and_stops() {
        let dir = k2_core::test_env::unique_temp_path("static-engine");
        std::fs::create_dir_all(dir.join("dist")).unwrap();
        std::fs::write(
            dir.join(MANIFEST_FILE),
            r#"{"schema":1,"kind":"app","id":"docs","name":"Docs","version":"1","host":{"mode":"static","root":"dist"}}"#,
        )
        .unwrap();
        std::fs::write(dir.join("dist/index.html"), "hello static").unwrap();
        let seed = SeedRef { hash: "0123456789abcdef".into(), path: dir.clone() };
        let frame = k2_core::frame_policy::FramePolicy::app_default().header_lines();
        let served = served_with(&seed, &frame).expect("served");
        assert!(served.site.spa, "index.html under root: spa by default");
        assert!(served.extra_lines.contains("frame-ancestors"), "the App default frame policy: {}", served.extra_lines);
        let port = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port();
        let h = serve("docs", "dep-static-1", port, served).expect("serve");
        assert!(owns(&h));
        assert_eq!(engine().liveness(&h), Liveness::Running);
        let mut c = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        c.write_all(b"GET /anything HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").unwrap();
        let mut out = String::new();
        c.read_to_string(&mut out).unwrap();
        assert!(out.starts_with("HTTP/1.1 200 OK"), "{out}");
        assert!(out.ends_with("hello static"), "{out}");
        engine().stop(&h, Duration::from_secs(1)).unwrap();
        assert_eq!(engine().liveness(&h), Liveness::Exited(None));
        assert!(engine().exec(&h, &[], false).is_err(), "nothing to exec into");
        assert_eq!(engine().isolation(), Isolation::None);
        // A cmd Seed is never served by it.
        std::fs::write(dir.join(MANIFEST_FILE), r#"{"schema":1,"kind":"app","id":"docs","name":"Docs","version":"1","host":{"mode":"cmd","start":["x"]}}"#).unwrap();
        assert_eq!(served_with(&seed, &frame).unwrap_err().code, codes::SEED_INVALID);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
