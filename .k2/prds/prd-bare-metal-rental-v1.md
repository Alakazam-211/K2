# PRD: Bare-Metal Rental v1 — one-click Dedicated servers via Hetzner Robot

**Status:** direction approved (Rosson, 2026-07-08). Companion to
`prd-k2-cloud-hosted-servers-v1.md` (its §4.2/§5/§9-Phase-4 sketched this;
this PRD is the execution spec). Audience: a future — possibly junior —
implementer. §7 pre-mortem is mandatory reading before code.

## 1. Where we actually are (verified against code, 2026-07-08)

**Already built and working (do NOT rebuild):**
- Checkout → webhook → queue → sweep pipeline in `k2-dev-web`:
  `app/api/checkout-server/route.ts` (Dedicated gated behind
  `DEDICATED_CHECKOUT=on` + mandatory `sandboxConsent`),
  `app/api/stripe/webhook/route.ts` (inserts `servers` row with
  `provider:'robot'`, enqueues `provision_queue` create job),
  `app/api/cron/provision-sweep/route.ts` (Vercel cron worker, CAS
  claims, retries, `AwaitingStockError` hold, ops emails).
- Pool **assignment**: `lib/servers-db.ts::claimPoolServer` (CAS over
  `pool_state='stock'` rows). Schema supports `provider in
  ('hcloud','robot')` + pool columns.
- On-box bootstrap, proven on real hardware (k2-dedicated-01 +
  k2-sandbox-01): `scripts/bootstrap-k2-dedicated.sh` (Standard
  provision + libkrun sandbox stack, GPLv2 **consent gate**
  `K2_SANDBOX_CONSENT=1`, sha-verified artifacts, 8-check acceptance
  incl. real microVM spawn) and `scripts/package-dedicated-artifacts.sh`
  (artifact publisher).
- Daemon call-home: `app/api/servers/callback/route.ts` flips rows to
  `online`; 15-min stale-provisioning watchdog exists.

**Deliberate stub (the seam we replace):**
`executeDedicatedCreate` in `provision-sweep/route.ts` claims a pool box
then **throws `TerminalJobError("dedicated-personalize: manual step
required (V1)")`** and emails ops. And nothing anywhere orders, rescues,
or images a bare-metal machine — grep for `robot-ws|installimage|rescue`
across both cloud repos returns zero source hits.

**The gap, precisely:** (a) personalize a claimed pool box without a
human; (b) acquire + image + bootstrap replacement boxes without a
human; (c) reclaim/wipe boxes safely. Everything else exists.

## 2. Architecture decisions (settle these before any code)

### D1 — Personalization is PULL, not push.
Do not build "the cloud SSHes into customer boxes." Instead: pool boxes
are pre-bootstrapped (Standard+sandbox stack installed, no tenant state)
and run a tiny **assignment poller** (systemd timer, on-box) that polls
`GET /api/pool/assignment?box=<id>&key=<pool-key>` every 30s. On
assignment it receives {tunnel_token, subdomain, owner_user, callback}
— the same inputs `provision-k2-server.sh` already takes — applies them
locally (the personalize steps are a thin wrapper over existing script
functions), disables its own poller, deletes the pool key, and
call-homes. Why pull wins:
- No fleet-wide SSH key held by the web app (steal it = own every box).
- Vercel serverless cannot hold long SSH sessions anyway (see P-V1).
- It reuses the proven call-home trust direction (box → cloud).
The pool key is per-box, single-purpose, and dies at handoff — this is
the PRD-§4.2 "provisioning key removed at handoff" made concrete.

### D2 — Replenishment runs where a long-lived process can live.
Robot orders take minutes→hours (auction) or hours→days (new AX/EX).
The Vercel cron can ORCHESTRATE (state machine in DB, one tick per
minute) but the SSH-heavy steps (rescue, installimage, bootstrap) need a
runner with real sessions. Use the k2-connect relay box (it already
runs our Rust control plane 24/7) as the **replenish runner**: a small
worker binary (or even a supervised shell runner) that consumes
`replenish_queue` rows and executes §4 steps. Keep Robot credentials ON
THE RUNNER (systemd EnvironmentFile), never in Vercel env, never in the
repo.

### D3 — Assigned boxes are never returned to the pool without a full
re-image. Reclaim = rescue → wipe (installimage) → re-bootstrap → then
`pool_state='stock'`. A returned box holds a customer's disk; there is
no shortcut. (Pre-mortem P3.)

### D4 — Auction first, catalog later. The Server-Market (auction) API
gives instant-ish delivery (~15min–2h) and lower prices, matching the
pool model. Fixed-catalog AX ordering can come later if auction supply
of the target spec dries up. The target spec is a CONFIG value
(min cores/RAM/NVMe, max €/mo) — never hardcoded, because auction
inventory shifts daily.

## 3. The Robot API (what the junior needs to know before touching it)

- Base `https://robot-ws.your-server.de`, HTTP **Basic auth** with the
  dedicated webservice credentials (`ROBOT_USER`/`ROBOT_PASSWORD` — on
  the ops box at `~/.config/hetzner-robot/credentials`, mode 0600; a
  runner-local EnvironmentFile in production). These are SEPARATE from
  the hcloud Cloud token (`lib/hcloud.ts` world) — two different Hetzner
  products, two different APIs, do not mix.
- **Rate limit is brutal: ~200 requests/hour.** Design every loop
  around it: cache product listings, poll order status at minutes-scale
  backoff, batch nothing per-request in a hot loop. Blowing the limit
  locks you out for the hour (pre-mortem P6).
- Endpoints v1 needs: `GET/POST /order/server_market/product` (auction
  list/order), `GET /order/server_market/transaction/<id>` (delivery
  status → server number), `GET /server` + `GET /server/<num>`,
  `POST /boot/<num>/rescue` (activate rescue, returns root password —
  treat as a SECRET, never log), `POST /reset/<num>` (power cycle),
  `POST /key` (manage SSH keys so rescue comes up key-authed instead of
  password where possible), `DELETE /boot/<num>/rescue`.
- Auction orders cannot be cancelled once confirmed and the box bills
  monthly from delivery — ordering bugs cost real money (P7).

## 4. The build, step by step

### Step 1 — `lib/robot.ts` (k2-dev-web) + runner twin
Typed client mirroring `lib/hcloud.ts`'s shape (thin fetch, explicit
types, error taxonomy). Web app uses it READ-ONLY (status displays);
all MUTATING calls live in the runner (D2). Unit tests with recorded
fixtures — never hit the live API in CI.

### Step 2 — replace the personalize stub (the "one-click" moment)
- New table columns: `pool_key_hash`, `assignment` JSON on `servers`.
- `executeDedicatedCreate`: after `claimPoolServer` succeeds, write the
  assignment payload, transition job to `awaiting-box-pickup` (NEW state,
  not terminal), stop throwing.
- New route `app/api/pool/assignment/route.ts`: box polls with its pool
  key; constant-time hash compare; returns assignment exactly once
  (single-use flip, CAS).
- On-box: `k2-pool-agent.sh` + systemd timer, baked by the (updated)
  bootstrap when run with `K2_POOL_MODE=1`. Applies assignment via the
  existing provision functions; self-destructs; daemon call-home flips
  the row `online` through the EXISTING callback route — after this
  step, a paying customer gets a Dedicated box with zero humans, as
  long as stock exists.
- The 15-min watchdog stays; add `awaiting-stock` dashboards.

### Step 3 — replenish pipeline (runner on the relay box)
State machine per box (DB rows in `replenish_queue`, every transition
appended to `server_events`):
`ordering → delivered → rescue-requested → imaging → bootstrapping →
acceptance → stocked | failed(reason)`.
- `ordering`: pick cheapest auction product matching the CONFIG spec
  under the price cap; place order; store transaction id.
- `delivered`: poll transaction (minutes-scale backoff) until a server
  number + IP exist.
- `rescue-requested`: register our provisioning SSH key via `/key`,
  activate rescue with that key, `POST /reset`, wait for SSH on the
  rescue system.
- `imaging`: drive `installimage` NON-interactively via an autosetup
  file (Ubuntu 24.04, **`SWRAID 1` + wipe ALL drives** — auction boxes
  arrive with previous RAID metadata and dirty disks; see P4), reboot,
  wait for SSH on the installed system (host key WILL change —
  provisioning known_hosts is per-box, not global).
- `bootstrapping`: run `bootstrap-k2-dedicated.sh` with
  `K2_SANDBOX_CONSENT=1 K2_POOL_MODE=1` + artifact env. The consent
  flag is legally load-bearing (GPLv2 gate): it is OUR consent for OUR
  pool box; the CUSTOMER's consent was captured at checkout
  (`metadata.sandbox_consent`) and must remain recorded on the order.
- `acceptance`: the script's own 8-check pass must exit 0; then remove
  the provisioning key from the box (leave only the pool-agent
  identity), flip `pool_state='stock'`.
- Trigger: pool watermark check in the existing provision-sweep cron
  (target: N_stock ≥ 1, configurable) enqueues `ordering` and ALSO
  emails ops (the PRD-§8 "✅ automation ⚠ human alert" posture stays —
  a human watches money-spending automation, v1).

### Step 4 — reclaim/wipe (D3)
`destroy` for a Dedicated row: revoke subdomain + tunnel token
(existing paths), then enqueue `reclaim` → rescue → installimage wipe →
re-bootstrap → `stocked`. Until built, reclaim stays a documented
manual runbook — but the DESTROY path must at minimum mark the box
`pool_state='dirty'` so nothing can ever re-assign it un-wiped.

### Step 5 — website finishing touches
- Flip `DEDICATED_CHECKOUT=on` only after Steps 2+3 have run end-to-end
  on a real order in staging-mode (Stripe test + one sacrificial box).
- Buy page: honest delivery copy — "usually instant (pooled), up to N
  hours during high demand" driven by live stock count.
- Dashboard: surface replenish/stock state to ops; surface
  `awaiting-stock` to the affected customer with the refund option
  (P2).

## 5. Money & margin guardrails

- Price cap per auction order = CONFIG, reviewed monthly against the
  $99 tier (target box cost ≤ ~€44/mo per the pricing memo; alert if
  the cheapest qualifying auction box exceeds the cap — do NOT
  auto-order above it).
- Hetzner bills the box from delivery whether or not a customer has it
  → pool size IS carrying cost; start with watermark 1.
- Customer cancels → we still pay until Hetzner cancellation takes
  effect; reclaim-to-pool is usually better than cancelling the box
  unless pool is over watermark.

## 6. Secrets & safety rails (non-negotiable)

- Robot credentials: runner EnvironmentFile only. NOT Vercel env, NOT
  the repo, NOT logs. The webservice user should be limited to what the
  runner needs.
- Rescue passwords / provisioning keys: secrets in memory and per-box
  files on the runner; scrubbed from logs (`***`), deleted at handoff.
- Every state transition writes `server_events` — this is the audit
  trail for "why did we buy a box at 3am."
- The runner refuses to place an order if >M orders happened in the
  last 24h (runaway-loop circuit breaker, CONFIG, default 3).

## 7. PRE-MORTEM — "one-click bare metal failed. What happened?"

- **P1. "We treated bare metal like cloud."** The implementer modeled
  ordering as synchronous (hcloud-style create→ready in 10s) and the
  sweep timed out / double-ordered. → The §4.3 state machine is the
  design; every step is resumable from DB state; the ONLY synchronous
  thing is a state transition. Test: kill the runner mid-`imaging`,
  restart, pipeline resumes without a duplicate order.
- **P2. "Customer paid, no stock, silence."** AwaitingStock held the
  order open for days with no comms; chargeback + reputation hit. →
  Stock-out surfaces to the CUSTOMER (dashboard + email at T+1h with
  ETA and one-click refund), not just ops. Charging model stays
  charge-at-checkout only because refund is one click — if that ever
  changes, switch to authorize-then-capture-at-online.
- **P3. "We re-rented a box with the last customer's disk."** A
  reclaimed box went back to stock without re-imaging ("it was only
  assigned for an hour"). This is a DATA BREACH, not a bug. → D3 is
  absolute; `pool_state='dirty'` is the default on any release of an
  assigned box, and `dirty → stock` has exactly one path: through the
  wipe pipeline. Test: attempt to claimPoolServer a dirty row — must be
  impossible at the query level.
- **P4. "installimage ate itself on auction hardware."** Previous RAID
  metadata / odd drive ordering / an NVMe that enumerated differently
  made half the fleet image wrong. → autosetup always wipes ALL drives,
  asserts expected drive count/size BEFORE imaging (delivered specs are
  in the transaction record), and `acceptance` (the bootstrap's 8-check
  pass) is what stocks a box — imaging "success" alone stocks nothing.
- **P5. "The cloud held SSH keys to every customer box."** Personalize
  was built push-style for expedience; the key leaked or the serverless
  runtime timed out mid-session, half-personalized boxes everywhere. →
  D1 pull model; the web app literally has no route to reach a customer
  box. Review check: no SSH library in k2-dev-web's dependency tree,
  ever.
- **P6. "Rate-limited into blindness."** A 10s status-poll loop burned
  the ~200 req/h Robot budget; then NOTHING (orders, resets, rescue)
  worked for an hour, mid-provision. → Minutes-scale backoff everywhere;
  a shared client-side budget counter; alert at 60% burn.
- **P7. "The runaway loop bought seven servers."** A retry bug
  re-entered `ordering` on every sweep tick; auction orders are
  non-cancellable; ~€300/mo of surprise. → The §6 circuit breaker +
  idempotency: `ordering` rows carry the transaction id from the FIRST
  attempt; re-entry resumes polling, never re-orders. Test: force-fail
  the order-status call 10× — exactly one order exists.
- **P8. "Rescue root passwords in the logs."** The rescue activation
  response (contains the password) was logged verbatim by generic
  request logging. → Client redacts the field at the deserialization
  boundary; grep CI check for the password field name in log output.
- **P9. "GPLv2 consent evaporated."** Boxes got the sandbox stack but
  nobody could prove the customer consented (or OUR consent flag was
  cargo-culted into a script default). → Checkout stores
  `sandbox_consent` in Stripe metadata AND the servers row (already
  built); the bootstrap keeps refusing without the env flag; the
  assignment payload carries the customer's consent timestamp.
- **P10. "Dirty IP, dead deliverability."** An auction box arrived with
  an IP on spam blocklists; the customer's agents couldn't reach half
  the internet and we debugged K2 for a week. → `acceptance` includes a
  blocklist check (Spamhaus/SORBS lookups) + outbound-connectivity
  probe set; fail → keep the box for replacement/return, don't stock.
- **P11. "Vercel was the worker."** Someone moved rescue/installimage
  into a serverless function; 10-minute limit truncated imaging;
  zombie boxes. → D2. The web app orchestrates STATE; the runner does
  WORK. If a step needs a persistent session, it belongs on the runner.
- **P12. "Nobody noticed automation stopped."** The runner died in
  March; the pool drained; April's first Dedicated order sat
  awaiting-stock for a weekend. → Watermark breach + runner heartbeat
  are paging alerts (ops email exists; add the heartbeat row the sweep
  checks). Silence is never success (same lesson as the CI monitors).

## 8. Open questions for Rosson

1. Pool watermark (recommend 1 to start — carrying cost vs instant
   delivery) and the auction price cap number.
2. Refund SLA copy for stock-outs (P2) — auto-refund after how long?
3. Should reclaimed-box wipe (§4.4) block v1 launch, or is manual
   reclaim + the `dirty` guard enough for the first N customers?
   (Recommend: the guard blocks launch; the automation can follow.)
4. Runner placement: relay box (D2) vs a dedicated tiny ops VM —
   relay is simpler, but it couples tunnel infra with buy-side
   automation. Recommend relay for v1, revisit at scale.
