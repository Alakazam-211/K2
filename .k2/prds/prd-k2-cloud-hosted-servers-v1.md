# PRD — K2 Cloud: Hosted Servers V1

**Status:** APPROVED by Rosson 2026-07-06 (plan + phase order + tier names + pricing).
**Research SSOT:** `.k2/notes/k2-cloud-hosted-servers-research.md` (4-agent substrate
map, file:line verified 2026-07-05).
**Companion PRD:** `.k2/prds/prd-v1-api-completion.md` (the API routes K2 Cloud
Standard depends on; separable, ships independently).
**Related:** `.k2/prds/prd-linux-headless-daemon.md` (draft 2026-06-20 — Phase 1
here implements a subset of it), `.k2/prds/prd-sandbox-addendum-hosted-sessions.md`.

---

## 1. Summary

K2 Cloud sells always-on K2 servers we run for the customer. One click at
checkout produces, with no human in the path: a provisioned Linux server
running the K2 daemon, an attached `your-name.k2.dev` Pro subdomain (included),
a ready first owner login, and a dashboard card with live status and
reboot/update controls.

Two tiers, priced monthly only (annual explicitly dropped — margin decision,
Rosson 2026-07-06):

| | **Standard — $29/mo** | **Dedicated — $99/mo** |
|---|---|---|
| Hardware | Hetzner Cloud shared vCPU (CPX-class, 4–8 GB) | Hetzner Robot bare metal (AX/EX-class) |
| Sandbox API (`/v1/sandboxes`) | ❌ (no `/dev/kvm` — physical constraint) | ✅ hardened microVM cells |
| Host-session API (`/v1` non-sandboxed) | ✅ (see companion PRD) | ✅ |
| Cockpit, agents, multiplayer, CLI | ✅ | ✅ |
| Included Pro subdomain ($7.99 value) | ✅ | ✅ |
| Dashboard reboot/update/status | ✅ | ✅ |
| Cost basis / margin | ~$10–16 → ~2× | ~$50 → ~2× |

Tier names locked: **Standard / Dedicated** (not Shared/Private, not
vCPU/BareMetal). The tier split is physical: `can_sandbox()` requires
Linux + `sandbox-microvm` build + `K2_SANDBOX` + `/dev/kvm`
(`v2_spawn.rs:255-264`), and Hetzner Cloud does not expose nested virt.

Public pricing is already live at k2.dev/pricing ("Coming soon" badge) and in
the GEO files (llms.txt / llms-full.txt / StructuredData.tsx — grep `2.99`
when prices change).

## 2. The one-click purchase flow (customer-visible spec)

Buy page interaction = exactly three inputs: **tier**, **subdomain label**
(live-validated via the existing `validateLabel` rules, or pick an owned
unattached subdomain from a dropdown), **pay** (Stripe Checkout, one
subscription bundling server + included subdomain). Then zero further clicks:

1. `checkout.session.completed` webhook (extend the existing handler in
   k2-dev-web `app/api/stripe/webhook/route.ts`):
   a. Insert `subdomains` row via service-role: `{owner_id, label,
      tunnel_token: k2c_<label>_<32hex>, status: active, tier: pro}` —
      the relay control plane syncs active rows every 30s
      (k2-connect `control-plane/src/main.rs:126-178`) → **zero relay changes**.
   b. Insert `servers` row `{status: provisioning}` (schema §4.3).
   c. Kick the provisioner (§4.2): Standard → hcloud create-from-snapshot;
      Dedicated → assign from the pre-warmed pool (§5).
2. Box boots → cloud-init/first-boot script: writes `~/.k2/tunnel.json`
   (`TunnelConfig` fields, `crates/k2-core/src/tunnel/config.rs:47`,
   `auto_start: true`), daemon starts (systemd, non-root `k2` user), tunnel
   connects (row already synced), script creates the first owner user (§6),
   then POSTs a call-home to the control plane.
3. `servers.status = online`; dashboard live-updates (Supabase realtime or
   poll); one-time credential reveal / invite link; "Connect" instructions
   (app host entry = `https://<label>.k2.dev`; portal login already works).

Re-pairing a different subdomain later: dashboard action → control plane calls
the daemon over the CURRENT tunnel — `POST /cli/tunnel/config` + tunnel
restart → daemon comes up on the new label; the old label frees.

Upgrade path note (must be in the UI copy): **Standard→Dedicated is a
re-provision + data migration** (different physical hardware), not a plan
toggle. V1 may ship without automated migration (manual, support-assisted);
say so honestly.

## 3. What already exists (do not rebuild)

- **Signed Linux daemon artifact**: `.github/workflows/daemon-binaries.yml:42-138`
  builds + minisign-signs `k2-daemon-linux-{x86_64,aarch64}` on every tag;
  `scripts/release.sh:360-474` emits `daemon-latest.json`.
- **Installer**: `scripts/install-daemon.sh` (arch-detect, mandatory minisign
  verify, systemd user unit). Explicitly does NOT pair (lines 287-293).
- **Remote restart/update (SHIPPED 0.39.33–0.39.35)**:
  `POST /cli/daemon/restart` (owner-or-admin session auth) and
  `/cli/daemon/update/{check,start,status,apply}` — Shape B headless binary
  self-update: signed download → verify → atomic swap → health-check →
  auto-rollback; `installKind` on `/boot-status`.
- **Headless first-owner**: two on-box curls — `POST /cli/users/add` +
  `POST /cli/users/set-role {"role":"owner"}` with `~/.k2/daemon.{port,token}`.
  Owner ROLE is assignable to a connect-user (`connect_users.rs:524-540`).
- **Subdomain hinge**: `public.subdomains` in Supabase K2X — web writes,
  relay routes from it (30s sync), daemon reads its row for the token,
  HAProxy/Caddy/frps need no per-customer config. Cert broker
  (`POST cert.k2.dev/cert`, tunnel_token-authed) already works headlessly.
- **Relay→Supabase write pattern**: `set_e2e_flag` (k2-connect
  `main.rs:604-666`) — the model for liveness writes.
- **Dashboard**: k2-dev-web `app/dashboard/page.tsx` (subdomains only today).

## 4. Architecture

### 4.1 Golden images
- **Standard**: Hetzner Cloud **snapshot**. Ubuntu 24.04 + k2-daemon release
  binary + frpc linux build (NOT auto-downloaded — `connector.rs:49-64`
  resolver paths; bake at `/usr/local/bin/frpc`) + `k2` CLI deps
  (curl/python3/openssl) + system-level systemd unit (non-root `k2` user,
  `Restart=always`) + first-boot provisioning script reading cloud-init
  user_data. Rebuilt per release (scripted via `hcloud` CLI).
- **Dedicated**: no snapshots on bare metal — `installimage` (Ubuntu 24.04)
  + idempotent bootstrap script over SSH: everything in the Standard image
  PLUS the sandbox stack per `.k2/notes/runbook-self-host-sandbox-server.md`
  + `.k2/notes/sandbox-passt/RUNBOOK-v2.md`: libkrun/libkrunfw (prebuilt
  artifacts we host — do NOT compile per-box), guest base image
  (`build-guest.sh` output, hosted), setuid-root `k2-vmm-worker`
  (re-`chmod u+s` after any rebuild), `setcap cap_net_admin+ei` on nft,
  systemd unit with `K2_SANDBOX=1 K2_SANDBOX_API=1 K2_HOOK_SCOPED=1` +
  `AmbientCapabilities=CAP_CHOWN CAP_NET_ADMIN` + `SupplementaryGroups=kvm`.

### 4.2 Provisioning service (control plane)
Lives with the k2-dev-web backend (Next.js route handlers + a small worker)
or as a job runner alongside the relay control plane — decision at build time;
default: Next.js routes + Supabase queue table (no new infra). Responsibilities:
- Hetzner Cloud API (Standard): create/delete/reboot/power from snapshot with
  cloud-init user_data; label servers with `servers.id`.
- Hetzner Robot API (Dedicated): pool replenishment orders, rescue +
  installimage, bootstrap-script runs (SSH with a provisioning key that is
  REMOVED at handoff).
- Per-customer payload: tunnel.json contents, owner username, invite/temp
  credential, callback URL + one-time callback token.
- Ops proxy for the dashboard (§7): holds each hosted daemon's `k2cloud-ops`
  credential encrypted; executes restart/update/tunnel-config calls over the
  tunnel.

### 4.3 Supabase schema (new)
```
servers (
  id uuid pk, owner_id → auth.users, kind text ('k2-cloud'),  -- future: 'self-hosted'
  plan text ('standard'|'dedicated'),
  provider text ('hcloud'|'robot'), provider_instance_id text,
  region text, subdomain_id → subdomains.id,
  status text ('provisioning'|'online'|'offline'|'rebooting'|'updating'|'error'|'suspended'),
  daemon_version text, last_seen_at timestamptz,
  created_at, updated_at
)
server_events (server_id, kind, detail, created_at)   -- audit trail for ops actions
provision_queue (id, server_id, action, payload jsonb, state, attempts, ...)
```
RLS: owner-select on `servers`/`server_events` (mirror `subdomains` policies);
service-role writes only. Pool boxes: `servers` rows with `owner_id = NULL`
(service-role visible only) until assignment.

### 4.4 Status: three independent signals, all shown
1. **VM**: provider API (hcloud server status / Robot reset status).
2. **K2 daemon reachable**: relay control plane writes
   `servers.last_seen_at/status` on frp Login/CloseProxy (`frp_handler`,
   k2-connect `main.rs:494`) — ~50-line reuse of the `set_e2e_flag` PATCH
   pattern. This is the truthful "the app is running" signal.
3. **Version**: `/boot-status` over the tunnel (has version + installKind);
   compare to public `daemon-latest.json` → "Update available" badge.

## 5. Dedicated pool model (decision: pre-warmed pool, Rosson-approved)

- Keep **1–2 provisioned, unassigned Dedicated boxes** (idle ~$50/mo each).
- Purchase → instant **assignment**: claim a pool row, write the subdomain
  row, run the per-customer sequence (tunnel.json over SSH-or-firstboot,
  owner user, callback) — seconds, fully automated.
- Replenish asynchronously via Robot API ordering + automated
  installimage/bootstrap; alert a human on stock-out or bootstrap failure
  (manual fallback). First 2–3 Dedicated orders are done BY HAND to validate
  the SKU and harden the bootstrap script before trusting automation.
- Hetzner Cloud CANNOT serve Dedicated (no `/dev/kvm`) — do not "fall back."

## 6. First-owner provisioning + daemon slices (Phase 1)

MVP mechanics (zero daemon changes) work today: first-boot script waits for
`~/.k2/daemon.{port,token}`, curls users/add + set-role owner. The proper V1
adds these daemon slices (each small, testable, in line with
`prd-linux-headless-daemon.md`):

- **S1 `must_change_password`** on `ConnectUser` (`connect_users.rs:139-165`,
  serde-default false): set by users/add opt-in flag; while set, a session
  authenticates but is restricted to `/cli/auth/change-password` (+ whoami);
  cleared by change_password. Login response carries `mustChangePassword`.
- **S2 seed-users file**: boot consumes-and-deletes `~/.k2/seed-users.json`
  (username/password-hash-or-temp/role/must_change) so the image never bakes
  a plaintext credential and provisioning never needs to poll for the port.
- **S3 sandbox capability on `/boot-status`**: `sandboxes: "microvm"|"none"`
  from `can_sandbox()` — dashboard + clients render tier truthfully (today
  availability is only observable as 409/404).
- **S4 hosted-lease fix**: `tunnel/lease.rs:51-61` reads legacy keychain key
  `session-refresh-token` (renderer now writes `session`) AND is
  Mac-keychain-only — hosted Linux daemons have no Supabase session. Fix the
  key mismatch; add a lease exemption or tunnel_token-authed claim path for
  hosted/pre-bound rows.
- **S5 owner-role route audit**: routes gated owner-TOKEN-only that a hosted
  customer's Owner-ROLE session must reach. Required: `/cli/api-keys/*`
  (create/revoke/list — customers never hold the daemon token). Review:
  `/cli/users/set-password`, `/cli/users/policy`, tunnel control (likely
  stays op-only), federation minting (stays owner-token).
- **S6 ops user**: `k2cloud-ops` (Admin role) created at provision; documented
  in the customer-facing user list; deleting it = opting out of managed ops
  (dashboard then greys reboot-K2/update buttons, VM-level reboot still works).

## 7. Dashboard: "Your servers"

New section in k2-dev-web dashboard (mirrors the subdomains list):
- Card per server: name/label, plan badge, region, the three status signals
  (§4.4), daemon version + "Update available", attached subdomain (+ change
  action), created/renews line.
- Actions: **Restart K2** (`POST /cli/daemon/restart` via ops proxy),
  **Update K2** (`update/start`, progress from `update/status`, rollback copy),
  **Reboot server** (provider API — also the fallback when the tunnel is
  down), **Destroy** (double-confirm; cancels via Stripe portal semantics).
- First-connect: one-time credential reveal (or invite link) + copy-paste
  connect instructions.
- All ops actions append `server_events` rows (visible audit trail).

Auth channel: dashboard → control plane (Supabase session) → ops proxy logs in
to the daemon over the tunnel as `k2cloud-ops` → calls the route. The
customer's browser never holds daemon credentials.

## 8. Automation matrix (what's manual, forever)

| Task | Automated? |
|---|---|
| Standard: purchase→online | ✅ 100%, minutes |
| Dedicated: purchase→online (pool) | ✅ assignment instant; replenish async |
| Dedicated: pool replenish | ✅ Robot API; ⚠ human alert on stock-out/failure |
| Reboot/update/status | ✅ (daemon routes exist; VM via provider API) |
| Golden image per release | ✅ scripted; a human owns pressing it |
| Hetzner quota raises | ❌ manual (account limits gate new accounts) |
| Hardware failure (Robot) | ❌ ticket + re-provision from pool |
| Abuse/cancellation edges | ❌ manual policy |
| First 2–3 Dedicated orders | ❌ deliberate manual validation |

## 9. Phases + acceptance criteria

- **Phase 0 — Golden image + provision script.** Deliverables:
  `scripts/provision-k2-server.sh` (doubles as the missing sandboxes-OFF VPS
  runbook AND the Raspberry Pi self-host guide), Standard snapshot recipe,
  cloud-init template. ACCEPT: fresh hcloud VM from snapshot reaches
  online-with-owner-login with zero SSH. Also: run the **Linux two-version
  self-update e2e** (never validated — 0.39.33 memory) on this box.
- **Phase 1 — Daemon slices S1–S6.** ACCEPT: integration tests per slice
  (crib `presence_integration.rs` harness patterns); forced-password-change
  round-trip over the tunnel; boot-status shows `sandboxes` field.
- **Phase 2 — API completion** (companion PRD). ACCEPT: its own criteria.
- **Phase 3 — Control plane + dashboard + Stripe.** Products
  `server-standard` $29 / `server-dedicated` $99 monthly-only, bundled
  subdomain; `servers` table + RLS; provisioning service; relay liveness
  writes; "Your servers" UI with all ops actions. ACCEPT: real card
  purchase → online server → connect from the app → restart+update from
  dashboard → destroy; downgrade-sweep handles lapse (server →
  suspended/stopped per policy).
- **Phase 4 — Dedicated automation.** Prebuilt libkrun artifacts + hosted
  guest image + bootstrap script; pool tooling; Robot ordering. ACCEPT:
  pool-assigned Dedicated passes the full sandbox smoke
  (`POST /v1/sandboxes` → cell → messages) with no human touch after
  assignment.

Website tie-in: W3 `/docs` (getting started, CLI ref auto-gen from
`k2 --schema`, API ref + OpenAPI) must land before or with Phase 2/3 —
the API is not sellable undocumented.

## 10. Decisions log

- 2026-07-05 Rosson: plan + 5-phase order approved.
- 2026-07-06 Rosson: tiers = **Standard/Dedicated**; **monthly only** ($29/$99),
  annual dropped for servers (domains keep annual); 1 Pro subdomain included
  with either tier; one-click purchase is the bar; dashboard must show live
  status of BOTH the VM and the K2 app, with reboot/update controls.
- 2026-07-06: Dedicated = pre-warmed pool (assignment instant, replenish async).
- 2026-07-06: ops channel = provision-time `k2cloud-ops` Admin user
  (customer-visible, removable = opt-out).
- Standing (2026-07-01, Rosson): shared-compute/Basic runs API sessions
  directly in the workspace, sandboxes OFF — acceptable because hosted
  Standard is SINGLE-TENANT (customer's own box).

## 11. Open questions (non-blocking for Phase 0)

1. Region choice at checkout vs single default region (recommend: default
   only for V1, near the relay/Ashburn or Falkenstein by latency need).
2. Included-subdomain billing mechanics in Stripe: bundled line item vs
   comped row tied to the server subscription's lifecycle (recommend: comped
   row, `stripe_subscription_id` = the server's sub, so lapse handling is one
   code path).
3. First-owner handoff: dashboard-reveal-once vs invite link (recommend:
   invite link once S1/S2 exist; reveal-once for Phase 0 manual sales).
4. Suspension policy on payment lapse: stop VM (data kept N days) → destroy.
5. Standard SKU sizing: CPX31-class (4 vCPU/8GB) vs CPX21 (3/4) — validate
   with a real workload on the Phase 0 box.

## 12. Risks

- **R1 Linux runtime e2e untested** (PTY semantics, self-update, ggml CPU —
  `p1-daemon-binaries-headless.md:77-101`): Phase 0 acceptance covers this.
- **R2 owner token model** (128-bit, plaintext, rotates per boot, loopback):
  fine on-box; never export it off the box. Control plane holds ONLY the
  ops-user credential.
- **R3 Robot stock/ordering variability**: pool + human alert.
- **R4 tier promise drift**: pricing page says API=Pro-domain and
  sandboxes=Dedicated — enforcement must land (companion PRD F5, boot-status
  S3) before self-serve purchase opens.
- **R5 image staleness**: tie image rebuild into release.sh checklist
  (WHATS_NEW-style gate) so hosted boxes never ship stale daemons.
