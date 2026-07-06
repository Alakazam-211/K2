# PRD — /v1 API Completion: Host Sessions, Gates & Tier Enforcement

**Status:** APPROVED scope (Rosson 2026-07-06, "fix up the remaining API routes").
**Research SSOT:** `.k2/notes/k2-cloud-hosted-servers-research.md` §D (route
inventory + exact reuse points, file:line verified 2026-07-05).
**Companion PRD:** `.k2/prds/prd-k2-cloud-hosted-servers-v1.md` (Phase 2 = this).
**Related:** `.k2/prds/prd-sandbox-p3-api-spec.md` (the shipped sandbox API),
`.k2/prds/prd-sandbox-addendum-hosted-sessions.md` (earlier hosted-sessions
sketch — superseded by this where they conflict).

---

## 1. Summary

The `/v1` machine API today is sandbox-first: spawning is microVM-only
(`POST /v1/sandboxes`, `/v1/w/<ws>/sessions` — hard-409 on hosts that can't
sandbox, `v1_sandboxes.rs:40-43,628-631`), with exactly one non-sandbox
execution path (`POST /v1/w/<ws>/message`, `v1_ws_message.rs:68`). This PRD
completes the surface so a **sandbox-less host** (K2 Cloud Standard, shared
VPS, Raspberry Pi, any Mac) is a first-class API citizen:

- **F1** Non-sandboxed **host sessions**: spawn/list/message/read a real
  session in a granted workspace, no microVM.
- **F2** Host-side **response read-back** (the F2-message loop without the
  per-cell UDS).
- **F3** **Gate split + capability reporting** (`K2_SANDBOX_API` currently
  means "the whole /v1 surface exists"; no boot-status capability flag).
- **F4** **Owner-role API-key management** (hosted customers never hold the
  daemon token).
- **F5** **Pro-domain API enforcement** (the pricing page now promises
  "API calls to your server" as a Pro-subdomain feature — nothing enforces it).
- **F6** Documentation/consistency debt (stream-token mode default, uniform
  404 discipline, OpenAPI).

Non-goals: multi-tenant untrusted execution on non-sandbox hosts (the
microVM tier remains the only untrusted boundary); federation-peer API
access; usage metering/billing (future).

## 2. Standing invariants (do not violate)

- **Never silently unsandboxed**: `/v1/sandboxes` and workspace sandbox
  spawns keep their 409 on `can_sandbox()==false`. Host sessions are a
  DIFFERENT, honestly-labeled door — responses carry `"sandbox":"none"`.
  Rationale + precedent: `v1_sandboxes.rs:10-14`, `prd-sandbox-p3-api-spec.md:10-12`.
- **Identity from the token, never the body** (`authorize_send_message`
  precedent, `routes/http.rs:614`); `[from]` is attribution, not authz.
- **Uniform 404, no oracle**: unknown workspace, ungranted workspace, and
  unknown session are indistinguishable (`v1_sandboxes.rs:271-277,373-390`).
- **Caller argv/env are NEVER executed**: policy resolvers mint the command.
- **POST-only guards** on every mutating route (`if !is_post { 405 }`,
  house rule) + POST allowlist entries near `dispatcher.rs:615/661`.
- Wire shapes, once shipped, are frozen by integration tests.

## 3. F1 — Host sessions (non-sandboxed spawn)

### Routes (sibling family, NOT under /v1/sandboxes)
```
POST /v1/w/<ws>/host-sessions              -> spawn (or resume with {"session": id})
GET  /v1/w/<ws>/host-sessions              -> list
POST /v1/w/<ws>/host-sessions/<id>         -> message-live (inject into PTY)
GET  /v1/w/<ws>/host-sessions/<id>/messages?since=<seq>
```
Request (spawn): `{prompt?, cols?, rows?, timeout_secs?}` — same hint
semantics as sandboxes (`policy.rs:36-48`); `timeout_secs` clamps 30..86400,
default 180 (`sandbox_reaper.rs`).
Response (spawn): `{sessionId, agentName, workspace, sandbox: "none",
stream: {grid: "/cli/sessions/grid?session=…&token=<stream_tok>"}}`.

### Security design — the passthrough policy resolver (the ONE new
security-critical piece)
New `v1_host_sessions/policy.rs` mirroring `v1_sandboxes/policy.rs:110-169`:
- **cwd pinned to the granted workspace's registered path** (resolve via
  `resolve_authorized_workspace` → `resolve_workspace_slug`,
  `v1_sandboxes.rs:373/321`) — never $HOME, never a caller-supplied path.
- **Command host-minted**: the workspace's configured agent command (the
  de-generalization seam) with its resume/session-id conventions; default
  `claude --session-id <uuid>`. NOTE: unlike cells, do NOT force
  `--dangerously-skip-permissions` — on the host, claude's own permission
  prompts ARE a safety layer; make skip-permissions an explicit per-workspace
  owner opt-in setting (default OFF).
- **Caller env/args dropped**; Anthropic key staged from the API principal's
  stored key (api_keys 0058 `anthropic_api_key`) exactly as cells do.
- **Agent name minted** `api-<principal>-<uuid>` → rides SessionAdded with a
  distinguishing label so these surface as marked tabs (orange-tab pattern;
  label value: `"host"` vs cells' `"microvm"` — renderer treats any
  non-null backend as API-launched).
- **Canonical off-limits guard kept** (`session_is_canonical`,
  `v1_sandboxes.rs:402`): host-sessions never claim the workspace's pinned
  canonical chat; the canonical agent remains reachable only via
  `/v1/w/<ws>/message` with its consent + busy gates.
- **Quota + reaper reused as-is**: `sandbox_quota::try_acquire(principal)`
  (429 at cap) and `sandbox_reaper::{register,stamp}` are principal/session
  keyed, not microVM-specific. Rename in a follow-up only if free.

### Gating
- Requires the host-sessions gate ON (F3) AND an authorized principal
  (`v1_principal`, `http.rs:555-567`) AND per-key workspace grant
  (`authorizes_workspace`, `api_keys.rs:100`, fail-closed NULL).
- Available on EVERY host including sandbox-capable ones (Dedicated gets both
  doors). `can_sandbox()` is irrelevant to this family.

### Spawn plumbing (all existing)
`v2_spawn::spawn_session` (`v2_spawn.rs:512`) with `sandbox: None`
(Passthrough — the internal-degrade path, now deliberate); registered in
`v2_session_map`; live-inject via `lookup_by_session_id(...).write(prompt+\r)`
(pattern `v1_sandboxes.rs:477-493`); stream token minted per session
(`stream_token.rs:100`, grid WS already accepts it, `dispatcher.rs:945-951`).
Grid-WS note: stream-token connections are claimer-capable but default to
viewer MODE — a client driving the PTY over grid-WS must send
`{"action":"set_mode","mode":"claimer"}` first (0.40.27 S5 deviation). Either
flip the StreamToken default to claimer (one line, `sessions_grid_ws.rs`) or
document the handshake — DECIDE at build; recommendation: flip it, the token
is per-session and single-purpose.

## 4. F2 — Host response read-back

Cells feed `sandbox_responses::append` via the per-cell UDS `/cli/respond`
(`cell_server.rs:536-554`); host sessions have no cell UDS. Fix: the host
loopback HTTP path. `k2 respond` on the host already reaches the daemon
(hook env is injected into spawned children via `hook_config`); wire
`/cli/respond` arriving over loopback WITH a host-session's session identity
to `sandbox_responses::{record_owner,append}` so
`GET .../host-sessions/<id>/messages` reuses `handle_messages` semantics
(`v1_sandboxes.rs:185`: since-cursor, capped ring, uniform-404 on
owner-mismatch). Scope tokens per `K2_HOOK_SCOPED` so a session can only
append to itself. ACCEPT: the in-session agent runs `k2 respond --final "…"`
and the API caller sees it at `latest_seq`.

## 5. F3 — Gate split + capability reporting

Today: `K2_SANDBOX_API` OFF → ALL `/v1/*` 404 (`dispatcher.rs:3231-3252`);
`misc_routes::sandbox_api_enabled` (`misc_routes.rs:1101-1103`).
- New env `K2_API` = "the /v1 surface exists" (auth, ping, message API,
  host-sessions). `K2_SANDBOX_API` narrows to the sandbox families only.
  Back-compat: `K2_SANDBOX_API=1` implies `K2_API=1` (existing Dedicated
  units keep working); log a deprecation line.
- `/boot-status` gains `api: {enabled, hostSessions, sandboxes: "microvm"|"none"}`
  (pairs with Cloud PRD S3). `/v1/ping` echoes the same capability object.
- Standard-tier image ships `K2_API=1` only; Dedicated ships both.

## 6. F4 — Owner-role API-key management

`/cli/api-keys/{create,revoke,list}` are owner-TOKEN-only
(`dispatcher.rs:3202-3224`; `http.rs:498-500` keeps API keys from managing
keys — that stays). Hosted customers only ever hold an Owner-ROLE connect
session. Change the gate to owner-token OR Owner-role session (the
`can_change_roles`-style check used by `/cli/users/set-role`,
`dispatcher.rs:1743-1760`). Admin-role does NOT get key management. Audit
trail: key create/revoke events include the acting identity.

## 7. F5 — Pro-domain API enforcement (make the pricing page true)

k2.dev/pricing now sells `/v1` access as a **Pro-subdomain** feature; nothing
enforces it (any routed request reaching the daemon is served if gates are on).
Enforcement point options:
- (a) **Relay control plane** (recommended): frps vhost routing already knows
  the authenticated user + tier (`subdomains.tier`, synced rows). For
  E2E-passthrough tunnels the relay can't inspect paths (ciphertext) — so
  relay-level enforcement can't see `/v1` specifically. ⇒
- (b) **Daemon-side tier hint** (recommended concrete design): the daemon
  knows its own subdomain binding (`tunnel.json`); extend the tunnel
  config/lease flow to carry `tier` (control plane includes it in the
  RLS-readable row; daemon caches it). The `/v1` dispatcher arm rejects with
  403 `{"error":"api_requires_pro_subdomain"}` when the request arrived via
  the tunnel TLS listener AND cached tier != pro. Loopback/LAN callers (the
  box's own automations) are never tier-gated. K2 Cloud servers always have
  a pro subdomain, so this only bites self-hosted Single-domain users —
  exactly the upsell the pricing page describes.
- Fail-open on unknown tier (missing/stale cache) for V1 + log — never brick
  a paying customer on a sync hiccup; tighten later.

## 8. F6 — Consistency + docs debt

- OpenAPI 3.1 spec authored for the FULL `/v1` surface (sandboxes, ws
  sessions, host-sessions, message, ping) — none exists anywhere; seeds:
  `runbook-self-host-sandbox-server.md:181-211` curl flows +
  `prd-sandbox-p3-api-spec.md`. Feeds website W3 `/docs`.
- Document (or remove via the F1 decision) the stream-token `set_mode`
  handshake.
- `k2` CLI verbs for the new family (`k2 api sessions …`) — OPTIONAL V1,
  the raw curl is the product; decide with W3.
- Keep `sandbox_responses`/`sandbox_quota`/`sandbox_reaper` module names
  (shared by both families now) — rename only in a dedicated no-behavior
  commit if ever.

## 9. Test plan (daemon integration suites, mirror the sandbox suites)

- `host_sessions_integration.rs`: spawn happy-path (`sandbox:"none"`,
  cwd == workspace path, minted command, dropped caller env), 404-uniformity
  (unknown ws / ungranted ws / unknown id), quota 429, reaper reap at
  timeout_secs, canonical-guard, message-live inject, messages since-cursor.
- `host_respond_integration.rs`: in-session `k2 respond` → API read-back;
  scoped-token cannot append cross-session.
- Gate matrix: `K2_API` off → all 404; on without `K2_SANDBOX_API` →
  host-sessions 200 + `/v1/sandboxes` 404/409 per invariant; boot-status
  capability object correct in all 4 combinations.
- F4: Owner-role session manages keys; Admin-role 403; API key still cannot.
- F5: simulated tunnel-listener request with tier=single → 403; loopback →
  always allowed; unknown tier → allowed + logged.
- House rules: no swallowed asserts; POST-only guards tested per route.

## 10. Slices (build order)

1. **S1 F3 gate split + boot-status capability** (small, unblocks images).
2. **S2 F1 passthrough resolver + spawn/list routes** (the core).
3. **S3 F1 message-live + F2 read-back** (completes the loop).
4. **S4 F4 owner-role key management.**
5. **S5 F5 tier enforcement** (needs the tunnel-config tier plumb).
6. **S6 F6 OpenAPI + docs handoff to website W3.**

Slices 1–4 are pure-daemon, testable headless (feedback_daemon_first);
worktree subagents → cherry-pick per house convention.

## 11. Open decisions

1. Stream-token grid default: flip to claimer (recommended) vs document
   `set_mode` handshake.
2. `--dangerously-skip-permissions` on host sessions: per-workspace owner
   opt-in default OFF (recommended) vs match-cells always-on.
3. Route naming: `host-sessions` (recommended, explicit) vs
   `sessions?sandbox=false`.
4. F5 unknown-tier posture after V1: keep fail-open or flip to fail-closed
   once lease/tier sync is proven reliable.
