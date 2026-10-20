# sew-client

The typed async Rust client for Sew (slice S25): the client HTTP API, the mgmt socket with
act-as, the websocket gateway, and the sync position model. Tests, `sewc`, the `sew` admin CLI,
the load harness and K2 all use it. It builds on Linux and macOS.

## Quick start

```rust
use sew_client::{Client, MgmtClient, RequestOptions};

let c = Client::new("https://chat.example.com")?;
c.auth_k2(&pass).await?;                 // the session token is kept in the client
let me = c.me().await?;
let spaces = c.spaces(&Default::default()).await?;

// Errors are typed C5 codes.
match c.channel(id).await {
    Err(e) if e.is_not_found() => { /* missing or hidden: the same thing */ }
    other => { /* … */ }
}

// The mgmt socket (Bearer sewsvc_…), and acting as an agent over it.
let mgmt = MgmtClient::from_env()?;      // $SEW_MGMT_SOCKET, $SEW_MGMT_TOKEN_FILE
let r = mgmt.provision(&req, RequestOptions::new()).await?;
let agent = mgmt.act_as(target, via);     // a normal Client: every typed method works
let me = agent.me().await?;
```

K2 sessions renew themselves with a pass source:

```rust
struct Passes(/* K2's signer */);
impl sew_client::PassSource for Passes {
    fn fresh_pass(&self) -> BoxFuture<'_, Result<SecretString, ClientError>> { /* … */ }
}
c.login_k2(Arc::new(Passes(..))).await?;  // renews under 10 min left; one re-login on 401
```

## Features

| Feature | Pulls in | K2 uses |
|---|---|---|
| (always) | reqwest (rustls), tokio, futures-util, sew-proto | yes |
| `mgmt` | hyper, hyper-util (unix only) | yes |
| `sync` | nothing extra | yes |
| `gateway` | tokio-tungstenite, zstd, rustls (wss) | no |
| `cli` | clap; enables `sewc` | no |

`default = ["gateway", "mgmt", "sync", "cli"]`. No OpenSSL anywhere
(`scripts/check-client-features.sh` checks every combination and the tree).

K2's dependency line (the tag comes from `CHANGELOG.md`; `scripts/client-semver.sh` gates each
tag):

```toml
sew-client = { git = "ssh://git@github.com/Alakazam-211/sew", tag = "sew-client-v0.1.0",
               default-features = false, features = ["mgmt", "sync"] }
```

`sew_client::proto` re-exports the exact `sew-proto` the client was built with; use that path.

## Rules K2 can rely on (S25 §16)

- The library never creates a runtime, blocks, or spawns (`tokio::spawn`, `spawn_blocking`,
  `block_on` are lint-checked out). It needs a Tokio 1.x runtime with IO and time enabled.
- Every public future is `Send` (compile-checked).
- Cancel-safe: `Gateway::next_event`, `DeliveryCursor::poll` (the cursor moves only on return),
  and every HTTP call (dropping it sends no further retry).
- No global state; environment variables are read only in `MgmtClient::from_env`; logs go
  through `tracing` only; secrets are `SecretString` and never appear in `Debug` or `Display`.
- **Idempotency keys:** every `POST` except `/api/v1/auth/*` carries an `Idempotency-Key`,
  generated once per call (`sc_` + 22 base64url chars) and reused with byte-identical bodies on
  every retry. The server scopes keys per acted-as member, so **K2 must pass its own key for
  durable work, unique per agent and purpose**: `k2-<purpose>-<outbox row id>`
  (`RequestOptions::new().idempotency_key(k)`). A retry after a daemon restart then replays
  instead of duplicating (`meta.replayed`).

## Retries

"Safe" = a `GET`, or a `POST` with an idempotency key. Defaults: 4 attempts, 60 s total,
full-jitter backoff `uniform[0, min(5 s, 250 ms × 2^n))`, a server wait over 10 s is returned
at once.

| Outcome | Retried when | Wait |
|---|---|---|
| Transport error, the request never left (connect failed) | always | backoff |
| Transport error after sending, or a timeout | safe | backoff |
| 429 `rate_limited` | always (a 429 has no side effects) | `details.retry_after_ms`, else `Retry-After`, else 1 s; + 0–100 ms |
| 503 `unavailable` | safe | `Retry-After`, else backoff |
| 409 `idempotency_conflict` `reason = in_progress` | always (same key) | `Retry-After` (1 s) |
| 409 `body_mismatch`, 500 `internal`, any other 4xx | never | — |
| `/api/v1/auth/*` | only when the request never left (a pass is single use) | backoff |

`call()` returns `Response<T>` with `ResponseMeta` (`attempts`, `replayed`, `request_id`,
`rate_limit`, `server_date`). The typed methods return the value only.

## The gateway in a background task

`Gateway` is pull-driven: nothing happens between `next_event` calls. To run it in the
background, spawn a task yourself and forward events:

```rust
let mut gw = client.gateway(GatewayOptions::default())?;
let (tx, mut rx) = tokio::sync::mpsc::channel(256);
tokio::spawn(async move {
    loop {
        match gw.next_event().await {
            Ok(ev) => if tx.send(ev).await.is_err() { gw.close().await; return },
            Err(_closed) => return,            // after Stopped(..)
        }
    }
});
while let Some(ev) = rx.recv().await {
    match ev {
        GatewayEvent::Ready(r) => { /* a fresh session: catch up (S25 §13) */ }
        GatewayEvent::Resumed { replayed } => { /* nothing was lost */ }
        GatewayEvent::Dispatch { s, event } => { /* … */ }
        GatewayEvent::Disconnected { next, after, .. } => { /* reconnecting by itself */ }
        GatewayEvent::Stopped(reason) => { /* terminal: 4011 revoked, 4004 auth, gave up */ }
        _ => {}
    }
}
```

Close codes follow C7 and contracts rev 22 (`4004 session_expired` gets a fresh credential and
resumes; other 4004s re-identify with a pass source, else stop). Reconnects back off with full
jitter from 1 s to 60 s; IDENTIFY is gated to one per 5 s per token.

## Sync

`SyncState::plan(&SyncBoot)` is a pure planner (drop all / drop channels / member log / fresh /
pulls); `JsonFileStore` keeps the state atomically with mode 0600; `DeliveryCursor` follows an
agent's delivery feed and resets to the head on a `sync_epoch` change or a 410 (rev 36). The
catch-up driver lands in S25.3.

## `sewc`

`sewc` is the dev and test CLI (`--json` for scripts; exit codes 0 ok, 1 API, 2 usage/config,
3 auth, 4 not found, 5 forbidden, 6 rate limited, 7 transport, 8 test failed). S25.1 ships
`login`, `logout`, `whoami`, `health`, `spaces`, `channels`, `resolve`, `gateway watch`,
`gateway resume-test`, `mgmt …`, `raw`, and `--act-as`. S33 v2 and S50 add `voice …`; S34 v2
adds `call join|decline|ring|show` and `voice video|screen on|off`. `voice join` and `call join`
post a body an enrolled E2EE device made (sewc has no MLS client of its own).
