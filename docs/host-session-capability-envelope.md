# Host-session capability envelope — integrator note (S5)

**Version:** first adoptable cut / **0.40.77** (JWKS public; 0.40.76 shipped envelope)  
**Audience:** apps that spawn `/v1` host-sessions and need agents to call the app’s API (e.g. Scout).  
**PRD:** `.k2/prds/prd-v1-host-session-capabilities-v1.md` §0  

This note is normative for final-test. OPS UI, rich queue, sandbox-family envelope, and full key-rotation runbooks are **out of scope** here.

---

## 1. Two tokens (do not conflate)

| Token | Plane | Who uses it | Lifetime |
|-------|--------|-------------|----------|
| **K2 API key** (`k2sk_…`) | Control | Integrator → K2 `/v1/*` | Long-lived until `k2 api-key revoke <id>` (owner-tier) |
| **Capability JWT** | Data | Agent process → **your app** | Short `exp`; re-staged on resume as **file** |

One API key may spawn many `sessionId`s. Each session gets its own short-lived capability handles for write-back — that is **not** “more API keys.”

---

## 2. Wire samples (concrete)

### 2.1 Spawn (cold) with capabilities

```http
POST /v1/w/sales/host-sessions
Authorization: Bearer k2sk_…
Content-Type: application/json

{
  "prompt": "Ground on Phase-1 answers and write sales findings for this interview. Credentials are in the capability file — never paste them into chat.",
  "timeout_secs": 600,
  "capabilities": [
    {
      "kind": "http_callback",
      "audience": "https://scout.example/api/v1/interviews/{resource_id}/phase1",
      "resource": "interview:ivw_abc",
      "actions": ["GET"]
    },
    {
      "kind": "http_callback",
      "audience": "https://scout.example/api/v1/interviews/{resource_id}/results",
      "resource": "interview:ivw_abc",
      "actions": ["POST"]
    }
  ]
}
```

**Example response (non-secret):**

```json
{
  "sessionId": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
  "agentName": "api-…",
  "workspace": "sales",
  "sandbox": "none",
  "stream": { "grid": "/cli/sessions/grid?session=…&token=…" },
  "capabilities": {
    "staged": true,
    "env": "K2_CAPABILITY_TOKEN",
    "expires_at": "2026-08-02T12:10:00Z",
    "jtis": ["jti-read-001", "jti-write-002"]
  }
}
```

`expires_at` is the earliest JWT `exp` among this mint (here both tokens share
`exp=1785672600` → `2026-08-02T12:10:00Z`, matching the cap-file examples in §2.3).

**`workspace` is always the URL slug** (`sales`), never the host filesystem path —
same value on cold-spawn, live-resume (`resumed: true`), and dead-resume
(`resumed: false`). Integrators may key plan→session registries on it.

**Resource namespace:** each capability `resource` **must** be `interview:<id>`
(e.g. `interview:ivw_abc`). Other prefixes (e.g. `space:…`) are rejected with
**400** `capabilities-invalid` (`must start with "interview:"`). Do not send
Scout plan/space ids without the `interview:` prefix.

**Response shape (one envelope for all paths):** session/status fields are
always **top-level**. `capabilities` is **only** the non-secret mint metadata
sub-object (`staged` / `env` / `expires_at` / `jtis`) — never a container for
`sessionId` / `workspace` / `resumed` / `delivered` / `live`. Live-resume adds
top-level `resumed`/`live`/`delivered`; cold-spawn adds `agentName` + `stream`
and omits `resumed` (fresh). Dead-resume is cold shape + `resumed: false`.

Fresh spawn **omits** `resumed`.

### 2.2 Staging (file SSOT + re-send obligation)

| Channel | When | Notes |
|---------|------|--------|
| **File** `{workspace}/.k2/caps/<sessionId>.json` | Every mint / remint | Multi-turn **SSOT**. Mode **0600**. Atomic write: temp + `rename()`. |
| **Env** `K2_CAPABILITY_TOKEN` | Cold spawn only | Same JSON string as the file. **Live resume cannot update env.** |

**Re-send-caps-on-resume (required for multi-turn envelope):**  
On every multi-turn resume / continue, **ALWAYS re-send `capabilities[]`** so K2 remints and overwrites the cap file.  
**Omit `capabilities[]` ⇒ no remint ⇒ file may be stale.** Do not write with a stale file after a long gap or after `resumed: false` without remint.

### 2.2b First-turn prompt transport (launch-param)

On **cold spawn** and **dead resume** (not live inject), host-sessions deliver the
caller `prompt` (plus frozen respond preamble + owner guest policy) as an
**interactive CLI launch parameter** when the workspace agent supports it
(Claude, Codex, Grok, Gemini, Cursor Agent, Pi). Hermes and unknown agents keep
post-spawn paste inject.

- Launch-param is **fire-once / request-scoped**: it is **not** stored in
  `args_json` or recovery replay.
- Live follow-ups (`message-live` / live resume) still inject only.
- Secrets still must not ride `prompt` — use this envelope’s cap file / env.
- See `.k2/prds/prd-host-session-launch-param-prompt-v1.md`.

### 2.3 Cap-file schema (what the agent parses each turn)

Path: `{workspace}/.k2/caps/<sessionId>.json`

**Schema:** a JSON **array** of objects (one per capability mint). Each object:

| Field | Type | Meaning |
|-------|------|---------|
| `actions` | `string[]` | Granted HTTP methods, e.g. `["GET"]` or `["POST"]` |
| `token` | `string` | Compact ES256 JWT |

**No other required top-level fields** in the file for v1. Response metadata (`jtis`, `expires_at`) is on the **HTTP response**, not necessarily mirrored in the file.

**Example file contents** (after spawn or remint; tokens abbreviated):

```json
[
  {
    "actions": ["GET"],
    "token": "eyJhbGciOiJFUzI1NiIsImtpZCI6Ii4uLiJ9.eyJpc3MiOiJrMi1ob3N0LXNlc3Npb25zIiwiYXVkIjoiaHR0cHM6Ly9zY291dC5leGFtcGxlL2FwaS92MS9pbnRlcnZpZXdzL2l2d19hYmMvcGhhc2UxIiwic3ViIjoiYTFiMmMzZDQtLi4uIiwicmVzb3VyY2UiOiJpbnRlcnZpZXc6aXZ3X2FiYyIsImFjdGlvbnMiOlsiR0VUIl0sImV4cCI6MTc4NTY3MjYwMCwiaWF0IjoxNzg1NjY5MDAwLCJqdGkiOiJqdGktcmVhZC0wMDEifQ.…"
  },
  {
    "actions": ["POST"],
    "token": "eyJhbGciOiJFUzI1NiIsImtpZCI6Ii4uLiJ9.eyJpc3MiOiJrMi1ob3N0LXNlc3Npb25zIiwiYXVkIjoiaHR0cHM6Ly9zY291dC5leGFtcGxlL2FwaS92MS9pbnRlcnZpZXdzL2l2d19hYmMvcmVzdWx0cyIsInN1YiI6ImExYjJjM2Q0LS4uLiIsInJlc291cmNlIjoiaW50ZXJ2aWV3Oml2d19hYmMiLCJhY3Rpb25zIjpbIlBPU1QiXSwiZXhwIjoxNzg1NjcyNjAwLCJpYXQiOjE3ODU2NjkwMDAsImp0aSI6Imp0aS13cml0ZS0wMDIifQ.…"
  }
]
```

Illustrative only (signature truncated). Payload claims: `exp=1785672600` /
`iat=1785669000` → **12:10:00Z / 11:10:00Z** on `2026-08-02`, same wall-clock as
response `expires_at` in §2.1.

**Agent obligation:** at the **start of each turn**, re-read this file and pick the JWT whose `actions` match the method you will call. **Do not cache turn-1’s JWT** across turns.

### 2.4 Live resume (`resumed: true`)

```http
POST /v1/w/sales/host-sessions
Authorization: Bearer k2sk_…
Content-Type: application/json

{
  "session": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
  "prompt": "Continue synthesis for this interview.",
  "timeout_secs": 600,
  "capabilities": [
    {
      "kind": "http_callback",
      "audience": "https://scout.example/api/v1/interviews/{resource_id}/phase1",
      "resource": "interview:ivw_abc",
      "actions": ["GET"]
    },
    {
      "kind": "http_callback",
      "audience": "https://scout.example/api/v1/interviews/{resource_id}/results",
      "resource": "interview:ivw_abc",
      "actions": ["POST"]
    }
  ]
}
```

**Example response (same top-level field homes as cold-spawn; live flags added):**

```json
{
  "sessionId": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
  "delivered": true,
  "live": true,
  "resumed": true,
  "workspace": "sales",
  "sandbox": "none",
  "capabilities": {
    "staged": true,
    "env": "K2_CAPABILITY_TOKEN",
    "expires_at": "2026-08-02T12:10:00Z",
    "jtis": ["jti-read-003", "jti-write-004"]
  }
}
```

- Caps present → **fresh mint**, atomic file overwrite, **new jtis**.
- Caps omitted → **no remint** (and no `capabilities` key on the response).
- Prior jtis remain cryptographically valid until **their** `exp`. For **single-valid** semantics, **add old jtis to your local revoke set** when you receive the new set (recommended for Scout). K2 does **not** server-invalidate old jtis.

Same optional fields on `POST /v1/w/<ws>/host-sessions/<id>` (message-live).

### 2.5 Dead resume (`resumed: false`) — **normative sequence**

When the cell is **dead**, the same POST body with `session` + **`capabilities[]` re-sent** does a **full re-spawn** (new live PTY), returns **`resumed: false`**, and **mints into the cap file** as part of that spawn.

**`sessionId` stays STABLE** — it is the same id the integrator addressed.
`resumed: false` is the **sole** re-spawn discriminator (new PTY under the
same handle). Do **not** expect a new `sessionId` on dead-resume.

**Integrator sequence for final-test (aligned):**

```
1. Detect resumed: false  (lost live continuity / turn-1 cell state)
2. Re-send capabilities[] on that (or immediate follow-up) host-sessions call
3. K2 fresh-mints → atomic overwrite of .k2/caps/<sessionId>.json
4. Agent starts / re-reads cap file
5. ONLY THEN perform turn-2 writes  (no write rides a stale file)
```

History-replay of prompt tokens is moot for the envelope path (token is out of prompt). Do **not** silent-continue as if turn-1 init still holds.

---

## 3. JWKS + verify layers

### 3.1 Fetch JWKS

```http
GET /v1/jwks
```

**No `Authorization` header. No API key.** Public — same tier as `/boot-status`
(not behind the authenticated `/v1/*` arm). A verifier holds **no** K2 secret by
design; requiring Bearer here would defeat asymmetric ES256 verify. Served even
when the rest of the `/v1` spawn surface is dark (still only public key material).

Response:

```json
{
  "keys": [
    {
      "kty": "EC",
      "crv": "P-256",
      "x": "…",
      "y": "…",
      "use": "sig",
      "alg": "ES256",
      "kid": "…"
    }
  ]
}
```

### 3.2 Layer A — JWT validity (crypto + claims)

| Check | Required value |
|--------|----------------|
| Signature | ES256 against JWKS entry matching `kid` |
| `alg` | `ES256` |
| `iss` | `k2-host-sessions` |
| `aud` | Exact audience URL for **this** endpoint (after `{resource_id}` resolution) |
| `sub` | Host-session `sessionId` (if you track binding) |
| `exp` | Not expired |
| `iat` | Present (issued-at) |

### 3.3 Layer B — App AUTHZ (valid JWT ≠ authorized write)

A cryptographically valid JWT is **not** enough. On every agent callback your app **must** also:

| Check | Purpose |
|--------|---------|
| **`resource` equals the plan/interview id implied by the URL** | **Cross-plan guard** — token for plan A must not write plan B. If `aud` already encodes the interview endpoint uniquely, still bind `resource` to the URL plan id. |
| **HTTP method ∈ JWT `actions`** | Verb grant (e.g. POST requires `"POST"` in `actions`) |
| **`jti` not in your local revocation set** | Completion / remint single-valid / abuse (PRD decision 7) |

### 3.4 Signing key (pilot)

- Private: **`~/.k2/capability-signing.pem`** on the **daemon host** (mode 0600), generated on first use.  
- **STATIC for the pilot — no rotation procedure yet.**  
- Public: only via `GET /v1/jwks`. Full rotation runbook = post-pilot ops debt.

---

## 4. Reliability

### 4.1 `timeout_secs`

- Clamp 30…86400; **default 180** if omitted.  
- Integrator: `timeout_secs >= max_inter_shot_gap + turn_2_budget`.  
- **Scout multi-turn: 600** (and client poll ≥ 600).

### 4.2 S9 work-completion reaper (product lock 2026-08-03)

Persistent-interview model: agents must live across user think-time and long
mid-write turns. **No spawn-time hard wall.**

| Number | Value | Role |
|--------|-------|------|
| `timeout_secs` | request, default **180** (clamp 30…86400) | Client poll budget / JWT lifetime clamp; **does not kill Working** |
| Grace after `--final` | **10s** (`FINAL_GRACE_SECS`) | Completion reap after final |
| Reaper tick | **15s** (env `K2_SANDBOX_REAPER_TICK_SECS`) | Poll cadence |

Behavior:

- Inject / register / non-final `k2 respond` → **Working** — **never** auto-reaped
  (no silence reap, no `timeout_secs` wall from spawn). Continuous productive
  work (e.g. Scout E-1 16 fence qs mid-write past 300s) survives.
- `k2 respond --final` → **Grace** → reap after **~10s** (unless a new inject /
  non-final respond re-enters Working).
- **Spend control** = integrator **`POST …/kill`** + capability non-remint / caps.

**Client verification (E-1 + completion):** spawn A with continuous mid-write /
no `--final` at `timeout_secs=300` — must survive past 300s wall. Spawn B that
reaches `--final` — dies within ~20–30s (grace). Resume/inject re-stamps Working.

K2-side unit evidence: `sandbox_reaper::tests::{working_survives_idle_window,
working_survives_past_timeout_secs_wall, working_survives_long_mid_write_silence,
grace_reaps_after_deadline}`.

### 4.3 Inject / auto-retry (no model cancel)

1. Per-session **injection lock** serializes pastes.  
2. Concurrent inject: wait or fail (**`pty_stalled`** / failed deliver) — not silent drop.  
3. **No** cancel of in-flight LLM generation.  
4. Retry into live-but-stuck may leave **two generations** in one cell.  
5. Write safety = app **`turn_id` idempotency** (+ AUTHZ layer §3.3).  

Live resume is **queue-exempt / cap-neutral**.

---

## 5. Concurrent ceilings + stable 429 codes

Defaults (env-overridable): **principal 64** / **workspace 15** / **global 512**.

**Queue (S8):** when at a cap, spawn **waits** up to `K2_SANDBOX_QUEUE_WAIT_SECS`
(default **30s**) polling for a free slot. On deadline still full → **429 with
the blocking cap’s code** (`workspace-cell-cap` / `concurrent-cell-cap` /
`cell-capacity`). Pre-0.40.78 this always became `spawn-queue-timeout` after
the wait (pilot F6 — workspace-15 never appeared under sustained load).
`K2_SANDBOX_QUEUE_WAIT_SECS=0` → immediate refuse, same codes.

| `code` | Meaning |
|--------|---------|
| `concurrent-cell-cap` | Per-API-key principal full |
| `workspace-cell-cap` | This workspace full |
| `cell-capacity` | Daemon global full |
| `spawn-queue-timeout` | Legacy / reserved (post-0.40.78 acquire path surfaces concrete cap codes after wait) |
| `spawn-queue-full` | Durable spawn queue depth exceeded (feature `K2_HOST_SESSION_SPAWN_QUEUE`; default **OFF**) |

Map all five to an **honest retry** UX — never an infinite spinner.

### 5.0 Durable spawn queue (optional, default OFF)

Env `K2_HOST_SESSION_SPAWN_QUEUE=1` enables a **path-keyed FIFO** of cold /
dead-resume host-session spawns when at cap (prd-host-session-spawn-queue-v1).
Default is **OFF** until integrators poll jobs (202 + no `sessionId` is **not**
spawn success).

When the feature is **ON**:

- Admit uses **nowait** acquire (no long S8 open-HTTP wait — fairness vs FIFO).
- Cap refuse + `queue` allowed (default `true`) → **202 Accepted**
  `{ "queued": true, "jobId", "position", "workspace" }` (no `sessionId` for cold).
- Cap refuse + `"queue": false` → **immediate 429** with the blocking cap code.
- Queue depth exceeded → **429** `spawn-queue-full`.
- Every quota **release** (ChildExit, early fail, not_live orphan) wakes the
  workspace FIFO head; capability JWTs mint only at drain/spawn time.
- Status / cancel:
  - `GET /v1/w/<ws>/host-sessions/queue`
  - `GET /v1/w/<ws>/host-sessions/queue/<jobId>`
  - `POST /v1/w/<ws>/host-sessions/queue/<jobId>/cancel`

When the feature is **OFF**: legacy S8 wait then 429 only (byte-compatible).
Live inject / live-resume never enqueue.

---

## 5.1 `GET /v1/w/<ws>/host-sessions` (list)

**Intended API** (audit): authorized workspace → list `api-…` host sessions.

```http
GET /v1/w/sales/host-sessions
Authorization: Bearer k2sk_…
```

```json
{
  "workspace": "sales",
  "sessions": [
    {
      "sessionId": "a1b2c3d4-…",
      "agentName": "api-…",
      "live": true,
      "lastSeenAt": 1754140000
    }
  ]
}
```

- **One row per `sessionId`** (latest `lastSeenAt` wins if historical agent rows
  share the id).
- **`live`:** true if that session’s PTY is in the daemon map **and** the child
  process is still alive (0.40.79 — no phantom `live:true` after restart/kill).
- **Adoption:** key on **`sessionId`**, not `sessionId`+`agentName` (agentName
  can rotate across respawns under the same handle).

---

## 5.2 `POST /v1/w/<ws>/host-sessions/<id>/kill` (force-stop)

**Integrator spend-cap / deliberate teardown** (0.40.79): force-stop a live
host-session PTY without deleting the cap file or revoking JWTs.

```http
POST /v1/w/sales/host-sessions/a1b2c3d4-…/kill
Authorization: Bearer k2sk_…
```

Empty body is OK. Authz matches message-live: `host_sessions` cap + workspace
grant + session owner (`owner_of(sessionId) == principal`). Unknown / unowned /
wrong-ws / canonical → **uniform 404** (no existence oracle).

| Situation | Status | Body |
|-----------|--------|------|
| Owned + live | **200** | `{"sessionId","killed":true}` |
| Owned + not live | **200** | `{"sessionId","killed":false,"reason":"not_live"}` |
| Unknown / other principal / ungranted / canonical | **404** | `{"error":"no such workspace"}` |

- Teardown is **force** (map unregister + kill); quota releases via the
  child-exit observer — do not double-release.
- Cap file under `.k2/caps/<sessionId>.json` and prior JWTs remain until natural
  `exp` / app-local jti revoke (same as natural reaper death).
- After `killed:true`, message-live / live-resume see a dead cell (404 /
  dead-resume path).

---

## 6. Revocation

| What | Who |
|------|-----|
| Capability JWT `jti` | **App-local** revoke set (Scout). Prefer single-valid: revoke previous jtis when remint returns new ones. |
| K2 API key | `k2 api-key revoke <id>` (owner-tier only) |

---

## 7. Explicitly deferred (not in 0.40.76 integrator surface)

OPS UI / mint-time `spawn_cap` product · rich queue position/cancel · sandbox-family envelope · full signing-key rotation runbook · busy/stuck API signal · `k2 cap` CLI.
