use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

/// Core state for the companion API proxy.
/// Stored in a module-level OnceLock, accessed by the proxy listener thread.
pub struct CompanionState {
    /// The public ngrok tunnel URL (e.g., "https://k2.ngrok.app")
    pub tunnel_url: Mutex<Option<String>>,
    /// Active authenticated sessions (token → session)
    pub sessions: Mutex<HashMap<String, Session>>,
    /// Connected WebSocket clients
    pub ws_clients: Mutex<Vec<WsClient>>,
    /// Shutdown signal — set to true to stop the proxy listener
    pub shutdown: AtomicBool,
    /// Internal hook server port (127.0.0.1:{port})
    pub hook_port: u16,
    /// Internal hook server auth token
    pub hook_token: String,
    /// Snapshot of CompanionSettings.cors_origins taken at start_companion.
    /// Empty → no CORS headers emitted. Restart the companion to pick up changes.
    pub cors_origins: Vec<String>,
    /// Snapshot of CompanionSettings.allow_remote_spawn taken at
    /// start_companion. When false, /companion/terminal/spawn endpoints
    /// return 403. Restart the companion to pick up changes.
    pub allow_remote_spawn: bool,
    /// Per-IP rate limiter gating /companion/auth attempts.
    pub auth_limiter: Mutex<AuthRateLimiter>,
    /// Cache for reflowed grid JSON strings, keyed by
    /// `(terminal_id, (cols, rows))`. Value is `(seqno, serialized_json)`.
    /// Hit when the stored seqno matches the current grid seqno — the
    /// reflow+serialize is reused instead of recomputed per-tick. Entries
    /// for a terminal are auto-invalidated on the next tick whose seqno
    /// is newer: the old string is simply overwritten. Bounded in practice
    /// by active mobile client count (a handful per session).
    pub reflow_cache: Mutex<std::collections::HashMap<(String, (u16, u16)), (u64, String)>>,
    /// Keeps the ngrok runtime thread alive — drop this to stop the tunnel
    pub _tunnel_keepalive: Mutex<Option<std::sync::mpsc::Sender<()>>>,
}

/// An authenticated companion session (24hr TTL).
pub struct Session {
    pub token: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub last_active: chrono::DateTime<chrono::Utc>,
    pub remote_addr: String,
    /// Rate limiting: request count in current window
    pub request_count: u32,
    /// Rate limiting: window start time
    pub window_start: Instant,
}

impl Session {
    /// Check if the session has expired.
    pub fn is_expired(&self) -> bool {
        chrono::Utc::now() > self.expires_at
    }

    /// Check rate limit (60 requests per 60 seconds).
    /// Returns true if within limit, false if exceeded.
    pub fn check_rate_limit(&mut self) -> bool {
        let now = Instant::now();
        let window_duration = std::time::Duration::from_secs(60);

        if now.duration_since(self.window_start) > window_duration {
            // Reset window
            self.window_start = now;
            self.request_count = 1;
            true
        } else if self.request_count < 60 {
            self.request_count += 1;
            true
        } else {
            false // Rate limit exceeded
        }
    }
}

/// Per-IP brute-force limiter for /companion/auth.
///
/// Thresholds: 5 attempts per minute + 20 attempts per hour per source IP.
/// Mirrors the code-server and vaultwarden pattern — simple fixed-window
/// counters rather than a full `governor` dep since volume is low.
///
/// Attempts are counted regardless of outcome: an argon2 verify runs anyway
/// once past the limiter (~100ms), so counting successes is cheap insurance
/// against a compromised credential doing 10k reauths per minute.
pub struct AuthRateLimiter {
    per_ip: HashMap<IpAddr, AuthAttempts>,
}

impl Default for AuthRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthRateLimiter {
    pub const MINUTE_LIMIT: u32 = 5;
    pub const HOUR_LIMIT: u32 = 20;

    pub fn new() -> Self {
        Self {
            per_ip: HashMap::new(),
        }
    }

    /// Returns Ok(()) if allowed, Err(retry_after_secs) if rate-limited.
    /// Increments the per-IP counter on allow.
    pub fn check_and_record(&mut self, ip: IpAddr) -> Result<(), u64> {
        self.prune_stale(Instant::now());
        let entry = self.per_ip.entry(ip).or_insert_with(AuthAttempts::new);
        entry.check_and_increment()
    }

    /// Drop entries whose hour window has fully closed — bounded memory.
    fn prune_stale(&mut self, now: Instant) {
        let hour = Duration::from_secs(3600);
        self.per_ip
            .retain(|_, a| now.duration_since(a.hour_window_start) < hour);
    }
}

struct AuthAttempts {
    minute_count: u32,
    minute_window_start: Instant,
    hour_count: u32,
    hour_window_start: Instant,
}

impl AuthAttempts {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            minute_count: 0,
            minute_window_start: now,
            hour_count: 0,
            hour_window_start: now,
        }
    }

    fn check_and_increment(&mut self) -> Result<(), u64> {
        let now = Instant::now();
        let minute = Duration::from_secs(60);
        let hour = Duration::from_secs(3600);

        if now.duration_since(self.minute_window_start) > minute {
            self.minute_window_start = now;
            self.minute_count = 0;
        }
        if now.duration_since(self.hour_window_start) > hour {
            self.hour_window_start = now;
            self.hour_count = 0;
        }

        if self.minute_count >= AuthRateLimiter::MINUTE_LIMIT {
            let elapsed = now.duration_since(self.minute_window_start).as_secs();
            return Err(60u64.saturating_sub(elapsed));
        }
        if self.hour_count >= AuthRateLimiter::HOUR_LIMIT {
            let elapsed = now.duration_since(self.hour_window_start).as_secs();
            return Err(3600u64.saturating_sub(elapsed));
        }

        self.minute_count += 1;
        self.hour_count += 1;
        Ok(())
    }
}

/// A connected WebSocket client.
pub struct WsClient {
    /// Unique client ID (UUID)
    pub client_id: String,
    pub session_token: String,
    /// Whether this client has authenticated via the WS auth message
    pub authenticated: bool,
    pub subscribed_terminals: HashSet<String>,
    /// Mobile screen dimensions for shadow terminal reflow.
    /// If set, grid updates are reflowed to these dimensions before sending.
    pub mobile_dims: Option<(u16, u16)>, // (cols, rows)
    /// Channel to send messages to the WS writer thread
    pub sender: std::sync::mpsc::Sender<String>,
    /// Last time we received any message from this client
    pub last_seen: Instant,
}
