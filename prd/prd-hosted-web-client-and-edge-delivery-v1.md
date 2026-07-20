# PRD — Hosted Web Client + Edge Delivery (v1 draft)

**Status:** draft for review · **Author:** Rosson + agent · **Date:** 2026-07-20
**One-liner:** Run the existing K2 thin client **in a browser** against any client's own daemon — served from a CDN edge (Cloudflare), versioned to match each daemon, reaching the box over the existing secure tunnel — so a user can drive their 24/7 server from any browser with **no install**.

> [!note] Placement: **public app-layer design** (secret-free: boxes by nickname,
> no IPs/keys). Lives under `prd/` in the K2 repo (public product PRDs). Demo VM lifecycle /
> control-plane detail stays in the sibling private PRD — not this document.

> [!info] Supersedes the "Try-Now Demo" framing of
> `prd-k2-cloud-vm-hosting-and-demo-v1.md` §Part 2. That PRD keeps the **VM
> lifecycle + demo guardrails**; this PRD owns **delivery, versioning, edge
> capacity, and the two access modes.** They cross-reference.

---

## 1. What this is (and the headline)
**The thin client is already a web app — this is a build variant, not a rewrite.**
The renderer is React 19 + TS, built by Vite to a static bundle; Tauri's webview
*is* a browser, and for **remote** hosts the data plane already speaks plain
`fetch`/`WebSocket` (`kessel/daemon-ws.ts`, no Tauri `invoke`). We shim
`@tauri-apps/api/*` via a demo/web Vite `resolve.alias` (see the sibling PRD
§2A for the shim + amputation detail). No WASM: WASM would only matter to run
the *daemon* in-browser, but the daemon lives on the box — which is the point.

**Two modes, one delivery pipeline:**
- **Persistent web client** — a real user, logged into **their own** server
  (`<sub>.app.k2.dev`), full session. The primary product.
- **Ephemeral demo** — a visitor on a throwaway VM with a scoped guest token
  (all VM lifecycle + guardrails in the sibling PRD §2C/2D).

Both use the identical edge-delivery + versioning machinery below; they differ
only in **auth + lifecycle**.

**Entry point (how a user finds their door):** the primary URL is
`https://<sub>.app.k2.dev` (bookmark it and go straight there). For anyone who
lands on the bare **`app.k2.dev`**, it's a **dumb, static, unauthenticated
router** — a single "enter your subdomain →" field that redirects to
`<sub>.app.k2.dev`. **No auth at this layer** (all auth happens at the daemon's
login once routed); no backend, no lookup service — the page is pure static JS,
fully CDN-cacheable, and the subdomain the user already bought *is* their
address. Auth lives entirely at `<sub>.app.k2.dev` (§2.3), never at `app.k2.dev`.

**Explicit non-goals (v1):**
- **Not** running the daemon in-browser (no WASM daemon — §1).
- **Not** replacing the desktop app; this is an additional access mode.
- **Not** OS-native features in the browser (Browser tab / local files /
  install-update / keychain / native window chrome — amputated, §7).
- **Not** a new auth system — reuses `connect-users`; only the *transport*
  (cookie vs query token) is new (§2.3).
- **Not** touching the E2E desktop tunnel path (§6.1).

---

## 2. Delivery architecture

### 2.1 Hostnames — one zone, two record behaviors
`k2.dev` is on **Cloudflare** (DNS + SSL) today. We use **one zone, two cloud
colors:**

| Hostname | Cloudflare | Purpose |
|---|---|---|
| `<sub>.k2.dev` (tunnel wildcard) | **grey-cloud (DNS-only)** | A → relay; TLS **passes through** to the daemon → **E2E desktop path unchanged** |
| `<sub>.app.k2.dev` (web wildcard) | **orange-cloud (proxied)** | Cloudflare CDN + TLS-terminate + DDoS-scrub → relay → tunnel → daemon |

`<sub>.app.k2.dev` (not `webportal.<sub>.k2.dev`) is chosen so a **single central
`*.app.k2.dev` cert + config** covers every client — **no per-server nested
record to publish.** Cost note: `*.app.k2.dev` is a *second-level* wildcard, not
covered by Universal SSL → needs **Cloudflare Advanced Certificate Manager**
(~$10/mo, one cert, fleet-wide).

### 2.2 The request flow
```
Browser → https://<sub>.app.k2.dev/
  │
  ▼
[Cloudflare POP]  (orange-cloud: TLS terminate • cache static • scrub DDoS)
  │   path routing:
  │     /  and  /app/<ver>/*   → CDN cache  (served here; box never sees it)
  │     /cli/*  /events  /boot-status → proxy to origin ↓
  ▼
[RELAY  k2e-01/02]  ← Cloudflare's ORIGIN is the relay, never the daemon
  │     maps <sub>.app.k2.dev → that client's tunnel (same as <sub>.k2.dev today)
  ▼
[frp secure tunnel]  ← the only way into a NAT'd box (no public/static URL)
  ▼
[client daemon]  → connect-users auth, sessions, workspaces
```

**Load sequence (what the user experiences):**
1. Browser → `<sub>.app.k2.dev` → Cloudflare serves the **tiny loader** (`/`).
2. Loader calls `/boot-status` (proxied to the daemon) → reads
   `webClientVersion`.
3. Loader pulls the **matching immutable** `app/<ver>/` bundle from CDN cache.
4. SPA mounts → **ConnectionGate** runs the protocol/`whoami`/`/boot-status`
   handshake → shows the **server login**.
5. User logs in via `/cli/auth/login` against the daemon's `connect-users`
   (the exact route that gates the desktop client too).

**Loader rules (get these right or the whole scheme jams):**
- **Cache split:** bundles are `Cache-Control: public, max-age=31536000,
  immutable`; the **loader is NOT immutable** — serve it `no-cache` (or ≤60s)
  so we can ship loader fixes. The loader is the one mutable byte-range in the
  system; keep it tiny and boring.
- **Validate `webClientVersion`** before use: strict semver regex + **enforce
  the support floor**. A compromised/buggy daemon must not be able to
  path-inject the bundle URL or pin users onto a known-vulnerable old bundle
  (downgrade attack). Out-of-range → loader shows "update your server" UX.
- **Offline state:** if `/boot-status` times out (box asleep/offline/tunnel
  down), the loader renders a friendly served-from-edge "server unreachable"
  page — the user's first-load experience must not be a spinner into a wall.
- **SPA deep links:** edge serves the loader for unknown non-`/cli` paths
  (SPA fallback routing), so `/settings` etc. survive refresh.
- **Debug override:** `?v=<ver>` query param (validated the same way) lets
  support force a bundle version.

**Same-origin throughout** (`<sub>.app.k2.dev` serves the SPA *and* proxies the
data plane) → **no CORS, no cross-origin token exposure.** `/events` is a
WebSocket; Cloudflare proxies WS fine — **but Cloudflare enforces ~100s
idle/read timeouts on proxied connections.** A quiet `/events` socket or a
long-poll `/cli` request dies silently at the edge. Therefore: **ping/keepalive
frames on `/events` well under the idle window** (the 0.40.48
connection-resilience reconnect logic then covers the rest), and no `/cli`
request may block longer than the read timeout (long operations poll or stream).

**Data plane always rides the tunnel** — a client daemon may be a NAT'd laptop
with no address (Adam's MacBook). Even public-IP client servers route through
the relay/tunnel for **uniform addressing** and to never expose a daemon
directly.

### 2.3 Browser auth transport (NEW daemon work — not free)
Today the daemon authenticates via **`?token=` query strings** (dispatcher
`extract_token(&query)`), which the desktop client can use safely. **A browser
cannot:** query-string credentials leak into browser history, edge/relay access
logs, and `Referer` headers. The web client therefore needs a real browser auth
mode on the daemon:

- **Login sets a cookie:** `/cli/auth/login` (web path) responds `Set-Cookie:
  k2_session=…; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age=<short>` —
  same `connect-users` session store, different transport.
- **Cookie accepted on the data plane:** `/cli/*` and the `/events` WS upgrade
  accept the session cookie as an auth source (in addition to existing token
  auth for CLI/desktop). Token-in-query stays for non-browser callers;
  **the web client never puts a credential in a URL.**
- **CSRF defense (cookies bring CSRF):** `SameSite=Strict` + require a custom
  header (e.g. `X-K2-Client: web`) on mutating `/cli/*` POSTs — a cross-site
  form can't set custom headers. Cheap and sufficient given same-origin.
- **Logout + revoke** route clears the cookie and kills the session server-side.
- **Edge rate-limit on `/cli/auth/login`** (Cloudflare rule) in front of the
  daemon's existing argon2 + lockout — brute-force dies at the edge, not on the
  box.

---

## 3. Design B — immutable versioned bundles + a loader

**Problem:** one edge SPA talks to *many* daemons at different versions (boxes
self-update on their own schedule). A single rolling bundle would force
lockstep and **break any client whose box hasn't updated yet.**

**Solution:** the edge is a **dumb, versioned CDN** — you never hand-deploy it
per release.
- Each `<sub>.app.k2.dev/` serves the **tiny short-cached loader** (§2.2 rules
  — the bundles are immutable; the loader deliberately is not).
- The loader reads the daemon's advertised **`webClientVersion`** from
  `/boot-status`, then loads the matching **content-hashed immutable** bundle
  `app/<ver>/main.[hash].js` from the CDN.
- **A front-end release = CI publishes a new `app/<ver>/` (additive; old
  versions stay).** No per-client, no per-box, no edge-config change. A box that
  self-updates its daemon automatically starts loading the newer SPA. **Zero
  skew, safe roll-forward/back.**
- This mirrors the desktop contract (renderer+daemon ship together, negotiate
  via `/boot-status`) — we're just moving bundle-selection to page load and
  serving the bytes from a CDN instead of the app binary.

**Retention cutoff (derived, not arbitrary):** keep web bundles back to the
**oldest daemon version still supported** (the `MIN_COMPATIBLE` floor). When a
daemon version drops below the floor (and the ConnectionGate force-updates it),
GC every web bundle below it. A CI/cron prune on the bucket. Bundles are tiny +
immutable, so this is attack-surface hygiene, not a cost problem.

---

## 4. Build vs buy — rent the front, own the back
| Layer | Build/Buy | Where it lives |
|---|---|---|
| Static SPA bundles + global cache | **Buy** | Cloudflare CDN + object storage (R2/S3). No server of ours. |
| DDoS scrub + TLS (web path) | **Buy** | Cloudflare. Multi-Tbps absorption we can't self-host. |
| Dynamic data plane (`/cli/*`,`/events`) | **Existing** | Relay `k2e-01/02` (only for dynamic; gets *lighter*). |
| Demo orchestrator (lease/mint/reap) | **Existing** | Control plane (`k2-connect`) or serverless. |
| Demo VMs | **Existing plan** | `k2-vmhost-01` (SYS-3). |

**No new servers for this feature.** The only servers we ever add are **relay
boxes**, gated on real **usage growth** (§5), which the CDN pushes further out.

**Not Cloudflare-locked.** The mechanism (edge-served, same-origin, versioned
bundles) works behind **any** edge/Caddy. Cloudflare is our **operational
choice** for k2.dev (already our DNS + SSL). A self-hoster serves the same web
client off their own edge — the open-core story is intact.

---

## 5. Edge capacity, DDoS & scaling
**Principle: keep the heavy/attackable load off the critical boxes.** The edge
boxes carry the two things we can least afford to lose — the **tunnel** (every
client's connectivity) and **DNS**. So:

- **Static + volumetric DDoS → Cloudflare**, never the relay. The relay's real
  growth driver is the **dynamic data plane** (per-client API/WS) — that's what
  §5's capacity rule watches.
- **Origin lock-down (hard invariant):** the relay's web path must **only accept
  Cloudflare** (firewall to Cloudflare IP ranges + **Authenticated Origin
  Pulls**). Otherwise an attacker who finds the relay IP bypasses the CDN and
  hits the relay raw.
- **Noisy-neighbor isolation:** a relay CPU/bandwidth spike must not starve DNS
  (and vice-versa). Growth path: **separate relay vs nameserver roles** and
  **anycast the DNS** (DNS is the textbook anycast workload).

### 5.1 Load distribution (GSLB)
- For **k2.dev** clients, the steering lever is **Cloudflare Load Balancing**
  (health + geo origin selection) — Cloudflare is authoritative for k2.dev.
- Our **PowerDNS nameserver edge** provides the equivalent GSLB for the
  **managed-DNS (customer BYO-domain)** product — a separate surface; don't
  conflate.
- DNS steering is **coarse** (TTL-bound): it steers **assignment + failover**,
  not per-request balancing (frp connections are sticky to their relay). We
  already have **US + EU** relay presence to make this geo-aware.

### 5.2 Capacity rule (N+1)
Keep each edge box under **(N−1)/N** utilization so **any one box can fail** and
survivors carry full load:
- **2 boxes → ≤50% each** (the current pair). Both sustained >50% at once →
  **provision a 3rd**.
- **3 boxes → ≤~66% each**, etc.

Refinements:
1. **Watch the binding constraint, not just CPU** — relays saturate on **NIC
   throughput / established connections / FDs**; DNS on **QPS (queries/sec)** —
   often before CPU moves.
2. **Sustained (p95 over a window), not spikes** — bursts are the CDN's problem.
3. **Test the failover reconnect storm** — when DNS flips, thousands of `frpc`
   reconnect at once; the survivor must eat steady load **+** the herd. Sizing
   N+1 on paper ≠ surviving the herd.
4. **Monitored in the fleet console;** alert on the N+1-margin breach.
   **Alert-and-confirm** before auto-ordering edge (we have the ordering
   scripts, but the blast radius is too high to fully automate at first).

---

## 6. Security invariants
1. **E2E desktop path unchanged** — `<sub>.k2.dev` stays grey-cloud passthrough.
   The web path is **edge-terminated by design** (a browser loading an
   edge-served SPA is not E2E; it's the hosted-convenience mode).
2. **Origin lock-down** — relay web path accepts Cloudflare only (§5).
3. **Browser token** — persistent web client uses **httpOnly, short-TTL, scoped
   session cookies** (never an owner/ambient token); **hardened CSP**
   (same-origin `connect-src`, no third-party). Ties to the scoped-identity
   model in `prd-k2-remote-session-v1.md`.
4. **Demo tokens** — scoped, short-TTL, non-owner guest tokens; **server-side**
   TTL/idle reapers (never client-side — GH#22 lesson); no LLM keys in the demo
   template.
5. **Same-origin data plane** — SPA + `/cli/*` under one host → no CORS
   allowlist, no cross-origin token.
6. **No credential ever in a URL on the web path** — cookie transport only
   (§2.3). Query-token auth remains for CLI/desktop callers.
7. **Owner-controllable web access** — `web_client_enabled` per-daemon setting
   (Layer-0 pattern from remote-session: coarse owner wall, checked first).
   When OFF the daemon rejects `.app`-Host data-plane requests with a teaching
   error. **Default ON** (the web client is a headline feature and the tunnel
   already exposes token-gated `/cli/*`; the toggle is for owners who want the
   browser door shut) — confirm default at review.

---

## 7. v1 slice (concrete work)

### 7.0 Ownership — who builds what
Split along the existing team seam:
- **Grok** — app + daemon + release/CI (builds K2 app/daemon features; owns
  `release.sh` and the release train).
- **Ops (K2SO workspace)** — edge + infra (Cloudflare, relay, certs, DNS,
  capacity). Also authored this PRD + owns the interface contract.

| Work group | Owner |
|---|---|
| Client build — web Vite variant, shim, loader | **Grok** |
| Daemon — cookie auth transport, `webClientVersion`, `web_client_enabled`, `.app` Host | **Grok** |
| Release/CI — `release.sh` publishes bundles + prune | **Grok** |
| Edge/infra — Cloudflare zone/cert/rules, relay routing + origin lock-down, LB | **Ops** |
| Bucket provisioning (R2/S3 + write creds for CI) | **Ops** (hands Grok the target) |

> [!important] **Interface contract — the seam. Build both halves in parallel against this; neither side reaches into the other.**
> 1. **Grok's CI writes** immutable `app/<ver>/…` to the **bucket** (Ops
>    provisions the bucket + gives CI write credentials + the URL base).
> 2. **Grok's daemon advertises** `webClientVersion` in `/boot-status` and
>    accepts **cookie auth** on `/cli/*` + the `/events` upgrade.
> 3. **Ops' edge serves** the loader + bundles from that bucket and proxies the
>    data plane to relay → tunnel → daemon.
>
> They meet only at three points: **the bucket**, **`/boot-status`**, and **the
> relay origin.** Lock those three and the halves integrate cleanly.

**Client build — Grok:**
- Web/demo Vite build variant + `@tauri-apps/api` shim (`invoke`→fetch,
  `listen`→one `/events` WS; ~50-line `daemon_events.rs` reimpl) — sibling PRD §2A.
- The **~20-line loader** (reads `/boot-status.webClientVersion`, loads
  `app/<ver>/`).

**Daemon — Grok (the real new surface — most of the daemon-side risk is here):**
- `/boot-status` advertises **`webClientVersion`** (it already reports the
  daemon version — this is that) — and is reachable **unauthenticated** (it
  already is; the loader needs it pre-login).
- **Cookie auth transport** (§2.3): cookie-setting login, cookie accepted on
  `/cli/*` + `/events` upgrade, CSRF header check, logout/revoke.
- **`web_client_enabled`** Layer-0 owner wall (§6.7).
- Accept the `<sub>.app.k2.dev` Host on handshake + data-plane routes.

**Release/CI — Grok:**
- `release.sh` also publishes `app/<ver>/` (immutable, content-hashed) to the
  bucket; prune job for the support-floor cutoff.

**Edge/infra — Ops:**
- Cloudflare: `*.app.k2.dev` orange-cloud + **ACM cert**; path rules (cache
  `/`,`/app/*`; proxy `/cli/*`,`/events`,`/boot-status`).
- Relay: route `<sub>.app.k2.dev` → that client's tunnel; **origin lock-down**
  (Cloudflare-only + Authenticated Origin Pulls).
- Cloudflare Load Balancing across relays (health/geo) — when the 2nd+ relay is
  live-serving web.

---

## 7.5 Phasing (de-risk in this order)
**Grok leads the critical path (1→3); Ops runs one prerequisite early and the
edge in parallel.** Phases 1–2 need **no Cloudflare** — Grok proves the entire
client↔daemon path over a throwaway laptop Caddy while Ops preps the edge, then
they cut over. Sequence:

1. **[Grok] Prove the client build** — web Vite variant + shim + loader, pointed
   at a dev daemon over its tunnel, **served from a laptop Caddy** (no Cloudflare
   yet). Kills the "does the SPA actually run remote-only" risk cheapest.
   **Local try:** [`docs/web-client-local-dev.md`](../docs/web-client-local-dev.md).
2. **[Grok] Daemon auth transport** — cookie login + cookie-accepted `/cli/*` +
   the `web_client_enabled` wall (§2.3, §6). The real new daemon surface; land +
   test it headless before edge work.
   - **[Ops, in parallel] Provision the bucket** (R2/S3) + CI write creds + URL
     base → hand to Grok. *This is Ops' one blocking prerequisite for phase 3.*
3. **[Grok] CI publishes versioned bundles** — `release.sh` → bucket; loader
   picks by `webClientVersion`. (Needs Ops' bucket from phase 2.)
4. **[Ops] Edge** — Cloudflare `*.app.k2.dev` orange-cloud + ACM + path rules +
   origin lock-down + the static `app.k2.dev` router. Cut over from the laptop
   Caddy. (Needs Grok's phases 1–3 landable.)
5. **[Both] Hardening** — rate-limits, keepalive/idle-timeout handling, offline
   UX, retention prune, LB across relays.

**So: Grok goes first** (phases 1–2 are pure app/daemon, no infra dependency),
Ops slots the **bucket** in during phase 2 and builds the **edge** (phase 4)
once Grok's half is testable.

## 7.6 Success criteria / done
- A user opens `<sub>.app.k2.dev` on a fresh browser (no install), logs in, and
  drives their **NAT'd** box (terminals + workspaces) — proven against a laptop
  daemon.
- The **served bundle matches the daemon's version** across a daemon self-update
  (no skew), and an **old un-updated daemon** still loads its correct old bundle.
- **No credential appears** in any URL, edge log, or browser history.
- Killing the relay (failover) or sleeping the box degrades to a **clear
  reconnect/offline UX**, not a hung spinner.
- A synthetic **L7 flood on `<sub>.app.k2.dev` is absorbed at Cloudflare**;
  relay CPU/QPS stays flat; the daemon never sees it.

## 8. Open questions
- **Cookie vs storage** for the persistent web-client token (defaulting httpOnly
  cookie — confirm).
- **Production web-client feature scope** — full parity minus OS-native bits
  (Browser tab / local-file / install-update / keychain), or a deliberately
  reduced "lite" client? (Demo amputation list is the starting point.)
- **Deep-wildcard cert** — ACM `*.app.k2.dev` vs a flatter single-host scheme
  with subdomain-in-path (trade CORS for cert simplicity — leaning ACM +
  same-origin).
- **Relay ↔ Cloudflare origin transport** — plain HTTPS to a locked-down origin
  vs Cloudflare Tunnel to the relay (hides the origin IP entirely).
- **When does role-split / anycast DNS land** relative to the 3rd edge box.
- **Multiplayer interaction** — the web client is another concurrent viewer/
  claimer (0.40.27 presence + grid-WS claim model). Confirm a browser session
  slots into the existing roster/kick/role machinery with no new work.
- ~~**Bundle bucket + provider**~~ — **DECIDED: Cloudflare R2** (no egress fees,
  Cloudflare-native since Cloudflare is already the edge). Ops provisions it +
  hands Grok the URL base + CI write credentials.
- **`web_client_enabled` default** — ON vs OFF out of the box (§6.7).
- **Mobile/PWA** — is `<sub>.app.k2.dev` installable as a PWA (manifest +
  service worker)? Cheap add, big "app on your phone" payoff — v1.1?

---

## 9. Awareness notes — resolve when we get there

These are **not pre-build blockers**. The architecture and phasing stand.
They are risks and contract details to **stay conscious of** as phases land;
resolve each when the work reaches that seam (Grok and/or Ops). Do not
silently invent a second design that ignores them.

### 9.1 Edge ↔ tunnel protocol for `.app` (Ops + Grok at phase 4)

Desktop path is grey-cloud / often E2E TLS to the daemon. Web path is
orange-cloud: **Cloudflare terminates TLS**, then origin is the relay.

Before production cutover, lock a short **origin protocol contract**:

- After CF termination, does the relay **HTTP reverse-proxy** into frp, or
  still attempt TLS passthrough?
- What **Host** does the daemon see: `adam.app.k2.dev`, rewritten
  `adam.k2.dev`, or something else?
- Does frpc need a **second registration / custom domain** for
  `*.app.k2.dev`, or does the relay strip `.app` and reuse the existing
  tunnel map?
- How does this interact with **E2E enrollment maps** (today centered on
  `user.k2.dev` / `*.user.k2.dev`)?

Grok can prove SPA + cookies over laptop Caddy (phases 1–2) without this;
**production path fails without it.** Sequence diagram preferred when Ops
and Grok sit down for phase 4.

### 9.2 Cookie auth as a *third* credential source (Grok phase 2)

Today the daemon already has **`?token=`** and **`Authorization: Bearer`**.
Web cookies are an **additional** source, not a replacement.

Resolve in the cookie phase:

- Cookie name, value shape (raw connect-user session token vs opaque handle).
- **TTL / idle** vs existing long-lived connect-user sessions (align or
  dual-TTL and document).
- Cookie **only** on `<sub>.app.k2.dev` — never on bare `app.k2.dev`.
- Login mints **connect-user** sessions only — **never** put the owner
  daemon token in a browser cookie.
- **CSRF:** custom header on mutating HTTP POSTs; **not** required on
  same-origin WebSocket upgrade (cookies ride the upgrade automatically).
- **Dev / laptop Caddy:** `Secure` cookies need HTTPS, or a
  **dev-only** relaxation so phase 1–2 is testable.

### 9.3 `webClientVersion` vs ConnectionGate floor (Grok phases 1 + 3)

- Field: new `webClientVersion` vs reuse `/boot-status.version` — pick one
  and stick to it in the loader contract.
- Support floor: same as desktop `MIN_COMPATIBLE`, or a **web-specific**
  floor if web bundles amputate or lag differently.
- Race: daemon ships **before** its `app/<ver>/` lands on the bucket
  (or the reverse) — loader UX for “bundle not published yet” vs hard fail.
- Desktop and web sharing one version number: decide whether a failed web
  publish **blocks** `release.sh` or is best-effort additive.

### 9.4 `web_client_enabled` default and wall semantics (Grok phase 2)

Default **ON** is a product choice (headline feature); unlike remote-session
(default OFF). When implementing the wall:

- Confirm ON vs OFF at ship time (§8 still open).
- OFF should **not** make unauthenticated `/boot-status` look like “server
  dead” to the loader — prefer a distinct teaching code for “web access
  disabled by owner.”
- Distinguish wall-OFF vs tunnel-down offline UX.

### 9.5 Companion / existing browser paths (product + Grok)

K2 already has companion / connect-user browser surfaces in places. Before
marketing “the” web portal:

- Is `<sub>.app.k2.dev` the **successor**, a parallel path, or a merge?
- Avoid two long-lived browser auth stories if one can absorb the other.

### 9.6 Feature amputation checklist (Grok phase 1)

Sibling PRD §2A holds shim detail; freeze a **works / stub / hidden** list
for v1 (Browser tab, keychain, install/update, native DnD, notifications,
etc.) so “full K2 in the browser” is not oversold. Demo amputation list is
the starting point, not the final product matrix.

### 9.7 Cloudflare idle timeouts (Grok client + Ops edge)

~100s proxy idle/read on CF is noted in §2.2. When implementing:

- Target **ping/keepalive** interval on `/events` (e.g. well under 100s).
- Inventory long-blocking `/cli` calls; convert to poll/stream if needed.
- Confirm **grid-WS / terminal I/O** sockets get the same keepalive story.

### 9.8 Multiplayer / presence (Grok when SPA is real)

Browser is another concurrent viewer/claimer. Confirm it slots into
existing roster / kick / role machinery with no Tauri-only client-id
assumptions (§8). Fix only if something breaks — don’t redesign presence.

### 9.9 Abuse, CSP, self-host (hardening phase 5)

- Pin a minimal **CSP** (same-origin `connect-src`, no third-party by default).
- Keep unauth `/boot-status` lean — don’t grow sensitive flags for the loader.
- Origin lock-down is the real control if CF is bypassed; login rate-limit
  at CF is defense-in-depth.
- Nested Pro labels under `.app` (e.g. `staging.adam.app.k2.dev`) are
  **out of scope** unless explicitly added later.
- Self-host / open-core: when documenting, minimum is serve loader + proxy
  `/cli/*`, `/boot-status`, `/events` — any edge, not Cloudflare-only.

### 9.10 Demo button vs this PRD

Success criteria here are **persistent web client against a real/NAT box**.
The marketing **“Try me”** button also needs the sibling VM/demo PRD
(`prd-k2-cloud-vm-hosting-and-demo-v1.md`). Do not ship release notes that
imply demo VMs are done when only edge delivery landed.

### 9.11 Release/CI hygiene (Grok phase 3)

- Web bundle publish is a **separate artifact** from DMG; failure modes
  should be loud and independent unless we choose atomicity.
- Staging vs prod bucket (if used) — don’t accidentally point prod loaders
  at empty/staging prefixes.
- Prune job uses the **support-floor cutoff** (§3), not arbitrary dates.

---

Related: `prd-k2-cloud-vm-hosting-and-demo-v1.md` (VM lifecycle + demo
guardrails), `prd-k2-remote-session-v1.md` (scoped identity; related
fail-closed walls, different credential model), [[Cloud - K2
Connect Relay]], [[Cloud - Nameserver Edge]], [[Codebase - Frontend and
Renderer]].
