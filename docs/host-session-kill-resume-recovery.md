# Host-session kill → dead-resume recovery — integrator runbook

**Audience:** apps that recover stuck / never-born host-sessions (e.g. Scout).  
**Companion:** [`host-session-capability-envelope.md`](./host-session-capability-envelope.md) (caps, live/dead resume, kill).  
**Status PRD:** `.k2/prds/prd-v1-host-session-status-v1.md` · addendum `.k2/prds/prd-caps-recovery-consensus-addendum-v1.md`  
**Adopted:** Julie / Scout 2026-08-10 (federated) — decision tree + deliberate-donts are normative scar tissue for integrators.

This note is the **operational flow** for kill → re-exec under the **same** `sessionId`. It is not a substitute for product lifecycle (`k2 respond --final` / `k2 done`).

---

## 1. Identity model (read first)

| Token | Who owns | Stable across kill→resume? |
|-------|----------|----------------------------|
| **K2 `sessionId`** | Integrator registry + durable `api-%` index | **YES** — always re-address this handle |
| Body field `"session"` | Same string as `sessionId` | Required on every resume POST |
| Provider conversation file | Daemon / ProviderResume (`--resume <sid>` / premint) | Host splices; you do **not** pass a separate provider id |
| Cap JWTs / jtis | You re-send `capabilities[]` → remint | New jtis; revoke old **locally** |

**Do not mint a new `sessionId` to “recover.”** Dead-resume keeps the same id.  
`resumed: false` = new PTY under the **same** handle.  
`resumed: true` = live inject into an already-running PTY.

| Path | Durable session index |
|------|------------------------|
| `POST …/kill` (force-stop) | **Kept** (`resumable: true`) so dead-resume works |
| Grace after `--final` / `k2 done` | **Cleared** — different path; not this runbook |

---

## 2. Decision tree — is it safe to kill?

**Never kill on lock age / wall-clock alone.**

### A. Product-owned stop (always OK if you own the space)

Interview/space done, spend-capped, or user-cancelled → kill is deliberate spend-stop. Proceed.

### B. Evidence-of-absence recovery candidate

Only kill-for-restart when host evidence says **no agent work is underway**:

| Signal | Safe to kill for recovery? | Why |
|--------|----------------------------|-----|
| **status** `phase=never_started` + `live=false` + `started=false` | **Yes** (when status GET is fleet-available) | False never-born discriminator |
| **list** `live=false` **and** messages `latest_seq` flat at 0 for a poll window | **No by default** — see §2.1 | seq=0 is also mid-write; weak alone |
| **status** `phase=working` or `live=true` | **No** | Mid-work / mid-boot |
| **status** `phase=grace` | Prefer wait-out grace (~10s) then recheck; kill only if stuck past grace+ε | Intentional post-`--final` window |
| **status** `phase=finished` | No restart needed for “born”; kill only for hygiene | Already dead |
| Lock age / “generating for N min” alone | **Never** | Overlaps healthy long turns |
| `latest_seq == 0` alone while live or unknown | **No** | Mid-write / thinking |

### 2.1 Integrator strictness (Scout ack 2026-08-10)

Under an **evidence-of-absence** constraint set, the interim weak row
(`live=false` + flat `latest_seq` **without** independent transcript / absence
proof) is treated as **NO** unless the integrator’s own absence check confirms.

That is **stricter than the host-default “maybe”** and costs only patience — preferred for Scout recovery. K2 status GET (`never_started` / `started=false`) is the primary host-side replacement for that weak row when fleet-shipped.

---

## 3. Ordered flow (kill → dead-resume)

Assume: `BASE`, `KEY` (`k2sk_…`), `WS` slug, `SID` from spawn registry.

### 0. Preconditions

- Recorded `{ plan/space → sessionId, workspace }` from spawn 2xx.  
- Same API key (or Owner) that may observe/kill that row.  
- Caps grammar ready to re-send (always remint on multi-turn).

### 1. Snapshot before kill

```http
GET /v1/w/{ws}/host-sessions
GET /v1/w/{ws}/host-sessions/{sessionId}          # when status shipped
GET /v1/w/{ws}/host-sessions/{sessionId}/messages?since=0
```

Note `live`, `phase` / `started` (if status), baseline `latest_seq`.

**Gate:** recovery-kill only if §2 says safe. Else abort.

### 2. Kill

```http
POST /v1/w/{ws}/host-sessions/{sessionId}/kill
Authorization: Bearer k2sk_…
```

Empty body OK.

| Response | Meaning |
|----------|---------|
| `200` `killed: true` | Was live; now force-stopped; index kept |
| `200` `killed: false`, `reason: not_live` | Already dead — **idempotent OK** |
| `404` | Wrong principal / ws / id — **stop** (uniform authz; do not invent) |

### 3. Confirm dead (short poll — not lock-age)

Poll ~5–15s until list (or status) shows `live == false`.  
Optional: kill again → `not_live`.  
Do **not** require a new `sessionId`.

### 4. Dead-resume (same identity)

```http
POST /v1/w/{ws}/host-sessions
Authorization: Bearer k2sk_…
Content-Type: application/json

{
  "session": "<SAME sessionId>",
  "prompt": "<next turn instructions>",
  "timeout_secs": 600,
  "capabilities": [ /* same shape as spawn — ALWAYS re-send */ ]
}
```

What K2 does:

- Same `sessionId` in the response (stable handle).  
- **`resumed: false`** + new PTY + launch-param prompt on cold/dead path.  
- ProviderResume splices Big-7 resume grammar — conversation continuity is host-side.  
- Caps reminted to `.k2/caps/<sessionId>.json`; **new jtis** → revoke prior jtis in your set.  
- Principal ownership re-asserted on host-session maps for this key.

### 5. Integrator ownership re-register

On 2xx:

1. Assert `response.sessionId === SID` (abort if not — do not silently rekey).  
2. Assert `workspace` slug matches registry.  
3. Upsert plan/space owner row: still SID; note `resumed: false`; new `agentName` / stream token if rotated.  
4. Replace stored jtis with `response.capabilities.jtis`; revoke old.  
5. **Do not** create a second plan row with a new sessionId for the same space.

### 6. Post-resume health (when we call it healthy)

Poll (backoff §4) until **one** of:

| Check | Healthy |
|-------|---------|
| **status** (preferred when fleet) | `live=true` **or** `phase=working` **or** `started=true` |
| list | row for SID with `live=true` |
| messages | `latest_seq` increases past pre-kill baseline **or** first respond after turn |
| **product fence ACK** | Integrator’s own write-back / fence verification for this turn (Scout: terminal healthy signal — ack 2026-08-10) |

**Not sufficient alone:** spawn HTTP 2xx (PTY accepted only). That was the never-born hole.

**Fail closed:** if after budget (e.g. 15–45s) still `live=false` and status `never_started` / `started=false`, treat as failed restart — do not leave the space “generating” forever.

### 7. Caps / write path after resume

```
if resumed == false:
  treat as new cell life — capabilities[] already re-sent on resume body
  wait for agent to re-read cap file before expecting callbacks
  Layer B still: sub==SID, resource bind, method∈actions, jti not revoked
```

See envelope §2.5 and §3.3 Layer B.

---

## 4. Backoff / retry discipline

| Phase | Pattern |
|-------|---------|
| After kill confirm | 0.5s, 1s, 2s — cap ~15s for `live=false` |
| After resume spawn | 1s, 2s, 4s, 8s … cap ~30–45s for live / started / seq growth / fence ACK |
| Max resume attempts per space per incident | **1–2**, then fail-closed |
| Concurrency | **Single-flight per `sessionId`** — never parallel kill or dual dead-resume |
| `429` / queue / cell-cap | Exponential backoff; do **not** convert into killing other spaces |

Do not thrash kill↔spawn under concurrent cold-start load (worsens startup-hang class).

---

## 5. Minimal shell shape

```bash
# env: BASE KEY WS SID
auth() { curl -sS -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" "$@"; }

# 1) preflight
auth "$BASE/v1/w/$WS/host-sessions" | jq --arg s "$SID" '.sessions[]? | select(.sessionId==$s)'
# optional when fleet: auth "$BASE/v1/w/$WS/host-sessions/$SID"

# 2) kill (only if §2 gate passed)
auth -X POST "$BASE/v1/w/$WS/host-sessions/$SID/kill" -d '{}'

# 3) wait dead
for i in 1 2 3 4 5; do
  live=$(auth "$BASE/v1/w/$WS/host-sessions" | jq -r --arg s "$SID" \
    '.sessions[]? | select(.sessionId==$s) | .live')
  [ "$live" = "false" ] && break
  sleep 1
done

# 4) dead-resume same identity
auth -X POST "$BASE/v1/w/$WS/host-sessions" -d @- <<JSON
{
  "session": "$SID",
  "prompt": "…turn N…",
  "timeout_secs": 600,
  "capabilities": [ /* … */ ]
}
JSON
# assert: .sessionId == $SID && .resumed == false

# 5) health: live / status started|working / latest_seq bump / product fence ACK
```

---

## 6. Mapped to Scout-class constraints

| Constraint | How this flow respects it |
|------------|---------------------------|
| **Evidence-of-absence** | Kill-for-recovery only on `never_started` / independent absence — not seq=0 alone; Scout interim row = **NO** without own transcript-absence check (§2.1) |
| **Never seize on lock age** | No age threshold triggers kill; product stop is explicit; recovery uses host phase / live |
| **Respawn re-registers ownership** | Same SID in body; on 2xx upsert registry + jtis; refuse a different `sessionId` |

---

## 7. Deliberate don’ts (scar tissue)

- Kill all non-live list rows in bulk (authz + never-born confusion).  
- Use `/messages` **404** as liveness (uniform authz trap).  
- Expect a **new** `sessionId` on dead-resume.  
- Rely on `timeout_secs` as a Working hard wall (JWT / client budget only under S9).  
- Skip `capabilities[]` on resume (stale cap file).  
- Restart loops under concurrent cold-start load without single-flight.  
- Treat spawn **2xx** alone as “agent started.”  
- Kill `live=true` / `phase=working` for recovery.

---

## 8. Status GET note

When fleet ships:

```http
GET /v1/w/{ws}/host-sessions/{sessionId}
```

Fields (snake_case drain alignment): `live`, `started`, `phase`, **`latest_seq`**, `reaper`, `durable`, …

Product lock:

```text
started = live OR latest_seq > 0 OR provider_session_file_exists
```

Live + seq 0 + no transcript yet → `phase=working`, `started=true` — **never** `never_started`.

Until status is on your box: list `live` + your absence package (Scout: stricter interim — §2.1).

---

## 9. Related

| Doc | Role |
|-----|------|
| [`host-session-capability-envelope.md`](./host-session-capability-envelope.md) | Caps mint, live/dead resume, kill wire, Layer B |
| `.k2/prds/prd-v1-host-session-status-v1.md` | Status endpoint shape |
| `.k2/prds/prd-caps-recovery-consensus-addendum-v1.md` | Resource unbake + status field naming + Layer B SSOT |
| `.k2/prds/prd-v1-host-session-kill-v1.md` | Kill route |
