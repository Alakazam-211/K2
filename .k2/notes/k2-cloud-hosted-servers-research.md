# K2 Cloud — Hosted Servers Research (2026-07-05)

Substrate map + execution evaluation for Rosson's 4-point K2 Cloud spec:
1. Hosted servers in Basic (shared vCPU, no sandboxing) and Pro (sandbox-capable) tiers.
2. Server visible in the k2.dev dashboard; attach a purchased subdomain; 1 pro-tier
   subdomain included with either plan.
3. First owner user provisioned for the customer, then they connect.
4. `/v1` API that spawns sessions NOT in sandboxes so Basic customers can still
   drive agents programmatically.

Four research agents mapped: (A) Linux server story, (B) owner/user bootstrap,
(C) subdomain/relay/billing cloud side, (D) `/v1` API + session spawn.
All file:line refs verified 2026-07-05.

---

## A. Linux server story — what exists

**Release artifact (plain daemon): EXISTS.**
- `.github/workflows/daemon-binaries.yml:42-138` — every `v*` tag builds
  `k2-daemon-linux-x86_64` + `-aarch64`, minisign-signed with the Tauri updater key.
  Plain `cargo build --release --bin k2-daemon` — NO `sandbox-microvm` feature.
- `scripts/release.sh:360-474` (Step 8.5) emits `daemon-latest.json` with per-platform
  `{url, sig, sha256}`; merges CI Linux artifacts if pre-staged in
  `target/release/daemon-dist/`, else macos-only manifest.
- No .deb/.rpm/tarball — bare signed binary + sig + manifest.

**Installer: EXISTS.** `scripts/install-daemon.sh` — curl|sh, arch-detect, mandatory
minisign verify (lines 195-220), installs `~/.local/bin/k2-daemon`, writes systemd
USER unit `~/.config/systemd/user/k2-daemon.service` `Restart=always` (237-256).
Footer 287-293: explicitly does NOT pair — "token bootstrap is still an open PRD
question." CLI twin: `k2 daemon install` (cli/k2:1588, router 10722-10731).

**Headless daemon: native.** Binds 127.0.0.1 only, sticky ephemeral port
(`main.rs:395-440`), publishes `~/.k2/daemon.port` + `daemon.token` (0600,
`write_restricted` main.rs:1736-1746). Public bind is forbidden by design — remote
reach is exclusively the frpc tunnel.

**Provisioning PRD exists, UNIMPLEMENTED:** `.k2/prds/prd-linux-headless-daemon.md`
(Draft 2026-06-20) — `provision.json` state machine,
`/cli/daemon/provision/{status,init-owner,pair,unpair}` + `rotate-token`,
`k2 daemon init-owner|pair` verbs, phases 0-6, risks R1-R9 (R1: owner token is
128-bit, plaintext, rotates every boot, never revoked). Grep confirms zero
implementation. This PRD is essentially the spec for K2 Cloud point 3.

**Sandbox host requirements (Pro tier):**
- `/dev/kvm` RW + kvm group (`k2-vmm-worker.rs:93,779-787`; `cell_uid_pool.rs:20-21`).
- libkrunfw + libkrun built `NET=1 BLK=1` to `/usr/local/lib64` + patchelf
  (`.k2/notes/sandbox-passt/RUNBOOK-v2.md` §3; `build.rs:20-35`).
- Guest rootfs `/opt/k2/guest-base` (worker :67-72, `K2_SANDBOX_GUEST_BASE`),
  built by `.k2/notes/sandbox-passt/build-guest.sh` (Debian 12, podman).
- setuid-root `k2-vmm-worker` (re-`chmod u+s` after every rebuild!); daemon runs
  non-root `k2` with `AmbientCapabilities=CAP_CHOWN CAP_NET_ADMIN`;
  `setcap cap_net_admin+ei /usr/sbin/nft`. System unit per
  `.k2/notes/runbook-self-host-sandbox-server.md:116-162` with
  `K2_SANDBOX_API=1 K2_SANDBOX=1 K2_HOOK_SCOPED=1`.
- **No sandbox-capable release artifact** — `sandbox-microvm` builds are on-box only
  (`Cargo.toml:19,36`; runbook :51). The live box runs a DEBUG on-box build.

**Capability gating:**
- SSOT: `v2_spawn::can_sandbox()` (`v2_spawn.rs:255-264`) = linux + feature +
  `K2_SANDBOX` env.
- `K2_SANDBOX_API` off → ALL `/v1/*` 404 (`dispatcher.rs:3231-3252`).
- API on but can_sandbox false → `POST /v1/sandboxes` = **409, never degrades**
  (`v1_sandboxes.rs:40-43,628-631`) — the "never silently unsandboxed" invariant.
- Internal spawns degrade LOUD to Passthrough (`v2_spawn.rs:1160-1178`).
- **No /boot-status capability flag for sandbox availability** — only observable
  via 409/404. Gap for tier-aware clients/dashboard.

**Boxes:** rpm.k2.dev = federation test box (0.40.13, no setup docs in-repo).
k2-sandbox-01 / linux-test.k2.dev (Hetzner bare-metal Ubuntu 24.04,
root@37.27.67.180) = sandbox box; runbooks: `p2b-onbox-bootstrap.md`,
`runbook-self-host-sandbox-server.md` (non-root provision, validated 2026-06-30),
`sandbox-passt/RUNBOOK-v2.md` + build-guest.sh. **sandboxes-OFF VPS runbook does
not exist yet** (recorded follow-up only).

**Config surface:** `~/.k2/` {daemon.port, daemon.token, tunnel.json, bin/frpc,
connect-users.json, settings.json, sandbox-homes/}. Tunnel autostart
`main.rs:817-872`. frpc resolved PATH → /usr/local/bin → ~/.k2/bin (connector.rs:49-64,
NOT auto-downloaded — image must bundle `frp_*_linux_amd64`). TLS terminated in-daemon
(`tunnel_tls_listener.rs`), LE cert via broker. Sandbox config is env-only.

## B. Owner/user bootstrap — what exists

- **Owner token**: minted fresh EVERY boot (`main.rs:432`, generate_token 1751-1755,
  32 hex chars), files under `~/.k2/` 0600. Owner = possession of
  `~/.k2/daemon.token` on-box. No keychain involvement daemon-side (Linux-clean).
- **Connect users** (`crates/k2-core/src/connect_users.rs`): argon2id,
  `~/.k2/connect-users.json` + `connect-sessions.json` (0600, tmp+rename).
  Roles `Viewer < Member < Admin < Owner`. **Owner ROLE is assignable to a
  connect-user** (`set_role` :524-540; route `/cli/users/set-role`
  dispatcher.rs:1743-1760) — but nothing creates one automatically today.
- **Headless owner creation works TODAY with two curls on-box:**
  ```
  curl -X POST "http://127.0.0.1:$(cat ~/.k2/daemon.port)/cli/users/add?token=$(cat ~/.k2/daemon.token)" \
       -d '{"username":"alice","password":"<temp>"}'
  curl -X POST ".../cli/users/set-role?token=..." -d '{"username":"alice","role":"owner"}'
  ```
- Login: `POST /cli/auth/login` (public, POST-only, 3-strikes/15-min lockout,
  session = SHA-256-digested 32-byte token, 30-day TTL, token_epoch revocation).
  `GET /cli/auth/whoami`, `POST /cli/auth/change-password` (revokes all sessions).
- Web portal: daemon serves `GET /` and `/account` (connect_users_routes.rs:299-535)
  — login + change-own-password only; user MANAGEMENT is desktop-app-only.
- **Gaps:** no seed-on-boot; no `must_change_password` flag on ConnectUser
  (:139-165); no invite tokens; no `k2 users` CLI verb; owner token ephemeral so
  a cloud control plane can't hold a durable owner credential (provision on-box).
- Note: some routes stay owner-TOKEN-only even for Owner-ROLE users:
  `/cli/users/set-password`, `/cli/users/policy`, tunnel control, federation
  minting, api-key management. Audit this list for hosted customers (they never
  have the daemon token unless they SSH — decide which of these an Owner-role
  session should be allowed to do; api-keys is the big one for point 4).

## C. Subdomains / relay / billing — what exists

**One Supabase project (K2X, ttgcalfrzzgkxnfepkiu) is the hinge.** The single
central table `public.subdomains` (types: k2-dev-web/lib/database.types.ts:17-40):
`owner_id→auth.users, label, tunnel_token, status, tier(free|single|pro),
stripe_*, current_period_end, target_endpoint, cert_status, e2e,
claimed_at/by/label`. Accounts ARE auth.users; no accounts table.

**Anything that writes a valid subdomains row is "attached":** the relay control
plane syncs active rows every 30s (`k2-connect/control-plane/src/main.rs:126-178,
360-382`); frps Login validates `metas.token` against it; NewProxy overwrites
custom_domains with the canonical FQDN set (anti-hijack, frp_plugin.rs:9-16);
HAProxy/Caddy/frps need ZERO per-customer config; cert broker (`POST
cert.k2.dev/cert`, authed by tunnel_token) already works headlessly.

**Purchase flow today:** k2-dev-web BuyForm (Single $2.99/mo, Pro $7.99/mo) →
`/api/checkout` → Stripe → webhook mints `tunnel_token = k2c_<label>_<32hex>` and
upserts the row (webhook/route.ts:132-235). No free-claim path — rows only come
from checkout. Upgrade/portal/downgrade-sweep routes exist. Relay only READS
billing columns (Stripe removed from control plane Phase 2).

**Daemon claim/bind today (interactive):** renderer Supabase login →
RLS-read own rows incl. tunnel_token → `claim_subdomain` RPC device lease
(3-min TTL, 60s renew) → `POST /cli/tunnel/config` writes `~/.k2/tunnel.json`
→ frpc render/supervise (`tunnel/{config,render,connector,lease,cert_broker}.rs`).
- **BUG (fix regardless):** `lease.rs:51-61` reads legacy keychain key
  `session-refresh-token`; renderer now writes blob key `session` → daemon-side
  lease renewal silently skipped on fresh sign-ins. Also keychain = Mac-only;
  hosted Linux daemon has no Supabase session at all → hosted rows need either
  a lease exemption or a token-authed claim path.

**Dashboard:** real per-user dashboard exists (`app/dashboard/page.tsx`) —
subdomains + tier/status/billing + Pro nested-children modal. Does NOT show:
tunnel_token, online/offline, claimed_by/at (in table, unrendered), any
server/device concept. No servers table anywhere.

**Multi-daemon per account: already true** (one account owns rosson, z3thon,
cfed, …). Each row = independent token = one tunnel = de-facto one daemon.

## D. /v1 API — what exists

- Global: `K2_SANDBOX_API` off → 404 all (`dispatcher.rs:3240-3251`). Auth
  `v1_principal` (http.rs:555-567): owner token or `k2sk_` API key
  (SHA-256 lookup, `api_keys.rs`; migrations 0058/0059:
  `anthropic_api_key` BYO cred per key, `allowed_workspaces` fail-closed
  NULL / "*" / JSON slugs). API keys can never manage keys; key CRUD is
  owner-token-only `/cli/api-keys/*`.
- Routes: `GET /v1/ping`; `POST /v1/sandboxes` (+`GET .../messages`);
  `POST/GET /v1/w/<ws>/sessions[/<id>][/messages]` (new/resume/message-live/fork
  + timeout_secs, idle reaper 180s default); `POST /v1/w/<ws>/message` —
  **the ONE non-sandbox /v1 path today**: drives the canonical agent via
  `workspace_msg::deliver_live`, gated by per-key workspace grant + per-workspace
  `remote_instruct` opt-in (fail-closed) + busy/HITL 409.
- Sandbox sessions ARE normal `DaemonPtySession`s (same `v2_spawn::spawn_session`
  + `v2_session_map`) with Microvm backend + per-session uid/egress; policy
  resolver (`v1_sandboxes/policy.rs:110-169`) forces command
  `claude --dangerously-skip-permissions`, drops caller env/args, mints agent
  name, pins cwd. Read-back = in-cell `k2 respond` over per-cell UDS →
  `sandbox_responses` ring (1000/session). Streams: grid WS accepts per-session
  stream tokens (24h TTL); API key itself never accepted on streams.
- **Non-sandboxed spawn is NOT reachable via /v1 today** (deliberate 409 at
  v1_sandboxes.rs:40/628 = the "never silently unsandboxed" invariant; PRD
  prd-sandbox-p3-api-spec.md:10-12).
- **Closest design doc for Basic-tier hosted sessions:**
  `.k2/prds/prd-sandbox-addendum-hosted-sessions.md` (draft 2026-06-30) —
  "one canonical spine, two front doors", F3 liveness router, F4 [from]
  attribution ≠ authz.

### What a non-sandboxed /v1 sessions route needs (point 4)
Reusable as-is: `v1_principal`, `authorizes_workspace`,
`resolve_authorized_workspace`, `sandbox_quota`, `sandbox_reaper`,
`v2_spawn::spawn_session` (sandbox:None → Passthrough), live-inject pattern
(v1_sandboxes.rs:477-493), `stream_token::mint` + grid WS.
Genuinely NEW (2 pieces):
1. **Passthrough policy resolver** mirroring `policy::resolve_spawn`: host-minted
   agent name, cwd pinned to the granted workspace's registered path (never
   $HOME/caller path), forced command, dropped caller env/args. Without it an
   API key = arbitrary host RCE as the daemon user.
2. **Response read-back without the cell UDS**: either wire host-side
   `k2 respond`/hook egress into `sandbox_responses`, or lean on the grid stream
   token (weaker: raw PTY, not curated F2 messages). Recommend the former —
   K2_HOOK_SCOPED hook env already exists for host sessions.
Also: don't run this through `/v1/sandboxes` (preserve the invariant); make it an
explicit sibling (`/v1/w/<ws>/host-sessions` or `sandbox:"none"` echo), and split
the gate flag (`K2_SANDBOX_API` currently means "the /v1 surface exists" — rename
or add `K2_HOST_SESSIONS_API` so Basic images enable /v1 without sandbox routes).

**Security rationale that makes Basic acceptable:** a hosted Basic server is
SINGLE-TENANT — the VPS belongs to that one customer; a non-sandboxed session
runs as the `k2` user on their own box. Blast radius = their own server, same as
`k2 talk` locally. The hard "never unsandboxed" stance exists for multi-tenant/
untrusted callers; per-key workspace grants + forced-command resolver + honest
labeling keep the API contract clean. (Rosson pre-decided this 2026-07-01:
"shared-compute gets a separate runbook — sandboxes OFF, API calls run open-ended
sessions directly in the workspace, unsafe-but-easy for students.")

---

## Synthesis — the four points

### 1. Basic vs Pro tiers = a physical split, not a feature flag
- **Basic** = shared-vCPU cloud VPS (no /dev/kvm) → `can_sandbox()` false →
  sandbox routes 409/absent. Runs: full cockpit over K2 Connect, canonical
  agents, `k2 talk`, `/v1/w/<ws>/message`, and (new) non-sandboxed /v1 sessions.
- **Pro** = KVM-capable host (Hetzner dedicated/bare-metal, like k2-sandbox-01)
  → full sandbox API. Pro provisioning today = the validated-but-manual runbooks
  (libkrun build, guest image, setuid worker, caps).
- Daemon work: expose sandbox capability in `/boot-status` (or `/v1/ping`) so
  clients/dashboard can show tier truthfully; KVM-detect at boot.
- Pricing memory (2026-06-29): market ~$0.05/vCPU-hr; K2 rec = price the CELL
  $0.10-0.20/cell-hr for sandbox compute + flat server fee. Suggest: Basic
  flat $X/mo (VPS cost + margin), Pro flat $Y/mo + metered cell-hours later.

### 2. Dashboard + subdomain attach
- Provisioning service writes a `subdomains` row directly (service-role):
  `{owner_id, label, tunnel_token: k2c_<label>_<32hex>, status: active,
  tier: pro}` — relay attaches automatically ≤30s, zero relay changes. Included
  pro subdomain = a row with tier pro and either no stripe_subscription_id or
  bundled into the server subscription's line items.
- Server image/cloud-init writes `~/.k2/tunnel.json` (token, label, device_id,
  auto_start:true) — skips the interactive claim/bind entirely. Image bundles
  frpc.
- New `servers` table (Supabase, RLS owner-select): `{id, owner_id, kind
  (self-hosted|k2-cloud), plan(basic|pro), provider_instance_id, region,
  daemon_version, subdomain_id FK, last_seen_at, status}`. Liveness: cleanest =
  relay control plane writes connect/disconnect on frp Login/CloseProxy (it
  already PATCHes Supabase for e2e flags — main.rs:604-666, ~50-line pattern
  reuse). Dashboard gets a "Your servers" section + deep link
  https://<label>.k2.dev (portal login already works).
- Fix the lease for hosted rows: exempt hosted/server-claimed rows from the
  device lease, or add a tunnel_token-authed claim; fix the lease.rs keychain
  key mismatch bug regardless.

### 3. First owner user
- MVP (zero daemon changes): cloud-init/provision script waits for
  `~/.k2/daemon.{port,token}`, then `POST /cli/users/add` +
  `POST /cli/users/set-role {"role":"owner"}`. Credentials surfaced ONCE in the
  dashboard (or emailed via Resend, already wired in k2-dev-web).
- Proper (small daemon slice, aligns with prd-linux-headless-daemon.md):
  `must_change_password: bool` on ConnectUser (serde-default false), enforced in
  handle_login → session restricted to change-password until cleared; and/or a
  consumed-on-boot seed file (`~/.k2/seed-users.json`) so the image never bakes a
  password; ideally invite-token so no temp password ever exists.
- Audit the owner-TOKEN-only route list for what an Owner-ROLE session must be
  able to do remotely (api-key management is required for point 4; tunnel
  control probably stays op-only).

### 4. Non-sandboxed /v1 sessions for Basic
- Ship in two layers:
  a. **Already works**: `/v1/w/<ws>/message` (canonical-message API) — document
     it as the Basic-tier "talk to your agent" API. Needs only the
     K2_SANDBOX_API gate rename/split so Basic images can enable /v1.
  b. **New**: `/v1/w/<ws>/host-sessions` (or param) — passthrough resolver +
     host read-back path per §D. Same shapes as sandbox sessions
     (sessionId/messages/since/timeout_secs) so client code is tier-portable;
     response body says `"sandbox":"none"` honestly.
- Keep `/v1/sandboxes` 409-on-Basic exactly as-is (the invariant holds; the new
  route is a different, honestly-labeled door).

## Proposed execution order

- **Phase 0 — Golden image + provision script (no product code):** Ubuntu 24.04
  image with k2-daemon release artifact, frpc, k2 CLI deps (curl/python3/openssl),
  system-level systemd unit (non-root k2 user), cloud-init template that writes
  tunnel.json + creates owner user via the two curls + posts back to the control
  plane. Deliverable: `scripts/provision-k2-server.sh` + the missing
  sandboxes-OFF VPS runbook (they're the same document, basically). Manually
  sellable hosted server at the end of this phase.
- **Phase 1 — Daemon slices:** (1) must_change_password + seed-users;
  (2) sandbox-capability in /boot-status + K2 CLI/API gate split
  (K2_SANDBOX_API → surface flag + per-family flags); (3) lease fix/exemption;
  (4) owner-role route audit (api-keys manageable by Owner-role session).
- **Phase 2 — Basic-tier API:** passthrough resolver + /v1 host-sessions +
  host read-back; tests mirroring the sandbox suites; API docs.
- **Phase 3 — Cloud control plane + dashboard:** servers table + RLS;
  provisioning service (Hetzner Cloud API for Basic; dedicated/Robot for Pro);
  Stripe products (server-basic, server-pro; bundle 1 pro subdomain);
  dashboard "Your servers" (status, subdomain attach picker for extra
  subdomains, first-owner credential reveal, restart/destroy);
  relay liveness writes.
- **Phase 4 — Pro automation:** sandbox-capable build artifact story (likely:
  bake libkrun+guest+worker into the Pro image rather than solving CI
  distribution), guest image versioning, bare-metal provisioning runbook →
  script, cell metering for future usage billing.

## Open decisions for Rosson
1. Hosting provider/SKUs: Hetzner Cloud shared vCPU for Basic (CX/CPX) +
   Hetzner dedicated (AX/EX via Robot) for Pro? (Current relay + sandbox box
   are both Hetzner; bare-metal provisioning is slower — hours not seconds —
   which shapes the Pro purchase UX. Alternative: a pool of pre-provisioned
   Pro hosts.)
2. Pricing: flat monthly per tier to start? (Basic ~2× VPS cost; Pro anchored
   near dedicated-host cost + margin; metered cell-hours later per the
   2026-06-29 benchmark memo.)
3. First-owner handoff UX: dashboard-reveal-once vs email vs invite-link.
4. Does the included pro subdomain count against a Pro-tier Stripe product, or
   is it a comped row (tier=pro, no subscription) tied to the server's
   subscription lifecycle?
5. Basic-tier API naming: sibling route (`host-sessions`) vs `sandbox:false`
   param — sibling recommended (keeps the never-degrade invariant legible).

---

# Addendum 2026-07-05 — one-click flow, dashboard ops, website/docs audit

(Rosson confirmed the plan + proposed order. New asks: one-click purchase→
server→subdomain; dashboard reboot/update + live status of server AND K2 app;
evaluate the website — zero API docs, doesn't sell the story.)

## One-click purchase flow (yes, feasible — checkout is the only interaction)

Buy page: pick Basic|Pro → type a subdomain label (live-validated, same
`validateLabel`) OR pick an already-owned unattached subdomain from a dropdown
→ Stripe checkout (ONE subscription: server + included pro subdomain). Then
zero further clicks:
1. Webhook (existing `checkout.session.completed` handler pattern) → insert
   `subdomains` row (mint `k2c_` token, tier pro, comped/bundled) + insert
   `servers` row (status: provisioning) + call Hetzner API: create VM from
   the golden image with cloud-init payload {tunnel.json contents, owner
   username, temp password or invite token, control-plane callback URL}.
2. Box boots → daemon up → tunnel auto-starts (relay synced the row ≤30s) →
   provision script creates owner user → calls back control plane →
   `servers.status = online`.
3. Dashboard live-updates provisioning→online (Supabase realtime or poll);
   credential reveal / invite link; "Connect" instructions (app host entry =
   https://<label>.k2.dev).
Re-pairing a DIFFERENT subdomain later = dashboard action → control plane
calls the daemon over the CURRENT tunnel: `POST /cli/tunnel/config` (writes
tunnel.json) + tunnel restart → daemon comes up on the new label. The old
label frees up. (Config route exists; needs the ops auth channel below.)

## Dashboard ops: status / reboot / update

**Status = three independent signals, show all three:**
- VM: provider API (Hetzner server status) — is the machine up.
- K2 daemon/tunnel: relay control plane already sees every frp Login/
  CloseProxy (`frp_handler`, k2-connect main.rs:494) and already PATCHes
  Supabase (`set_e2e_flag` pattern, main.rs:604-666) → write
  `servers.last_seen_at/online` on connect/disconnect. This is the truthful
  "K2 app is running and reachable" signal.
- Version: daemon `/boot-status` (already reports version + installKind) read
  over the tunnel; compare to public `daemon-latest.json` → "Update available".

**Actions — the daemon-side machinery ALREADY SHIPPED (0.39.33-0.39.35):**
- Restart K2: `POST /cli/daemon/restart` (owner-or-admin session auth;
  graceful shutdown, systemd Restart=always respawns, reaps PTY children).
- Update K2: `/cli/daemon/update/{check,start,status,apply}` — Shape B
  headless binary self-update: download signed artifact → minisign+sha256 →
  atomic swap → restart → health-check → auto-rollback. Works on headless
  installKind (0.39.35+ manifest fix). CAVEAT: the real two-version Linux/
  systemd e2e was never run — make it a Phase-0/1 acceptance test.
- Reboot VM: Hetzner API from the control plane (also the fallback when the
  daemon/tunnel is unreachable — reboot brings systemd + daemon + tunnel back).

**Auth channel for ops (recommended):** provision-time service admin user
(e.g. `k2cloud-ops`, Admin role, random password held encrypted by the
control plane) on each hosted daemon. Control plane logs in over the tunnel
(`/cli/auth/login`) and calls restart/update/tunnel-config with that session
— all three routes accept Admin-session auth already, so ZERO daemon changes.
Transparency: the customer sees the ops user in their user list; removing it
= opting out of managed ops (document it). Alternative (more work, later):
signed one-time action tokens minted by the control plane.
Optional: golden image ships a systemd timer for unattended auto-update
(dashboard toggle), so "update" doesn't depend on tunnel reachability.

## Website + docs audit (agent-verified, k2-dev-web)

**Current story = frozen at ~0.40.14 reality.** Homepage is actually strong
(agent-server positioning, 9 agent logos, canonical-files diagram, daemon
story, download button, Fair Source). But:
1. **No docs section at all** — no /docs, no MDX tooling, no API reference;
   zero mentions of /v1, k2sk_ keys, sandboxes, federation, presence anywhere
   on the site. Biggest gap for selling K2 Cloud (an API+CLI product).
2. **No public /pricing page** — Single $2.99/Pro $7.99 only visible inside
   the auth-gated BuyForm. K2 Cloud tiers need a home.
3. **CLI has no page** — homepage shows one transcript; 25+ verbs undocumented.
4. **Stale/contradictory copy on /k2-connect**: still instructs NGROK setup
   (page.tsx:186-192, 341-373) next to the K2 Connect pitch; "K2 by K2"
   mockup glitch (:208).
5. Missing arcs: federation, presence/multiplayer, projects/feedback/agent
   mgmt, Linux story, K2-as-a-Server entirely.
6. Minor: stale layout.tsx meta (old positioning, Aider keyword), 5-URL
   sitemap, no llms.txt, no blog.

**Docs feasibility is GOOD:**
- Tooling: none installed, but the site is plain Tailwind+Next 16 — drop in
  fumadocs or @next/mdx; a hand-rolled markdown renderer already exists
  (changelog page :45-91). `docs` is already a reserved subdomain label.
- Seed content already written:
  - API reference: `.k2/notes/runbook-self-host-sandbox-server.md:181-211`
    has public-ready curl flows (api-key create → POST /v1/sandboxes → GET
    messages); `.k2/prds/prd-sandbox-p3-api-spec.md` is the authoritative
    spec (needs public-tone rewrite). No OpenAPI file exists — author one.
  - CLI reference: `k2 --schema` (cli/k2:5482) emits the FULL machine-readable
    verb/flag catalog → auto-generate the CLI docs; per-verb heredoc help is
    polished. Caveat: help prose still says K2SO/K2SO_* in places — rebrand
    pass needed.
  - Changelog: WHATS_NEW.md (90 public-voice entries) — drop-in.
  - Concepts: docs/ARCHITECTURE.md is public-grade; README has good prose.
  - Old K2SO-website: salvage 4 dropped sections (workspace states/autonomy,
    heartbeat scheduling, Cmd+L assistant, agencies/teams) as copy skeletons
    + the llms.txt pattern.

**Website track (parallel to the server phases):**
- W1 quick fixes (hours): kill ngrok copy, fix meta/description + K2-by-K2,
  sitemap, llms.txt.
- W2 public /pricing (subdomains now; server tiers land with Phase 3).
- W3 /docs: Getting Started (download/install, Linux `install-daemon.sh`),
  CLI reference (auto-gen from --schema), API reference (runbook seed +
  OpenAPI), Concepts (workspace=agent, daemon, K2 Connect, sandboxes,
  federation).
- W4 homepage story refresh (API/K2-as-a-Server section, federation,
  presence, projects) + K2 Cloud landing page when Phase 3 ships.
W1+W2 anytime; W3 should precede or accompany Phase 2 (Basic-tier API needs
docs to be sellable); W4 with Phase 3.
