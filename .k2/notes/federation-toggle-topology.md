# Federation toggle topology — audit + consolidation design

Owner question: *"Enable Federation" (K2 Connect page) and "Let remote users
message agents" (General page) look redundant — do we need both? Simplify how
you enable federation so the toggles/checkboxes make sense.*

Audited at `e44595f` (local main tip). Every claim below is file:line-verified.

## 1. Flag inventory (end to end)

### A. `federation_enabled` — "Enable federation (cross-server agents)"
- **Setting**: `AppSettings.federation_enabled`, default OFF —
  `crates/k2-core/src/app_settings.rs:237` (default at `:444`).
- **Writers**:
  - K2 Connect settings page checkbox —
    `src/renderer/components/Settings/sections/K2ConnectSection.tsx:1063-1086`
    (hidden for confirmed Members; host-aware write via
    `src/renderer/stores/settings.ts:382` `setFederationEnabled`).
  - `k2 fed enable|disable|status` — `cli/k2:7038-7053` (POSTs
    `/cli/settings/update {"federationEnabled": …}`; headless path).
- **Env override**: `K2_FEDERATION=1|true|yes|on` force-ON regardless of the
  setting — `crates/k2-core/src/federation/mod.rs:80-87`.
- **Effective check**: `federation::enabled()` = env OR the persisted flag,
  mirrored into a process atomic at boot (`crates/k2-daemon/src/main.rs:203`)
  and on every settings update/reset
  (`crates/k2-daemon/src/settings_routes.rs:66,87`).
- **What it gates**: the ENTIRE `/cli/federation/*` surface — the dispatcher
  404s every path when off (`crates/k2-daemon/src/routes/dispatcher.rs:2978-2989`).
  That is: `pair/request`, `pair/confirm`, `inbound`, `send`, `roster`,
  `pubkey`, `peers`, `peer-roster`. So ONE switch covers pairing (both
  directions), outbound send, inbound receive, peer-facing roster visibility,
  and the renderer's peer-list reads. It gates nothing else.

### B. `allow_remote_instruct` (app-level) — "Let remote users message agents"
- **Setting**: `AppSettings.allow_remote_instruct`, default OFF —
  `crates/k2-core/src/app_settings.rs:230` (default at `:443`).
- **Writer**: General settings page toggle —
  `src/renderer/components/Settings/sections/GeneralSection.tsx:437-483`
  (host-aware write via `src/renderer/stores/settings.ts:368`
  `setAllowRemoteInstruct`). No CLI verb; no env override. NOTE: the row has
  `data-settings-id="general.allow-remote-instruct"` but NO entry in
  `GENERAL_MANIFEST` (`GeneralSection.tsx:41-57`) — it is unsearchable.

### C. `projects.allow_remote_instruct` (per-workspace) — "Let remote users message this workspace"
- **Setting**: per-project DB column, default 0 —
  `crates/k2-core/src/workspace/settings.rs` (`update_project_setting` /
  `get_allow_remote_instruct`).
- **Writers**: Workspace panel toggle
  (`src/renderer/components/Settings/sections/ProjectsSection.tsx:1778-1841`)
  via `GET /cli/remote-instruct?project=…&enable=…`
  (`crates/k2-daemon/src/misc_routes.rs:150-167`).

### B∨C. The effective consent gate
`remote_instruct_allowed_for_path(path)` = app-level B **OR** per-workspace C
(`crates/k2-core/src/workspace/settings.rs:242-249`; app-level ON opts in ALL
workspaces, back-compat). **Three distinct consumers:**
1. **Federation inbound delivery** — after the envelope passes the full
   security gate (verify sig vs pinned key → `require_peer(fp,"inbound")` →
   replay/skew/ttl/loop/sanitize), the recipient workspace's consent decides
   deliver-to-canonical-chat vs DECLINE (`delivered:false, mode:"declined"`)
   — `crates/k2-daemon/src/federation_routes.rs:285-328`. This is the
   0.40.20 "receive-trust refinement".
2. **K2 Connect connect-user composer** — `/cli/terminal/send-message`:
   owner token always allowed; a connect-user (role ≥ Member) is allowed only
   when this gate passes for the session's workspace
   (`crates/k2-daemon/src/routes/dispatcher.rs:44-65` →
   `authorize_send_message`, `crates/k2-daemon/src/routes/http.rs:609-610`).
   Renderer-hide mirror: `src/renderer/components/Terminal/terminalCompose.ts:71`.
3. **Sandbox v1 session message route** —
   `crates/k2-daemon/src/v1_ws_message.rs:102` (same decline shape).

### D. Other federation gates (for the flow map)
- **Peer trust + capabilities**: `peer.trust == Trusted` + per-request
  capability (`inbound`, `roster`) via fail-closed `require_peer` —
  `crates/k2-core/src/federation/peers.rs`; checked in `ingress::ingest`
  (inbound), `handle_send`/`handle_peer_roster`
  (`federation_routes.rs:394-399,684-689`), `verify_roster_request`.
- **GAP#3 connection gate** (agent-initiated outbound only): send allowed only
  if the SOURCE workspace has `<agent>@<host>` in `k2 connections` —
  `federation_routes.rs:401-420`. Owner-remote `k2 talk` never sets
  `from_workspace` → skips this gate by design.
- **Owner-or-admin management gate**: `pair/confirm`, `send`, `pubkey`,
  `peers`, `peer-roster` (`dispatcher.rs:3002-3130`). `pair/request` is
  deliberately UNAUTH (creates only Pending); `inbound` is authenticated by
  the signed envelope (DECISION-2), `roster` by a signed challenge.
- **Test-only**: `K2_FEDERATION_INBOUND_BASE` dial override
  (`federation_routes.rs:542-549`).

## 2. Message-flow topology (where each gate sits)

```
pair/request (UNAUTH, A gates route)      → Pending peer row
pair/confirm (A + owner-or-admin + SAS)   → Trusted + caps        [peers.json]
send  (A + owner-or-admin token)
  └ peer.trust==Trusted                   (always)
  └ is_remote_connection(src, agent@host) (only when from_workspace present)
  └ seal(sign) → durable outbox enqueue → dial https://<sub>.k2.dev
relay (K2 Connect: ciphertext passthrough — no gate)
inbound (A gates route; NO token)
  └ ingress::ingest: verify sig vs pinned key → require_peer(fp,"inbound")
    → replay → skew → ttl → loop → sanitize   (fail-closed, pre-delivery)
  └ CONSENT: remote_instruct_allowed_for_path(recipient ws)  ← the B∨C gate
       ON  → workspace_msg::deliver_live (canonical chat, wake+inject)
       OFF → DECLINE (200, delivered:false) — surfaced to sender as
             status:"declined" (0.40.20 fix)
reply = the same send path in the other direction
roster (A gates route; signed challenge + require_peer(fp,"roster"))
```

## 3. Truth table — are the two toggles redundant?

A = `federation_enabled` (∨ `K2_FEDERATION`); B∨C = effective remote-instruct
consent for the recipient workspace.

| A | B∨C | Pair/roster/peers | Outbound send | Inbound federation msg | Connect-user composer |
|---|-----|-------------------|---------------|------------------------|-----------------------|
| off | off | 404 (dark) | 404 | 404 (route dark) | 403 denied |
| off | on  | 404 (dark) | 404 | 404 (route dark) | **ALLOWED** |
| on  | off | works | works | security gate passes, then **DECLINED** | 403 denied |
| on  | on  | works | works | delivered to canonical chat | ALLOWED |

**Verdict: NOT redundant.** Each toggle has an effect the other cannot produce
(row 2: B alone enables the connect-user composer with federation fully dark;
row 3: A alone enables pairing/outbound/roster with all inbound declined). No
row collapses into another. Removing either daemon-side gate would change
security behavior. The redundancy is an ILLUSION created by UI topology: the
switch that decides whether federation inbound actually lands lives on a
different settings page, under copy that only mentions K2 Connect composer
users and never mentions federation.

## 4. Findings (ranked)

**Over-engineering / paper cuts**
1. **Placement + copy (the actual bug behind the question).** Federation's
   inbound consent is `allow_remote_instruct`, but its toggle sits in General
   with copy exclusively about K2 Connect composer users
   (`GeneralSection.tsx:449-455`). An owner who enables federation and pairs
   peers still gets every inbound message declined until they flip an
   unrelated-looking toggle on another page. Fix: co-locate + rewrite copy
   (implemented below).
2. **App-level master silently overrides per-workspace OFF.** The OR at
   `workspace/settings.rs:242-249` means with B on, a workspace toggle shown
   OFF in ProjectsSection is effectively ON — misleading UI state. Fix:
   surface the override in the per-workspace row copy (implemented below).
   The OR semantics themselves are deliberate back-compat; keep.
3. **Unsearchable setting**: no `GENERAL_MANIFEST` entry for the
   remote-instruct row (fixed by the move — it gets a manifest entry in its
   new home).

**Under-engineering (flag; not built now)**
4. **SECURITY — `/cli/settings/update` is `token_ok`-gated, not
   owner-or-admin** (`dispatcher.rs:1931-1943`, `http.rs:121-135`): any live
   connect-user session INCLUDING Member can POST
   `{"federationEnabled":true}` or `{"allowRemoteInstruct":true}` directly.
   The K2ConnectSection comment ("the daemon also owner-gates the write",
   `K2ConnectSection.tsx:1061-1062`) is FALSE for this route — only the
   renderer hides the checkbox from members. Tightening the route (or
   splitting security-relevant keys behind `require_owner_or_admin`) CHANGES
   auth behavior → **flagged for owner, not implemented** per the brief.
5. **Dead outbox retry loop.** `handle_send` durably enqueues "for the retry
   loop" (`federation_routes.rs:483-486`) and the CLI tells users a queued
   message "will deliver when it reconnects" (`cli/k2:496`), but NO production
   code drains the outbox — `outbox::list_all`/`list_for_peer` have no
   non-test callers. A send to a briefly-unreachable peer is queued forever
   (silent message loss with a reassuring message). Needs a boot/interval
   drain task.
6. **Roster visibility has no consent granularity.** Once A is on and a peer
   is Trusted+`roster`, `build_local_roster` exposes ALL configured agents
   (`crates/k2-core/src/federation/roster.rs:79-97`) — including workspaces
   that will decline every message. Read-only, but matches the documented
   "no incoming consent/visibility surface" gap; per-peer/per-workspace
   exposure belongs with the future peer-list controls.
7. **Trust is grant-only** (documented gap): `PeerTrust::Blocked` and the
   `peers.rs` primitives exist, FederationOverview even renders a "blocked"
   badge, but no route/CLI/UI can revoke/unpair/block. Belongs on the peer
   list rows in FederationOverview.

## 5. Consolidated control model

### Option A — UI-only consolidation (CHOSEN)
Keep both daemon settings and every daemon-side gate exactly as-is. Create a
single **Remote access** group on the K2 Connect page:
- **Enable federation (cross-server agents)** — unchanged master
  (`federation_enabled`).
- **Let remote users message agents** — the same `allow_remote_instruct` key,
  MOVED from General to sit directly beneath the master, with copy that names
  BOTH audiences (K2 Connect users via the composer AND paired federation
  servers) and points at the per-workspace override.
- Per-workspace toggle stays in the Workspace panel (it's per-workspace
  consent, the seed of the future per-peer model) and gains an "overridden by
  the global setting" hint when B is on.
- Future per-peer trust/revoke controls land on the FederationOverview peer
  rows — the model leaves that slot open (nothing global to migrate later).
- Migration: identity — no key renamed, no default changed, so every existing
  flag combination behaves exactly as before the update (locked by a
  round-trip test over all four combinations).

### Option B — split `allow_remote_instruct` into two daemon keys (rejected)
`allow_connect_user_instruct` + `allow_federation_inbound`, migration maps the
old key to both. Finer control, but it ADDS a checkbox (opposite of the ask),
duplicates what per-peer capabilities will do better on the peer list, and
schema churn risks a silent behavior flip during migration.

**Why A:** the truth table proves neither daemon gate is redundant, so the
legitimate simplification is placement and language, not gate removal. A
changes zero security behavior and needs only an identity migration, so no
user's federation silently turns on or off across the update. It leaves
per-peer trust management with the peer list where the documented gaps say it
belongs, instead of minting another global checkbox we'd have to migrate away
from later.
