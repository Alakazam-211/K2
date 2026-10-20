//! Sessions: the token cell, K2 renew through a pass source, re-login (S25 §11).
//!
//! The cell holds the current token, its expiry on the client's clock, the provider, the member
//! and a generation that bumps on every change. The gateway watches the generation and sends
//! `REAUTH {token}` on the live socket after a change (C7).

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use futures_util::future::BoxFuture;
use sew_proto::api::auth::SessionResponse;
use sew_proto::clock::Clock;
use sew_proto::id::Id;
use tokio::sync::watch;

use crate::api::Client;
use crate::error::ClientError;
use crate::secret::SecretString;

/// Renew a K2 session when less than this is left (S25 §11).
pub const RENEW_BEFORE: Duration = Duration::from_secs(10 * 60);

/// Supplies fresh K2 passes (JWT strings) for login, renew and re-login. K2 implements it over
/// its pass signer; `sewc` runs a `--pass-cmd`.
pub trait PassSource: Send + Sync {
    /// A fresh pass. Called once per login, renew or re-login; passes are single use.
    fn fresh_pass(&self) -> BoxFuture<'_, Result<SecretString, ClientError>>;
}

/// Which identity provider issued the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Provider {
    /// A K2 pass (1 h sessions, renewable).
    K2,
    /// A standalone password login (30 d / 7 d idle, no auto-login).
    Standalone,
}

impl Provider {
    /// `k2` or `standalone`.
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::K2 => "k2",
            Provider::Standalone => "standalone",
        }
    }
}

#[derive(Default)]
struct CellState {
    token: Option<SecretString>,
    /// Expiry in the client clock's unix ms.
    expires_local_ms: Option<u64>,
    provider: Option<Provider>,
    member_id: Option<Id>,
    generation: u64,
}

/// The shared session state of a [`Client`] and its clones.
pub(crate) struct TokenCell {
    state: Mutex<CellState>,
    pass_source: Mutex<Option<Arc<dyn PassSource>>>,
    /// One renew or re-login at a time; concurrent callers wait and reuse the result.
    pub(crate) renew: tokio::sync::Mutex<()>,
    changed: watch::Sender<u64>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl fmt::Debug for TokenCell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = lock(&self.state);
        f.debug_struct("TokenCell")
            .field("token", &s.token.as_ref().map(|_| "***"))
            .field("expires_local_ms", &s.expires_local_ms)
            .field("provider", &s.provider)
            .field("member_id", &s.member_id)
            .field("generation", &s.generation)
            .finish()
    }
}

impl TokenCell {
    pub(crate) fn new(token: Option<SecretString>, pass: Option<Arc<dyn PassSource>>) -> TokenCell {
        let (tx, _) = watch::channel(0);
        TokenCell {
            state: Mutex::new(CellState {
                token,
                ..CellState::default()
            }),
            pass_source: Mutex::new(pass),
            renew: tokio::sync::Mutex::new(()),
            changed: tx,
        }
    }

    pub(crate) fn token(&self) -> Option<SecretString> {
        lock(&self.state).token.clone()
    }

    pub(crate) fn member_id(&self) -> Option<Id> {
        lock(&self.state).member_id
    }

    pub(crate) fn set_member_id(&self, id: Id) {
        lock(&self.state).member_id = Some(id);
    }

    pub(crate) fn provider(&self) -> Option<Provider> {
        lock(&self.state).provider
    }

    pub(crate) fn generation(&self) -> u64 {
        lock(&self.state).generation
    }

    #[cfg_attr(not(feature = "gateway"), allow(dead_code))]
    pub(crate) fn subscribe(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    pub(crate) fn pass_source(&self) -> Option<Arc<dyn PassSource>> {
        lock(&self.pass_source).clone()
    }

    pub(crate) fn set_pass_source(&self, p: Arc<dyn PassSource>) {
        *lock(&self.pass_source) = Some(p);
    }

    /// Stores a new session. The expiry is tracked on the client's clock: the lifetime is
    /// `expires_at − server Date` (so clock skew does not matter), counted from now.
    pub(crate) fn set_session(
        &self,
        s: &SessionResponse,
        server_date: Option<SystemTime>,
        provider: Provider,
        clock: &dyn Clock,
    ) {
        let now = clock.now_unix_ms();
        let expires = u64::try_from(s.expires_at.unix_ms()).unwrap_or(0);
        let issued = server_date
            .and_then(|d| d.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(now);
        let lifetime = expires.saturating_sub(issued);
        let generation = {
            let mut st = lock(&self.state);
            st.token = Some(SecretString::new(s.token.clone()));
            st.expires_local_ms = Some(now.saturating_add(lifetime));
            st.provider = Some(provider);
            st.member_id = Some(s.member.id);
            st.generation += 1;
            st.generation
        };
        self.changed.send_replace(generation);
    }

    /// Forgets the token (after logout).
    pub(crate) fn clear(&self) {
        let generation = {
            let mut st = lock(&self.state);
            st.token = None;
            st.expires_local_ms = None;
            st.generation += 1;
            st.generation
        };
        self.changed.send_replace(generation);
    }

    /// Time left on the client's clock, when the expiry is known.
    pub(crate) fn remaining(&self, clock: &dyn Clock) -> Option<Duration> {
        let exp = lock(&self.state).expires_local_ms?;
        Some(Duration::from_millis(
            exp.saturating_sub(clock.now_unix_ms()),
        ))
    }

    /// A held K2-or-unknown session with a known expiry under [`RENEW_BEFORE`].
    pub(crate) fn needs_renew(&self, clock: &dyn Clock) -> bool {
        let st = lock(&self.state);
        if st.token.is_none() || st.provider == Some(Provider::Standalone) {
            return false;
        }
        match st.expires_local_ms {
            Some(exp) => exp.saturating_sub(clock.now_unix_ms()) < RENEW_BEFORE.as_millis() as u64,
            None => false,
        }
    }
}

impl Client {
    /// Logs in with a pass from `src` (`POST /api/v1/auth/k2`) and keeps `src` for renew and
    /// re-login.
    pub async fn login_k2(&self, src: Arc<dyn PassSource>) -> Result<SessionResponse, ClientError> {
        let cell = self.session_cell()?;
        cell.set_pass_source(src.clone());
        let pass = src.fresh_pass().await?;
        self.auth_k2(pass.expose()).await
    }

    /// Renews a K2 session when less than 10 minutes are left (`POST /api/v1/auth/renew` with
    /// the current Bearer and a fresh pass); if renew is refused, logs in again. A no-op
    /// without a pass source, for standalone sessions, and when acting as an agent. Runs before
    /// every HTTP call. One renew at a time; concurrent callers wait and reuse the result.
    pub async fn ensure_fresh(&self) -> Result<(), ClientError> {
        let Ok(cell) = self.session_cell() else {
            return Ok(());
        };
        let Some(src) = cell.pass_source() else {
            return Ok(());
        };
        let clock = self.clock();
        if !cell.needs_renew(clock.as_ref()) {
            return Ok(());
        }
        let _g = cell.renew.lock().await;
        if !cell.needs_renew(clock.as_ref()) {
            return Ok(());
        }
        let pass = src.fresh_pass().await?;
        match self.auth_renew(pass.expose()).await {
            Ok(_) => Ok(()),
            Err(e) if e.is_auth() => {
                tracing::debug!(error = %e, "sew-client: renew refused; logging in again");
                self.relogin_locked(src.as_ref()).await.map(|_| ())
            }
            Err(e) => Err(e),
        }
    }

    /// Logs in again with a fresh pass, unless the token changed since `seen_generation`
    /// (another caller already refreshed it). `Ok(false)` when there is no pass source.
    pub(crate) async fn relogin_since(&self, seen_generation: u64) -> Result<bool, ClientError> {
        let cell = self.session_cell()?;
        let Some(src) = cell.pass_source() else {
            return Ok(false);
        };
        let _g = cell.renew.lock().await;
        if cell.generation() != seen_generation {
            return Ok(true);
        }
        self.relogin_locked(src.as_ref()).await.map(|_| true)
    }

    async fn relogin_locked(&self, src: &dyn PassSource) -> Result<SessionResponse, ClientError> {
        let pass = src.fresh_pass().await?;
        self.auth_k2(pass.expose()).await
    }

    /// Whether a pass source is set (the session can renew and log in again by itself).
    pub fn has_pass_source(&self) -> bool {
        self.session_cell()
            .map(|c| c.pass_source().is_some())
            .unwrap_or(false)
    }

    /// The session provider, once a login or renew happened on this client.
    pub fn provider(&self) -> Option<Provider> {
        self.session_cell().ok().and_then(|c| c.provider())
    }

    /// Time left on the session (client clock), when known.
    pub fn session_remaining(&self) -> Option<Duration> {
        let cell = self.session_cell().ok()?;
        cell.remaining(self.clock().as_ref())
    }
}
