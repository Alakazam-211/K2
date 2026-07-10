# PRD: Cloud Server Upgrade v1 — "grow up" a K2 server to bigger/dedicated hardware

**Status:** direction from Rosson (2026-07-08). Nothing built. This is the
PRODUCT form of the nsi.k2.dev Mac-mini→Linux migration hand-run
(2026-07-07, see `project_server_migration_arc` memory + §9 lessons here).
Companion to `prd-server-migration-v1.md` (the K2-app peer-to-peer migration
CLI) and `prd-bare-metal-rental-v1.md` (the acquire-a-box pipeline this
consumes). Audience: future implementer, possibly junior — §9 (lessons
from the real run) and §10 (pre-mortem) are mandatory before code.

## 1. What it is (and why it's a great feature)

A K2 Cloud customer whose agents have outgrown their server clicks
**"Upgrade this server"** and K2 moves their ENTIRE server — identity,
workspaces, chat history, users, agent teams, the live subdomain — from
a smaller box to a bigger/dedicated one, with a short cutover window and
zero data loss. The old box is billed-off only after the customer
confirms the new one is healthy.

Common shapes:
- Standard $29 (Hetzner Cloud shared vCPU) → **Dedicated $99 (bare metal,
  sandboxes ON)** — the headline "your agents can now run microVM
  sandboxes" upsell.
- Dedicated → bigger Dedicated (more cores/RAM/NVMe).
- (Later) region move.

**Why it's easier than the nsi hand-run:** we control BOTH ends (both are
our provisioned boxes, both Linux, both have the daemon + our SSH access
+ the tunnel). The nsi run's hardest parts — a customer's Mac mini we
reached only over their LAN, macOS openrsync quirks, an orphaned launchd
daemon — DO NOT EXIST here. This is the friendly case.

## 2. The one hard invariant (why a naive "provision new + repoint DNS" fails)

A K2 server's IDENTITY is not its IP or subdomain — it is
`~/.k2/tunnel-key.pem` (SHA-256 of its SPKI = the federation fingerprint
peers pin) + `tunnel.json`'s **device_id** (owns the subdomain lease).
Spin up a fresh daemon and it generates a NEW keypair → every agent-team
peer that pinned the old fingerprint breaks with UnknownPeer, and the
subdomain lease fights (last-claimer-wins). So an upgrade is NOT
"provision a new server"; it is **transplant the identity onto new
hardware**. Everything in §4 follows from this.

## 3. Carry / regenerate / re-auth / remap (verbatim from the verified nsi contract)

**CARRY VERBATIM (byte-for-byte, this IS the server's soul):**
- `~/.k2/tunnel-key.pem` (+ cert) — the identity keypair
- `~/.k2/tunnel.json` — SAME device_id (lease continues) + subdomain
- `~/.k2/federation-peers.json` — agent-team trust (absent = no peers,
  simpler; present = MUST carry or teams break)
- `~/.k2/k2.db` (formerly k2so.db — do a `wal_checkpoint(TRUNCATE)` or a
  `.backup` snapshot first for a consistent copy): projects, agent
  config, `workspace_remote_connections`, users
- `~/.k2/connect-users.json` + sessions + seed-users.json — accounts
  (argon2 hashes ⇒ **all passwords carry**, users log in unchanged)
- `~/.k2/{settings,themes,hooks,outbox,inbox,sessions,heartbeats}/`
- workspace trees (each carries its own `<project>/.k2/` agent files)
- `~/.claude/projects/<slug>/` — memory + session `.jsonl` transcripts
  (chat history), `~/.claude/history.jsonl`, `~/.cursor/chats` if present

**REGENERATE / INSTALL on the new box (never copy):** `daemon.token`,
`*.port`, `bin/` (arch-specific — frpc + shims), `sandbox-*` state, AND
the **`k2` CLI itself**: `install -m 0755 cli/k2 /usr/local/bin/k2`
(version-matched to the daemon, or curl the raw from the release tag). It
lives OUTSIDE `~/.k2` so rsync never carries it, and a headless Linux
server has no desktop app to self-heal the symlink macOS does — so it
must be installed explicitly (see §9 L-cli).

**RE-AUTH (keychain-bound, cannot move — the operator/box redoes once):**
Claude auth for agents (`dev.k2.claude-auth`), K2 Connect account
session if used, companion pairing. NOTE: the customer's END USERS
re-auth NOTHING — their accounts + passwords + sessions all carry in
connect-users.json. Only box-side machine credentials re-auth.

**REMAP (paths change if the home path differs):** `projects.path` +
`workspaces.worktree_path` absolute prefixes, AND Claude transcript slug
DIRECTORIES (they encode the absolute path: `-home-alice-...`) + the cwd
strings embedded inside each `.jsonl`. For Cloud→Cloud both boxes use the
SAME service-user home (e.g. `/home/k2` or `/opt/k2`), so **the remap is
usually a NO-OP** — a huge simplification over nsi (which was
`/Users/alakazamlabs` → `/home/alakazamlabs`). Assert-zero-residual
regardless (see §9 L-remap).

## 4. The flow (hosted, both-ends-controlled)

Preconditions: customer owns server S (subdomain `sub.k2.dev`, box B_old).
Target: bigger box B_new.

1. **Acquire B_new** (reuse `prd-bare-metal-rental-v1`): for Dedicated,
   claim a pre-warmed pool box already bootstrapped with the sandbox
   stack; for bigger-Standard, provision from the golden snapshot. B_new
   comes up as a GENERIC box (daemon running, NO tenant identity yet) —
   exactly a pool box. Do NOT run the pool-agent personalize on it; an
   upgrade transplants identity instead of minting fresh.
2. **Pre-sync (no downtime):** with B_old LIVE, rsync the big cold data
   (workspace trees, ~/.claude) B_old→B_new. Runs minutes–hours; customer
   unaffected. (This is the nsi "staging" phase.)
3. **Arm cutover (no downtime):** install/verify frpc AND the `k2` CLI on
   B_new (`install -m 0755 cli/k2 /usr/local/bin/k2`; confirm `k2 help`
   round-trips to the daemon — §9 L-cli); stage the
   carried `~/.k2` identity files EXCEPT the live-lease bits held back;
   run an OFFLINE boot check on B_new with the tunnel DISABLED (frpc held
   aside) + lease off — daemon must reach `phase: ready`, DB integrity
   ok, project count matches. Park B_new (daemon stopped).
4. **Cutover window (short downtime, customer-scheduled):**
   a. Final incremental sync: fresh `k2.db` snapshot + ~/.claude + tree
      deltas B_old→B_new (QUOTE remote paths — §9 L-rsync).
   b. Tombstone B_old: stop its daemon, `disable` its unit, write
      `~/.k2/migrated.json`. VERIFY the daemon is actually dead AND its
      port is unbound (§9 L-tombstone).
   c. Start B_new's daemon WITH the tunnel → frpc registers `sub` via the
      carried tunnel.json token → `sub.k2.dev` routes to B_new within the
      frps reap window (~30–60s — §9 L-frps).
   d. Online verify: strict-TLS `boot-status` shows the new box + ready,
      a login works, a chat resumes.
5. **Confirm + bill:** customer (or ops) confirms healthy → update the
   `servers` row (provider/instance/region/plan), start billing B_new,
   STOP billing B_old. B_old goes `pool_state='dirty'` and enters the
   reclaim/wipe pipeline (never restocked un-wiped — bare-metal PRD D3).
6. **Rollback (any time before step 5 confirm):** stop B_new daemon,
   re-enable B_old's daemon → `sub.k2.dev` returns to B_old in the reap
   window. B_old was never touched destructively (§7).

## 5. Where it lives (build order, name-agnostic)

1. Cloud orchestration: a `server_upgrade` job type in the same
   provision/queue machinery (k2-dev-web + the relay runner for the
   SSH-heavy steps — same split as bare-metal PRD D2: web orchestrates
   STATE, relay runner does WORK over SSH). State machine mirrors §4:
   `acquiring → presyncing → armed → cutover → verifying → confirmed |
   rolled-back | failed`.
2. Reuse the bare-metal runner's SSH-step executor + the migration
   carry/verify logic (this is where the nsi hand-run's shell becomes a
   codified, tested runner step — see §8).
3. Dashboard: "Upgrade" on a server card → pick target tier/size → price
   delta + downtime estimate + schedule window → progress → confirm.
4. Billing: proration handled in the k2.dev web app (billing lives there,
   not the control plane).

## 6. Non-goals v1
- No live/zero-downtime migration (short cutover window is fine; agents
  are async — a 1–2 min gap is acceptable, unlike a web server).
- No cross-cloud (Hetzner→AWS) — same-provider fleet only.
- No automatic "we noticed you're maxed, auto-upgrade" — customer-
  initiated only (auto-suggest is a later, separate feature).
- No downgrade path v1 (bigger→smaller has disk-fit risk; separate PRD).

## 7. Trust / safety rails (same spine as the other cloud PRDs)
- **NON-DESTRUCTIVE (Rosson's standing rule):** the source box is never
  wiped until the customer confirms the target is healthy. Export is
  read-only; B_old stays a complete rollback target through step 5.
- **Single-holder identity:** never run both daemons with the tunnel up
  at once — the whole flow is ordered so exactly one holds the device_id
  at any instant (§9 L-frps explains the reap gap).
- **Secrets:** the carried tunnel-key.pem + connect-users hashes move
  over SSH between OUR boxes on the runner (never through the web app,
  never logged). Redact in transit like the bare-metal runner does.
- **Audit:** every state transition → `server_events`; the customer sees
  an honest timeline.

## 8. What to reuse vs build new
- REUSE: bare-metal acquire pipeline (§4.1), the relay SSH-step runner +
  its budget/breaker/redaction, the `dirty`→reclaim guard, the
  provision/queue+cron machinery, the callback contract.
- BUILD: the identity-transplant runner step (carry/verify/remap of §3 —
  codify the nsi shell), the `server_upgrade` job + state machine, the
  final-incremental-sync step, the Upgrade dashboard UI + proration.
- The `k2 migrate` CLI from `prd-server-migration-v1` shares the SAME
  carry/verify core — build it ONCE as a library both consume (the CLI
  is peer-to-peer/customer-driven; this is cloud-to-cloud/ops-driven).

## 9. LESSONS FROM THE REAL nsi RUN (2026-07-07 — do not relearn these)

- **L-tombstone: launchctl/systemctl "stop" can lie.** The mini's daemon
  was a launchd ORPHAN (no job loaded); `launchctl bootout` no-op'd and
  the daemon kept running + serving its port. Tombstone MUST `pkill -x
  k2-daemon` AND poll until the PORT is unbound — never trust the service
  manager's word. Also: `pgrep -f k2-daemon` matches your own SSH command
  line — use `pgrep -x`. (Hosted boxes run a real systemd unit so this is
  milder, but VERIFY-PORT-DEAD stays mandatory.)
- **L-frps: the ~30–60s takeover gap is normal, not failure.** After
  B_old dies, B_new's frpc gets "proxy [k2so-<sub>] already exists" until
  frps reaps the dead session, THEN "start proxy success". Cutover verify
  must WAIT ≥60s before declaring failure; during the gap curls hit the
  Caddy wildcard / get TLS resets. (This is also why the frps proxy NAME
  must stay `k2so-<sub>` — same-name conflict IS the single-holder
  mechanism; see k2so-endgame-v1 §7.)
- **L-rsync: quote remote paths with spaces.** openrsync (macOS) split
  `AI Projects/` into `AI` + `Projects` at the remote shell → files
  landed at the wrong path. Always single-quote the remote side:
  `rsync … "host:'/home/k2/AI Projects/'"`. Hosted Linux→Linux with
  no-space service paths is safer, but the codified step must quote
  regardless (customer workspace names contain spaces).
- **L-remap: assert zero residual, both DB and files.** After the path
  remap, prove it: `grep -rIl <old-prefix>` over ~/.claude = 0, and a
  DB dump | grep = 0. The Claude slug dirs AND the cwd inside each .jsonl
  both need rewriting (reuse `clone/repair.rs`). Cloud→Cloud same-home =
  usually a no-op, but ASSERT it rather than assume.
- **L-lease: Linux has no account-session lease renewal.** The mac-only
  `read_account_session` path isn't wired on Linux; the subdomain stays
  held via the tunnel.json BEARER token instead. The journal line
  "[tunnel/lease] renewal disabled" on the new box is EXPECTED, not a
  bug. Confirm the subdomain holds via tunnel-token registration.
- **L-consistent-db: snapshot, don't copy a live DB.** Use
  `sqlite3 .backup` (or wal_checkpoint TRUNCATE then copy) — a live
  `k2.db` mid-write copies torn. The nsi run took a `.backup` snapshot at
  cutover; do the same.
- **L-verify-before-tombstone: offline-boot the target FIRST.** nsi did a
  full parked boot check (frpc held aside, lease off) proving ready +
  DB-integrity + project-count BEFORE killing the source. Never tombstone
  on faith.
- **L-daemon-version: match or newer.** nsi's box ran a newer daemon than
  the mini; carried DB migrated forward cleanly. Ensure B_new's daemon
  version ≥ B_old's (never migrate DB backwards).
- **L-cleanup: remove transient provisioning keys post-cutover.** The
  temp migration SSH key was deleted from both ends after. The codified
  runner must scrub its per-job key at handoff (same as the pool-agent's
  single-use key).
- **L-cli: the `k2` CLI is INSTALLED, not carried — and Linux won't
  self-heal it.** On the nsi run the CLI never reached B_new: it lives at
  `/usr/local/bin/k2` (a symlink into K2.app on macOS; an `install`ed copy
  of `cli/k2` on Linux), NOT inside `~/.k2`, so rsync never carried it;
  the daemon self-stages only the `k2-open` shim, not the CLI; and a
  headless Linux box has no desktop app to re-link it at boot. Net: daemon
  + data present, `k2` command absent (found + fixed 2026-07-10 on nsi via
  `install -m 0755 cli/k2 /usr/local/bin/k2` — the CLI is a bash script
  that talks HTTP via `~/.k2/heartbeat.{port,token}`, so it works the
  instant it exists). DURABLE FIX (0.40.40+): have the daemon self-stage
  the `k2` CLI at boot the same way it stages `k2-open` (`open_shim.rs`
  `include_str!` pattern) so every Linux server self-heals and this step
  can never be skipped again.

## 10. PRE-MORTEM — "the upgrade feature lost someone's server"
- **P1. Both daemons live at once → subdomain war.** Ordering bug started
  B_new's tunnel before B_old was port-dead. → Step 4 is strictly
  sequenced; the runner asserts B_old port-unbound before B_new tunnel-up;
  test the interleaving.
- **P2. "Stopped" source wasn't stopped (L-tombstone).** → verify-port-
  dead is a required gate, not a log line.
- **P3. Declared failure during the normal frps gap (L-frps).** →
  ≥60s reap wait baked into verify; retries, not aborts, in the window.
- **P4. Wiped the source before confirm.** DATA LOSS. → §7 non-destructive
  rule is absolute; B_old→dirty→reclaim only AFTER step-5 confirm; the
  reclaim pipeline itself never restocks un-wiped (bare-metal D3).
- **P5. Torn DB copy (L-consistent-db).** → snapshot only; integrity-check
  on the target before cutover.
- **P6. Residual old paths broke chat history / agent cwd (L-remap).** →
  assert-zero-residual gate on both DB and ~/.claude; fail the job, don't
  ship a half-remapped box.
- **P7. Agent teams broke (identity not carried).** → tunnel-key.pem +
  federation-peers.json in the verbatim-carry set; a post-cutover check
  pings a known peer to confirm the fingerprint still matches.
- **P8. Customer billed for two boxes / neither.** → billing flip is
  atomic with step-5 confirm; a stuck job never silently double-bills
  (dashboard shows the in-flight state; proration in the web app).
- **P9. Rollback impossible because B_old was mutated.** → export is
  read-only; B_old's daemon is only STOPPED (reversible), never
  reconfigured, until confirm.
- **P10. Silent runner death mid-upgrade (bare-metal P12 twin).** →
  the upgrade job heartbeats; a stalled cutover pages ops AND surfaces to
  the customer with the rollback option.

## 11. Open questions for Rosson
1. Cutover downtime budget to advertise (nsi was ~a few min; agents are
   async so this is generous) — target copy?
2. Who triggers cutover timing — fully automated on schedule, or
   ops-confirmed each step for the first N upgrades (recommend the latter,
   same "first 2–3 by hand" posture as bare-metal)?
3. Proration model for the mid-cycle tier change (web-app billing).
4. Downgrade (bigger→smaller): separate PRD, or explicit non-offering?
