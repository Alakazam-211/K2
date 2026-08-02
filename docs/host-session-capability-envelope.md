# Host-session capability envelope — integrator note (S5)

**Version:** first adoptable cut / **0.40.76**  
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

Fresh spawn **omits** `resumed`.

### 2.2 Staging (file SSOT + re-send obligation)

| Channel | When | Notes |
|---------|------|--------|
| **File** `{workspace}/.k2/caps/<sessionId>.json` | Every mint / remint | Multi-turn **SSOT**. Mode **0600**. Atomic write: temp + `rename()`. |
| **Env** `K2_CAPABILITY_TOKEN` | Cold spawn only | Same JSON string as the file. **Live resume cannot update env.** |

**Re-send-caps-on-resume (required for multi-turn envelope):**  
On every multi-turn resume / continue, **ALWAYS re-send `capabilities[]`** so K2 remints and overwrites the cap file.  
**Omit `capabilities[]` ⇒ no remint ⇒ file may be stale.** Do not write with a stale file after a long gap or after `resumed: false` without remint.

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
    "token": "eyJhbGciOiJFUzI1NiIsImtpZCI6Ii4uLiJ9.eyJpc3MiOiJrMi1ob3N0LXNlc3Npb25zIiwiYXVkIjoiaHR0cHM6Ly9zY291dC5leGFtcGxlL2FwaS92MS9pbnRlcnZpZXdzL2l2d19hYmMvcGhhc2UxIiwic3ViIjoiYTFiMmMzZDQtLi4uIiwicmVzb3VyY2UiOiJpbnRlcnZpZXc6aXZ3X2FiYyIsImFjdGlvbnMiOlsiR0VUIl0sImV4cCI6MTc1NDE0MDAwMCwiaWF0IjoxNzU0MTM2NDAwLCJqdGkiOiJqdGktcmVhZC0wMDEifQ.…"
  },
  {
    "actions": ["POST"],
    "token": "eyJhbGciOiJFUzI1NiIsImtpZCI6Ii4uLiJ9.eyJpc3MiOiJrMi1ob3N0LXNlc3Npb25zIiwiYXVkIjoiaHR0cHM6Ly9zY291dC5leGFtcGxlL2FwaS92MS9pbnRlcnZpZXdzL2l2d19hYmMvcmVzdWx0cyIsInN1YiI6ImExYjJjM2Q0LS4uLiIsInJlc291cmNlIjoiaW50ZXJ2aWV3Oml2d19hYmMiLCJhY3Rpb25zIjpbIlBPU1QiXSwiZXhwIjoxNzU0MTQwMDAwLCJpYXQiOjE3NTQxMzY0MDAsImp0aSI6Imp0aS13cml0ZS0wMDIifQ.…"
  }
]
```

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

**Response includes** `resumed: true`, `live: true`, `delivered`, and (when caps sent) `capabilities: { staged, env, expires_at, jtis: [<new>, …] }`.

- Caps present → **fresh mint**, atomic file overwrite, **new jtis**.
- Caps omitted → **no remint**.
- Prior jtis remain cryptographically valid until **their** `exp`. For **single-valid** semantics, **add old jtis to your local revoke set** when you receive the new set (recommended for Scout). K2 does **not** server-invalidate old jtis.

Same optional fields on `POST /v1/w/<ws>/host-sessions/<id>` (message-live).

### 2.5 Dead resume (`resumed: false`) — **normative sequence**

When the cell is **dead**, the same POST body with `session` + **`capabilities[]` re-sent** does a **full re-spawn** (new live PTY), returns **`resumed: false`**, and **mints into the cap file** as part of that spawn.

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
Authorization: Bearer k2sk_…
```

Requires API surface enabled. Response:

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

### 4.2 S9 work-completion reaper

- Inject / register → **Working** (not idle-reaped).  
- Non-final `k2 respond` → stay Working.  
- `k2 respond --final` → short **grace** then may reap.  
- **Hard wall** = `timeout_secs` from register.

### 4.3 Inject / auto-retry (no model cancel)

1. Per-session **injection lock** serializes pastes.  
2. Concurrent inject: wait or fail (**`pty_stalled`** / failed deliver) — not silent drop.  
3. **No** cancel of in-flight LLM generation.  
4. Retry into live-but-stuck may leave **two generations** in one cell.  
5. Write safety = app **`turn_id` idempotency** (+ AUTHZ layer §3.3).  

Live resume is **queue-exempt / cap-neutral**.

---

## 5. Concurrent ceilings + stable 429 codes

Defaults (env-overridable): **principal 64** / **workspace 15** / **global 512**. Queue wait default **30s** then:

| `code` | Meaning |
|--------|---------|
| `concurrent-cell-cap` | Per-API-key principal full |
| `workspace-cell-cap` | This workspace full |
| `cell-capacity` | Daemon global full |
| `spawn-queue-timeout` | Waited full window, still full |

Map all four to an **honest retry** UX — never an infinite spinner.

---

## 6. Revocation

| What | Who |
|------|-----|
| Capability JWT `jti` | **App-local** revoke set (Scout). Prefer single-valid: revoke previous jtis when remint returns new ones. |
| K2 API key | `k2 api-key revoke <id>` (owner-tier only) |

---

## 7. Explicitly deferred (not in 0.40.76 integrator surface)

OPS UI / mint-time `spawn_cap` product · rich queue position/cancel · sandbox-family envelope · full signing-key rotation runbook · busy/stuck API signal · `k2 cap` CLI.
