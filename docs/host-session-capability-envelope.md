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

## 2. Wire sample

### 2.1 Spawn (cold) with capabilities

```http
POST /v1/w/<workspace-slug>/host-sessions
Authorization: Bearer <k2sk_…>
Content-Type: application/json

{
  "prompt": "Task text only — no secrets.",
  "timeout_secs": 600,
  "capabilities": [
    {
      "kind": "http_callback",
      "audience": "https://your.app/api/v1/interviews/{resource_id}/phase1",
      "resource": "interview:ivw_abc",
      "actions": ["GET"]
    },
    {
      "kind": "http_callback",
      "audience": "https://your.app/api/v1/interviews/{resource_id}/results",
      "resource": "interview:ivw_abc",
      "actions": ["POST"]
    }
  ]
}
```

**Response (shape):** `sessionId`, `agentName`, `workspace`, `sandbox: "none"`, `stream.grid`, optional `capabilities: { staged, env, expires_at, jtis }`. Fresh spawn **omits** `resumed` (or treats as non-resume).

**Staging:**

- File (multi-turn SSOT): `{workspace}/.k2/caps/<sessionId>.json` — JSON array `[{ "actions": [...], "token": "<jwt>" }, …]`, mode **0600**.
- Env (turn-0 only): `K2_CAPABILITY_TOKEN` = same JSON string at process start. **Live resume cannot update env.**

### 2.2 Live resume / inject (same PTY)

```http
POST /v1/w/<workspace-slug>/host-sessions
Authorization: Bearer <k2sk_…>
Content-Type: application/json

{
  "session": "<sessionId>",
  "prompt": "Continue…",
  "timeout_secs": 600,
  "capabilities": [ /* same shape; re-send to remint */ ]
}
```

- **`resumed: true`** — same live PTY; **queue-exempt / cap-neutral**.
- If `capabilities[]` present → **fresh JWTs**, atomic overwrite of the cap file, **new `jtis`** in response.
- If `capabilities[]` omitted → **no remint** (always re-send on multi-turn envelope path).
- Prior jtis remain valid until **their** `exp`. For single-valid-token semantics, **revoke old jtis in your app** (Scout-local). K2 does **not** server-invalidate prior jtis on remint.

Message-live: `POST /v1/w/<ws>/host-sessions/<id>` with the same optional `prompt` / `capabilities` / `timeout_secs`.

### 2.3 Dead resume

Same body with `session` when the PTY is gone → **new live cell**, **`resumed: false`**, full spawn path + caps mint. Treat as lost turn-1 cell state (no silent continue).

### 2.4 JWKS + verify (concrete)

```http
GET /v1/jwks
Authorization: Bearer <k2sk_…>
```

(Requires API surface enabled; same auth as other `/v1/*` routes.)

**Response:** `{ "keys": [ { "kty":"EC", "crv":"P-256", "x", "y", "use":"sig", "alg":"ES256", "kid" } ] }`

**App MUST verify each capability JWT:**

| Check | Value |
|--------|--------|
| Signature | ES256 against JWKS `kid` |
| `alg` | `ES256` |
| `iss` | `k2-host-sessions` (constant) |
| `aud` | Exact audience URL for this endpoint (after `{resource_id}` resolution) |
| `sub` | Host-session id (`sessionId`) if you track it |
| `resource` | Equals the interview/plan id implied by your URL plan |
| HTTP method | ∈ JWT `actions` |
| `exp` | Not expired |
| `jti` | Not in your **local revoke set** |

**Signing key (pilot):** private key at **`~/.k2/capability-signing.pem`** (mode 0600) on the **daemon host**, generated on first use. **STATIC for the pilot — no rotation procedure yet.** Public material only via `GET /v1/jwks`. Full rotation runbook = post-pilot ops debt.

### 2.5 Agent obligation

At the **start of each turn**, re-read `.k2/caps/<sessionId>.json` (or `K2_CAPABILITY_TOKEN` only if still on first process start). **Do not cache turn-1’s JWT** across turns.

---

## 3. Reliability

### 3.1 `timeout_secs`

- Clamp 30…86400; **default 180** if omitted (too short for multi-turn silent gen).
- Integrator: `timeout_secs >= max_inter_shot_gap + turn_2_budget`.
- **Scout multi-turn: 600** (and client poll ≥ 600).

### 3.2 S9 work-completion reaper

- Inject / register → **Working** (not idle-reaped).
- Non-final `k2 respond` → stay Working.
- `k2 respond --final` → short **grace** then may reap.
- **Hard wall** = `timeout_secs` from register (never-final safety).

### 3.3 Inject / auto-retry (no model cancel)

1. Per-session **injection lock** serializes pastes.  
2. Concurrent inject: wait or fail (**`pty_stalled`** / failed deliver) — not silent drop.  
3. **No** cancel of in-flight LLM generation.  
4. Retry into live-but-stuck may leave **two generations** in one cell.  
5. Write safety = app **`turn_id` idempotency** (+ fences).  

Live resume does **not** take a new concurrent slot.

---

## 4. Concurrent ceilings + stable 429 codes

Defaults (env-overridable): **principal 64** / **workspace 15** / **global 512**. Queue wait default **30s** then:

| `code` | Meaning |
|--------|---------|
| `concurrent-cell-cap` | Per-API-key principal full |
| `workspace-cell-cap` | This workspace full |
| `cell-capacity` | Daemon global full |
| `spawn-queue-timeout` | Waited full window, still full |

Map all four to an **honest retry** UX — never an infinite spinner.

---

## 5. Revocation

| What | Who |
|------|-----|
| Capability JWT `jti` | **Scout-local** revoke set at verify time |
| K2 API key | `k2 api-key revoke <id>` (owner-tier; not the Scout API key) |

---

## 6. Explicitly deferred (not in 0.40.76 integrator surface)

OPS UI / mint-time `spawn_cap` product · rich queue position/cancel · sandbox-family envelope · full signing-key rotation runbook · busy/stuck API signal · `k2 cap` CLI.
