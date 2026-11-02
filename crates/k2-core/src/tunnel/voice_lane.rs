//! The voice lane, `frpc-voice` (voice rooms PRD v1.9 §6.2, §6.6 M2/M4,
//! slice S9b; 0.46.8, behind the `voice_tunnel_preview` switch).
//!
//! A second frpc, supervised by the daemon, that carries ONLY sew-media's
//! TURN-TLS: one `https` proxy named `k2voice-<label>` for
//! `turn.<label>.k2.dev` → `127.0.0.1:5349` with a PROXY v2 header
//! ([`super::render::render_voice_frpc_toml`]). Voice then never shares a
//! TCP connection with the box's other tunnel traffic (a bulk download
//! can't head-of-line block a call). The main frpc carries no turn proxy.
//!
//! - **Login:** the main frpc's token and device id, plus
//!   `metadatas.role = "voice"`, so the relay never takes this login or its
//!   close as the box's liveness. frp's `tcpMux` stays at its default (on):
//!   frps requires it to match; isolation comes from the separate process.
//! - **Relay:** it follows the main frpc: the relay the main supervisor dials
//!   NOW ([`super::connector::voice_follow`], the RelaySelector's pick). When
//!   the main frpc fails over, fails back or moves with the list, this lane
//!   is re-rendered and restarted on the new relay at once. It never picks a
//!   relay itself (relay-to-relay failover for `turn.<label>` is phase 2).
//! - **Restarts:** at once on exit; backoff only after repeated failures
//!   ([`RestartPolicy`]: the third exit inside 60 s waits 2 s, doubling to
//!   at most 30 s). A normal blip is about 1–2 s.
//! - **Daemon restarts:** frpc-voice writes into `~/.k2/frpc-voice.log` (no
//!   pipe) in its own session and records `frpc-voice.pid`, like the main
//!   frpc (runtime stage H); the next daemon ADOPTS it when its pid,
//!   identity, argv and rendered config still match ([`frpc_proc::try_adopt`]),
//!   so a daemon update doesn't end calls. A hand-off leaves it running.
//! - **Files:** `<runtime dir>/frpc-voice.toml` (0600: it holds the token)
//!   and `frpc-voice.pid` beside it. The name must never START with the
//!   main config's path: the main stop path reaps `pkill -f "frpc -c <cfg>"`.
//! - **When it runs:** armed ([`arm`], the daemon does it when the box's
//!   voice runs the `tunnel` TURN tier) AND the preview switch is on AND the
//!   main tunnel runs, end-to-end encrypted (the relay passes `turn.<label>`
//!   only for E2E labels), with a label. The main `stop()` stops it too.
//!   With the preview off nothing here ever starts.
//! - **Up / down:** up = frps accepted THIS run's `k2voice-<label>` proxy
//!   (`start proxy success` in its log); an exit, a login failure, a proxy
//!   refusal or a lost session clears it. [`set_listener`] hears every
//!   change at once (the daemon's media-path state machine, §6.6 M11).

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;

use super::config::RelayEndpoint;
use super::connector::{self, FrpcHealth, ProxyRegistration};
use super::frpc_proc::{self, FrpcProc};
use super::render::{render_voice_frpc_toml, voice_proxy_name, voice_turn_host};

/// The voice lane's rendered config, beside the main `frpc.toml`.
pub const VOICE_CONFIG_NAME: &str = "frpc-voice.toml";
/// The env var that forces the preview on (`1`/`true`/`yes`/`on`).
pub const PREVIEW_ENV: &str = "K2_VOICE_TUNNEL";
/// The supervisor's poll (it is also woken at once by [`kick`]).
const POLL: Duration = Duration::from_millis(250);

/// `<runtime dir>/frpc-voice.toml` (the main config's private runtime dir;
/// `~/.k2/frpc-voice.toml` when there is none, and in tests).
pub fn voice_config_path() -> PathBuf {
    #[cfg(not(test))]
    {
        if let Some(d) = crate::runtime_dir::private_runtime_dir() {
            return d.join(VOICE_CONFIG_NAME);
        }
    }
    k2_dir().join(VOICE_CONFIG_NAME)
}

/// Where frpc-voice writes its output: `~/.k2/frpc-voice.log`.
pub fn voice_log_path() -> PathBuf {
    k2_dir().join("frpc-voice.log")
}

fn k2_dir() -> PathBuf {
    crate::paths::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".k2")
}

// ── the preview switch ───────────────────────────────────────────────────

/// The voice tunnel preview is on (`voice_tunnel_preview` or
/// `K2_VOICE_TUNNEL`, [`crate::voice_rooms::tunnel_enabled`]). Default OFF:
/// then no box runs frpc-voice and nothing renders a turn proxy.
pub fn preview_enabled() -> bool {
    crate::voice_rooms::tunnel_enabled()
}

// ── restarts ─────────────────────────────────────────────────────────────

/// When to restart after an exit (§6.6 M4): at once, unless frpc-voice
/// keeps dying. The third exit inside [`Self::WINDOW`] waits 2 s, each
/// further one doubles, at most 30 s; exits older than the window are
/// forgotten.
#[derive(Debug, Default)]
pub(crate) struct RestartPolicy {
    exits: VecDeque<Instant>,
}

impl RestartPolicy {
    pub(crate) const WINDOW: Duration = Duration::from_secs(60);
    const FREE: usize = 2;
    const FIRST: Duration = Duration::from_secs(2);
    const MAX: Duration = Duration::from_secs(30);

    /// One exit (or failed spawn) at `now`: how long to wait before the
    /// next spawn.
    pub(crate) fn on_exit(&mut self, now: Instant) -> Duration {
        while self.exits.front().is_some_and(|t| now.duration_since(*t) > Self::WINDOW) {
            self.exits.pop_front();
        }
        self.exits.push_back(now);
        let n = self.exits.len();
        if n <= Self::FREE {
            return Duration::ZERO;
        }
        let shift = (n - Self::FREE - 1).min(8) as u32;
        (Self::FIRST * 2u32.pow(shift)).min(Self::MAX)
    }
}

// ── state ────────────────────────────────────────────────────────────────

/// One frpc-voice process this daemon supervises.
struct Run {
    proc: FrpcProc,
    pid: i32,
    relay: RelayEndpoint,
    rendered: String,
    tail_stop: Arc<AtomicBool>,
    run_id: u64,
    adopted: bool,
    came_up: bool,
}

#[derive(Default)]
struct Lane {
    run: Option<Run>,
    /// The proxy tracker for the current label (`k2voice-<label>`).
    proxy: Option<(String, Arc<ProxyRegistration>)>,
    health: Option<Arc<FrpcHealth>>,
    policy: RestartPolicy,
    /// No spawn before this (backoff after repeated exits).
    next_spawn: Option<Instant>,
    /// The first spawn of this supervisor tries to adopt a running frpc-voice
    /// (a previous daemon's) before it reaps and spawns.
    adopt_checked: bool,
}

impl Lane {
    fn proxy_for(&mut self, label: &str) -> Arc<ProxyRegistration> {
        let name = voice_proxy_name(label);
        match &self.proxy {
            Some((n, p)) if *n == name => p.clone(),
            _ => {
                let p = Arc::new(ProxyRegistration::named(name.clone()));
                self.proxy = Some((name, p.clone()));
                p
            }
        }
    }

    fn health(&mut self) -> Arc<FrpcHealth> {
        self.health.get_or_insert_with(|| Arc::new(FrpcHealth::new())).clone()
    }

    fn registered(&self) -> bool {
        self.run.is_some() && self.proxy.as_ref().is_some_and(|(_, p)| p.is_registered())
    }

    /// Stop supervising the run: kill it, or (hand-off) leave it running.
    fn end_run(&mut self, kill: bool) {
        if let Some(mut r) = self.run.take() {
            r.tail_stop.store(true, Ordering::SeqCst);
            if kill {
                r.proc.kill_and_wait();
            } else {
                std::mem::forget(r.proc);
            }
        }
        if let Some((_, p)) = &self.proxy {
            p.clear();
        }
    }
}

static LANE: OnceLock<Mutex<Lane>> = OnceLock::new();
static ARMED: AtomicBool = AtomicBool::new(false);
static SUPERVISING: AtomicBool = AtomicBool::new(false);
static WAKE: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

fn lane() -> std::sync::MutexGuard<'static, Lane> {
    LANE.get_or_init(|| Mutex::new(Lane::default())).lock().unwrap_or_else(|p| p.into_inner())
}

/// The last up/down the lane reported, and when it changed (unix secs).
#[derive(Debug, Clone, Copy, Default)]
struct UpState {
    up: bool,
    since: i64,
}

static UP: Mutex<UpState> = Mutex::new(UpState { up: false, since: 0 });
static LISTENER: Mutex<Option<fn(bool)>> = Mutex::new(None);

/// Hear every up/down change of the lane at once (`true` = frps accepted
/// `k2voice-<label>` for the running frpc-voice). One listener: the daemon's
/// media-path state machine (§6.6 M11). Called on the thread that saw it.
pub fn set_listener(f: fn(bool)) {
    *LISTENER.lock().unwrap_or_else(|p| p.into_inner()) = Some(f);
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Publish `up` and tell the listener when it changed.
fn publish_up(up: bool) {
    let changed = {
        let mut st = UP.lock().unwrap_or_else(|p| p.into_inner());
        if st.up == up {
            false
        } else {
            *st = UpState { up, since: now_secs() };
            true
        }
    };
    if changed {
        crate::log_debug!("[tunnel] voice lane {}", if up { "up (frps accepted k2voice proxy)" } else { "down" });
        let l = *LISTENER.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(f) = l {
            f(up);
        }
    }
}

/// Whether the voice lane is up now (frps holds this run's turn proxy).
pub fn is_up() -> bool {
    UP.lock().unwrap_or_else(|p| p.into_inner()).up
}

// ── arm / disarm ─────────────────────────────────────────────────────────

/// Run the voice lane whenever the tunnel runs (the box's voice is on the
/// `tunnel` TURN tier). Refused while the preview is off. Idempotent.
pub fn arm() -> Result<(), String> {
    if !preview_enabled() {
        return Err(format!("the voice tunnel preview is off (Settings, or {PREVIEW_ENV}=1)"));
    }
    if !ARMED.swap(true, Ordering::SeqCst) {
        crate::log_debug!("[tunnel] voice lane armed");
    }
    ensure_supervisor();
    kick();
    Ok(())
}

/// Stop the voice lane for good (tier moved to direct, voice removed, the
/// preview turned off): kill frpc-voice, reap any stray one, drop its pid
/// record and config (it holds the token). Idempotent.
pub fn disarm(reason: &str) {
    let was = ARMED.swap(false, Ordering::SeqCst);
    {
        let mut l = lane();
        l.end_run(true);
        l.next_spawn = None;
        l.adopt_checked = false;
    }
    let cfg = voice_config_path();
    connector::reap_stray_frpc(&cfg);
    frpc_proc::clear_pid_record(&cfg);
    let _ = std::fs::remove_file(&cfg);
    if was {
        crate::log_debug!("[tunnel] voice lane disarmed ({reason})");
    }
    publish_up(false);
    kick();
}

/// Whether the daemon armed the lane (it may still wait for the tunnel).
pub fn armed() -> bool {
    ARMED.load(Ordering::SeqCst)
}

/// The main tunnel stopped (`connector::stop`): frpc-voice stops with it.
/// An armed lane comes back with the next tunnel start.
pub(crate) fn on_main_stop(reason: &str) {
    let had = {
        let mut l = lane();
        let had = l.run.is_some();
        l.end_run(true);
        l.next_spawn = None;
        had
    };
    let cfg = voice_config_path();
    connector::reap_stray_frpc(&cfg);
    frpc_proc::clear_pid_record(&cfg);
    if had {
        crate::log_debug!("[tunnel] voice lane stopped with the tunnel ({reason})");
    }
    publish_up(false);
}

/// Wake the supervisor now (a tunnel start, a relay move, a setting).
pub fn kick() {
    let (m, c) = &WAKE;
    *m.lock().unwrap_or_else(|p| p.into_inner()) = true;
    c.notify_all();
}

fn ensure_supervisor() {
    if SUPERVISING.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        return;
    }
    let spawned = std::thread::Builder::new().name("k2-frpc-voice".into()).spawn(|| loop {
        if !tick() {
            SUPERVISING.store(false, Ordering::SeqCst);
            // An arm() that ran after this tick's check saw a live
            // supervisor and started none: keep going for it.
            if ARMED.load(Ordering::SeqCst)
                && !connector::detached()
                && SUPERVISING.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok()
            {
                continue;
            }
            return;
        }
        let (m, c) = &WAKE;
        let mut woke = m.lock().unwrap_or_else(|p| p.into_inner());
        if !*woke {
            woke = c.wait_timeout(woke, POLL).unwrap_or_else(|p| p.into_inner()).0;
        }
        *woke = false;
    });
    if let Err(e) = spawned {
        SUPERVISING.store(false, Ordering::SeqCst);
        crate::log_debug!("[tunnel] WARN: voice lane supervisor thread: {e}");
    }
}

/// What the lane should be running now.
struct Want {
    label: String,
    relay: RelayEndpoint,
    rendered: String,
    frpc: PathBuf,
}

fn want() -> Option<Want> {
    if !ARMED.load(Ordering::SeqCst) || !preview_enabled() {
        return None;
    }
    let (mut cfg, relay, e2e, frpc) = connector::voice_follow()?;
    if !e2e {
        return None;
    }
    // Always the confirmed label, however the main tunnel is named: its own
    // config has one only under K2_TUNNEL_PROXY_NAME=confirmed.
    let label = super::confirmed_label::voice_label_or_ask(&cfg)?;
    cfg.confirmed_label = Some(label.clone());
    let rendered = render_voice_frpc_toml(&cfg, &relay)?;
    Some(Want { label, relay, rendered, frpc })
}

/// One supervisor pass. `false` = nothing left to supervise (exit).
fn tick() -> bool {
    if connector::detached() {
        // Runtime stage H: the successor adopts frpc-voice; never kill it.
        let mut l = lane();
        if let Some(r) = l.run.as_ref() {
            crate::log_debug!("[tunnel] hand-off: leaving frpc-voice pid {} running for the successor", r.pid);
        }
        l.end_run(false);
        return false;
    }
    let want = want();
    let now = Instant::now();
    let up;
    {
        let mut l = lane();
        // 1) The running frpc-voice exited: restart at once (or after the
        //    backoff when it keeps dying).
        let exited = l.run.as_mut().and_then(|r| match r.proc.try_wait() {
            Ok(Some(code)) => Some((code, r.came_up)),
            Ok(None) => None,
            Err(e) => {
                crate::log_debug!("[tunnel] WARN: frpc-voice wait: {e}");
                Some((None, r.came_up))
            }
        });
        if let Some((code, came_up)) = exited {
            l.end_run(false);
            let health = l.health();
            if !came_up {
                health.record_failure(format!("frpc-voice exited ({code:?}) without registering its proxy"));
            }
            let delay = l.policy.on_exit(now);
            l.next_spawn = Some(now + delay);
            crate::log_debug!("[tunnel] frpc-voice exited ({code:?}); restarting in {delay:?}");
        }
        // 2) Follow the main frpc: a moved relay (failover, fail-back, a
        //    list move) or a changed identity re-renders and restarts at
        //    once; a stopped / non-E2E tunnel or a disarm stops the lane.
        let moved = match (l.run.as_ref(), want.as_ref()) {
            (Some(r), Some(w)) if r.rendered != w.rendered => Some(Some(format!("{} -> {}", r.relay, w.relay))),
            (Some(_), None) => Some(None),
            _ => None,
        };
        match moved {
            Some(Some(how)) => {
                crate::log_debug!("[tunnel] voice lane follows the tunnel: {how} (restarting frpc-voice)");
                l.end_run(true);
                l.next_spawn = None;
            }
            Some(None) => l.end_run(true),
            None => {}
        }
        // 3) Nothing running and something wanted: adopt or spawn.
        if let Some(w) = want.as_ref() {
            if l.run.is_none() && l.next_spawn.is_none_or(|t| now >= t) {
                l.next_spawn = None;
                start_run(&mut l, w);
            }
        }
        // 4) Health: the run came up once frps accepted its proxy.
        let registered = l.registered();
        if let Some(r) = l.run.as_mut() {
            if registered && !r.came_up {
                r.came_up = true;
                let id = r.run_id;
                l.health().record_up(id);
            }
        }
        up = l.registered();
    }
    publish_up(up);
    let keep = ARMED.load(Ordering::SeqCst) || lane().run.is_some();
    if !keep {
        let mut l = lane();
        l.adopt_checked = false;
    }
    keep
}

/// Adopt the previous daemon's frpc-voice when it runs exactly this config,
/// else reap any stray one and spawn fresh.
fn start_run(l: &mut Lane, w: &Want) {
    let cfg_path = voice_config_path();
    let proxy = l.proxy_for(&w.label);
    let health = l.health();
    if !l.adopt_checked {
        l.adopt_checked = true;
        match frpc_proc::try_adopt(&cfg_path, &w.rendered) {
            Ok(rec) => {
                let run_id = proxy.begin_run();
                let from = frpc_proc::replay_run(&voice_log_path(), &rec, |line| proxy.observe_line(run_id, line));
                let tail_stop = tail(from, rec.pid, proxy.clone(), run_id, health.clone());
                crate::log_debug!("[tunnel] adopting running frpc-voice pid {} (same config, relay {})", rec.pid, w.relay);
                l.run = Some(Run {
                    proc: FrpcProc::Adopted { pid: rec.pid },
                    pid: rec.pid,
                    relay: w.relay.clone(),
                    rendered: w.rendered.clone(),
                    tail_stop,
                    run_id,
                    adopted: true,
                    came_up: false,
                });
                return;
            }
            Err(why) => {
                if why != frpc_proc::NoAdopt::NoRecord {
                    crate::log_debug!("[tunnel] not adopting the recorded frpc-voice ({why:?}): fresh spawn");
                }
            }
        }
    }
    if let Err(e) = crate::handoff_state::guard() {
        crate::log_debug!("[tunnel] frpc-voice not started: {e}");
        return;
    }
    // Never two: an orphan with our config still holds the proxy name.
    connector::reap_stray_frpc(&cfg_path);
    frpc_proc::clear_pid_record(&cfg_path);
    let spawned = write_config(&cfg_path, &w.rendered).and_then(|()| {
        let run_id = proxy.begin_run();
        frpc_proc::spawn_logged(&w.frpc, &cfg_path, &voice_log_path()).map(|(child, pid, offset)| (child, pid, offset, run_id))
    });
    match spawned {
        Ok((child, pid, offset, run_id)) => {
            let tail_stop = tail(offset, pid, proxy, run_id, health);
            crate::log_debug!("[tunnel] frpc-voice started, pid {pid}, relay {}", w.relay);
            l.run = Some(Run {
                proc: FrpcProc::Spawned(child),
                pid,
                relay: w.relay.clone(),
                rendered: w.rendered.clone(),
                tail_stop,
                run_id,
                adopted: false,
                came_up: false,
            });
        }
        Err(e) => {
            health.fail_with(format!("frpc-voice: {e}"));
            let delay = l.policy.on_exit(Instant::now());
            l.next_spawn = Some(Instant::now() + delay);
            crate::log_debug!("[tunnel] frpc-voice spawn failed: {e}; retrying in {delay:?}");
        }
    }
}

fn tail(offset: u64, pid: i32, proxy: Arc<ProxyRegistration>, run_id: u64, health: Arc<FrpcHealth>) -> Arc<AtomicBool> {
    let stop = Arc::new(AtomicBool::new(false));
    frpc_proc::spawn_tail(voice_log_path(), offset, pid, stop.clone(), move |line| {
        proxy.observe_line(run_id, line);
        if proxy.is_current_run(run_id) {
            health.observe_line(run_id, line);
            // Up / down at once, not at the next poll (§6.6 M11: `up` on
            // the first `start proxy success`).
            publish_up(proxy.is_registered());
        }
    });
    stop
}

/// Write the rendered config 0600 via tmp + rename.
fn write_config(path: &Path, toml: &str) -> Result<(), String> {
    let dir = path.parent().ok_or_else(|| "frpc-voice config has no parent dir".to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let tmp = dir.join(format!("{VOICE_CONFIG_NAME}.tmp.{}", std::process::id()));
    std::fs::write(&tmp, toml.as_bytes()).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    connector::restrict_mode(&tmp);
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("rename {}: {e}", path.display())
    })?;
    connector::restrict_mode(path);
    Ok(())
}

// ── status ───────────────────────────────────────────────────────────────

/// The voice lane for `k2 tunnel status` / `/boot-status` (`voiceLane`).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VoiceLaneStatus {
    /// The `voice_tunnel_preview` switch (or `K2_VOICE_TUNNEL`).
    pub preview: bool,
    /// The daemon armed it (voice runs the `tunnel` TURN tier).
    pub armed: bool,
    /// `off` (not armed or preview off), `waiting_for_tunnel` (armed, the
    /// main tunnel isn't running E2E with a label), `starting` (frpc-voice
    /// runs, frps hasn't accepted its proxy yet), `up`, or `restarting`
    /// (it exited; the next start is due).
    pub state: &'static str,
    /// frps holds this run's `k2voice-<label>`.
    pub up: bool,
    /// When `up` last changed (unix secs; 0 = never in this process).
    pub since: i64,
    pub proxy_name: Option<String>,
    pub turn_host: Option<String>,
    pub server_addr: Option<String>,
    pub server_port: Option<u16>,
    pub pid: Option<i32>,
    /// The running frpc-voice was adopted from a previous daemon.
    pub adopted: bool,
    pub consecutive_failures: u64,
    pub last_error: Option<String>,
    pub config: String,
    pub log: String,
}

pub fn status() -> VoiceLaneStatus {
    let preview = preview_enabled();
    let armed = armed();
    let follow = connector::voice_follow();
    let label = follow.as_ref().and_then(|(c, ..)| super::confirmed_label::voice_label(c));
    let l = lane();
    let (failures, last_error) = l.health.as_ref().map(|h| h.snapshot()).unwrap_or((0, None));
    let registered = l.registered();
    let state = match &l.run {
        _ if !(armed && preview) => "off",
        Some(_) if registered => "up",
        Some(_) => "starting",
        None if l.next_spawn.is_some() => "restarting",
        None => "waiting_for_tunnel",
    };
    let up = *UP.lock().unwrap_or_else(|p| p.into_inner());
    VoiceLaneStatus {
        preview,
        armed,
        state,
        up: up.up,
        since: up.since,
        proxy_name: label.as_deref().map(voice_proxy_name),
        turn_host: label.as_deref().map(voice_turn_host),
        server_addr: l.run.as_ref().map(|r| r.relay.host.clone()),
        server_port: l.run.as_ref().map(|r| r.relay.port),
        pid: l.run.as_ref().map(|r| r.pid),
        adopted: l.run.as_ref().is_some_and(|r| r.adopted),
        consecutive_failures: failures,
        last_error,
        config: voice_config_path().display().to_string(),
        log: voice_log_path().display().to_string(),
    }
}

/// Tests: forget the lane as a fresh daemon has it (no kill: a hand-off
/// test adopts the frpc-voice it left running), and disarm.
#[cfg(test)]
pub(crate) fn forget_for_test() {
    ARMED.store(false, Ordering::SeqCst);
    let mut l = lane();
    l.end_run(false);
    *l = Lane::default();
    drop(l);
    *UP.lock().unwrap_or_else(|p| p.into_inner()) = UpState::default();
    kick();
    let deadline = Instant::now() + Duration::from_secs(5);
    while SUPERVISING.load(Ordering::SeqCst) && Instant::now() < deadline {
        kick();
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
#[path = "voice_lane_tests.rs"]
mod tests;
