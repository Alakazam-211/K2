# PRD: Server Migration v1 — move a whole K2 server between machines

**Status:** Draft for Rosson review · 2026-07-07
**Owner:** Rosson
**Driver:** migrate `nsi.k2.dev` off its old Mac mini onto `k2-dedicated-01`
(Hetzner Robot #3032615, 78.46.104.118, Ubuntu 24.04 RAID1) — and every future
Mac-mini→Dedicated / box→box move after it.

---

## 1. Problem

K2 has no way to move a *server* — only workspaces. "Clone to" is per-workspace
and deliberately identity-free, which is correct for cloning but fatal for
migration:

1. `/cli/clone/unpack` registers the folder as a **new** project (fresh
   `projects.id`, no machine-specific state carried — by design, see
   `crates/k2-core/src/clone/mod.rs`).
2. The destination daemon generates a **new** `~/.k2/tunnel-key.pem` on first
   boot (`tunnel::tls::load_or_generate_keypair`). The federation fingerprint
   is SHA-256 of that key's SPKI — so the migrated box presents an unknown
   identity and every pinned peer rejects it (`require_peer → UnknownPeer`).
3. The trust store (`~/.k2/federation-peers.json`) and the connection gate
   rows (`workspace_remote_connections`) don't travel either.

Net: after a clone, agent-team connections are dead in both directions.
Migration must carry the identity verbatim; that is the whole feature.

## 2. Product shape decision: K2-only v1, cloud later

**Recommendation: build this in K2 (daemon + `k2 migrate` CLI) now; do NOT
block it on a cloud surface.** Rationale:

- Migration is inherently local-daemon work: it reads disk, checkpoints
  SQLite, exports OS-keychain items (user-present prompts on macOS). The cloud
  can only ever *orchestrate* the same daemon primitive — so the daemon
  primitive is the v1 either way (daemon-first house rule).
- The nsi migration is needed now; a cloud "show my federated Mac minis"
  surface needs new substrate (daemon→cloud inventory reporting, consent
  model, staleness) and belongs to the existing **Fleet Console** arc
  (`project_fleet_management_now`). Migrate-a-server becomes a button on that
  console in a later phase — same daemon API underneath, zero rework.
- Not showing Mac minis in the cloud today is a feature gap, not a migration
  blocker: v1 migrations are operator-driven (you, on the machines involved).

Phase 3 below reserves the cloud surface so the API is designed for it.

## 3. Identity model — what moves, what regenerates, what re-auths

**Carry VERBATIM (this is "the server"):**
| What | Where | Why |
|---|---|---|
| Federation/tunnel keypair | `~/.k2/tunnel-key.pem` (+ `tunnel-cert.pem`) | fingerprint = identity; peers pin it |
| Peer trust store | `~/.k2/federation-peers.json` | who WE trust |
| Tunnel config incl. `device_id` + subdomain + token | `~/.k2/tunnel.json` | same `device_id` ⇒ lease continues seamlessly (single-holder lease keyed by device_id, 3-min TTL) |
| The database | `~/.k2/k2so.db` (after `wal_checkpoint(TRUNCATE)`) | projects, workspaces, `workspace_remote_connections`, agent presets/config, api_keys |
| Connect users + sessions | `~/.k2/connect-users.json`, `connect-sessions.json`, `seed-users.json` | argon2 logins survive |
| Settings/themes/hooks/skills state | `settings.json`, `themes/`, `hooks/`, `whats-new.state`, `federation-seen.json`, `federation-outbox/`, `inbox/`, `heartbeats/`, `sessions/` | continuity |
| Workspace trees | wherever `projects.path` points | includes each workspace's `<project>/.k2/` (AGENT.md, skills, heartbeats) |
| Claude-side state | `~/.claude/projects/<slug>/memory/` + session `.jsonl` (reuse clone inventory) | agent memory/history |

**REGENERATE on target (never copy):** `daemon.token`, `daemon.port`,
`heartbeat.port`/`heartbeat.token`, `tunnel-https.port`, logs,
`clone-tmp/`, `downloads/`, `sandbox-*` (rebuild), `bin/` (arch-specific —
frpc etc. re-fetched).

**RE-AUTH on target (cannot move silently):** OS-keychain items —
K2 Connect account refresh token (`dev.k2.connect.account`), companion
password hash (`K2SO-companion-auth`), Claude/agent OAuth
(`dev.k2.claude-auth`, `~/.claude/.credentials.json`), remembered
remote-host tokens. v1 = a printed **re-auth checklist** at the end of
import; macOS→Linux additionally noted because Linux lease renewal via
account session isn't wired yet (`lease.rs::read_account_session` is
mac-only — tunnel bearer token in `tunnel.json` still works).

**REMAP:** every `projects.path` and `workspaces.worktree_path` is absolute
(`/Users/nsi/…` → `/home/k2/…`). Import applies a prefix map (interactive
confirm or `--map /Users/nsi=/home/k2`), then runs the existing
`clone/repair.rs` slug-repair per project so Claude session/memory dirs match
the new paths. Unmappable paths → listed, project marked `needs-attention`,
never silently dropped.

## 4. UX / flow

### `k2 migrate export [--out <file>]` (source box)
1. Preflight: daemon healthy, DB `wal_checkpoint(TRUNCATE)`, warn on active
   PTY sessions (they will not survive — history does).
2. Quiesce: pause agents/heartbeats, stop accepting new sessions.
3. Bundle: tar.zst of the verbatim-set above + `manifest.json`
   (schema_version, source hostname/OS/arch, daemon version, fingerprint,
   subdomain, project list with paths+sizes, workspace-tree
   include/exclude decisions). Workspace trees included by default;
   `--no-workspaces` for rsync-separately mode (big repos).
4. Output: single file, 0600. **Contains private key + tokens ⇒ treat like a
   key.** Optional `--encrypt` (age-style passphrase) in P1.

### `k2 migrate import <bundle> [--map A=B]...` (target box)
1. Preflight ("doctor"): fresh-daemon check (refuse to overwrite an
   already-federated `~/.k2` without `--force`), OS/arch report, required
   agent CLIs present? (claude/codex per agent_presets), disk space, secret
   backend present (Linux: Secret Service or file-fallback note).
2. Restore verbatim set → remap paths → regenerate ports/tokens →
   slug-repair → re-fetch arch binaries (frpc).
3. Start daemon. Same `device_id` claims the lease within one 60s renewal
   cycle ⇒ `nsi.k2.dev` now routes here. Peers see the same fingerprint ⇒
   trust intact, agent teams reconnect.
4. Print verification report + re-auth checklist.

### `k2 migrate verify` (target, and runnable anytime)
Checks: lease held (subdomain → this box), fingerprint matches manifest,
each `workspace_remote_connections` row dials successfully, each project
path exists + `<project>/.k2/agent` intact, users file loads, agent CLIs
runnable. Exit non-zero on any failure (tests fail loudly).

### Cutover ordering (the dangerous part)
**Guarantee: migration NEVER deletes or modifies source data (Rosson,
2026-07-07).** Export is read-only apart from the WAL checkpoint; the old
machine keeps everything. Decommissioning the previous device (destroying a
rented Linux box, retiring a Mac mini) is the user's separate, explicit
decision afterwards — never part of this flow.

Ordering maximizes how long the source stays live:
1. Export + transfer + import run **while the source is still fully live**
   (export is a snapshot; new state after the snapshot doesn't migrate —
   stated in the prompt).
2. **Offline verify** on the target before any switch: paths remapped, files
   present, users load, agent CLIs runnable — everything except lease +
   peer dials.
3. **Cutover is one atomic step:** source enters **tombstone mode**
   (tunnel/lease renewal + federation listener off, UI banner "This server
   migrated to <target>", state file `~/.k2/migrated.json` — a soft flag,
   zero data touched, daemon still usable locally) and the target daemon
   starts, claims the lease, passes **online verify** (lease held, peers
   dial).
4. Rollback at any point = stop target + `k2 migrate untombstone` on source
   — back to the exact pre-migration state, one command.

Why tombstone at all (vs. leaving both live): two daemons with the same
`device_id` fight over the lease (last-claimer-wins each 60s) and answer
federated peers inconsistently. Only one box can BE the identity at a time;
tombstone is the parked-but-intact state that makes split-brain impossible
by construction.

## 5. Phases

- **P0 — bundle + import + verify + tombstone (CLI, file transport).**
  Operator moves the bundle with scp/rsync. Enough to migrate nsi.
  ~ the whole feature is here.
- **P1 — `k2 migrate to <ssh-host>` one-command.** Source-driven: streams
  bundle over SSH to a box that already ran the daemon installer
  (`install-daemon.sh`), triggers import, runs verify, tombstones itself on
  green. Add `--encrypt`. Add `--rsync-workspaces` for huge trees
  (bundle carries state; trees rsync separately with delta passes).
- **P2 — app UI.** Settings → "Migrate this server…" wizard = thin trigger
  over the same daemon routes (`/cli/migrate/*`, POST-only guards).
- **P3 — cloud/Fleet Console surface.** Daemons report inventory (name, OS,
  version, fingerprint, online) to K2 Connect; console lists your Mac minis
  + cloud boxes; "Migrate" pairs two of YOUR daemons and orchestrates
  P1 over the tunnel instead of SSH. Folds into
  `project_fleet_management_now`; not designed further here.

## 6. Risks / edges

- **Bundle is radioactive** (private key + bearer tokens): 0600, delete after
  import, never through third-party storage; P1 encrypts by default.
- **Live sessions die at export.** Stated upfront in the CLI prompt.
- **Same-subdomain flap during TTL window:** worst case 3 min (lease TTL)
  of relay pointing at the tombstoned source; acceptable, document it.
- **mac→Linux realities:** case-sensitive FS (flag case-colliding paths in
  doctor), CRLF-safe nothing to do, launchd→systemd handled by installer,
  Linux Secret Service may be absent on headless boxes (fall back to 0600
  files — same trust level as `tunnel.json` today).
- **Agent CLIs are not migratable** (OAuth per machine). Doctor lists what
  needs `claude login` etc. on the target before agents run.
- **Sandbox state** (`sandbox-homes/`, overlays) is arch-specific → rebuilt,
  not moved.

## 7. Non-goals v1

Zero-downtime/live migration; partial migration (that's Clone); Windows;
multi-server merge; automated DNS changes (subdomain follows the lease —
no DNS work needed for *.k2.dev tunnels); **any deletion/cleanup of the
source machine** (P2's app wizard may OFFER "wipe this server's K2 data" as
a separate post-verify action, but it is never automatic).

## 8. The nsi runbook = P0 dry-run

We migrate nsi BY HAND first, following §4 step-for-step with rsync + sqlite3
+ a path-remap script, capturing every surprise in the arc memory. What the
hand-run proves painful is what P0 automates. The feature ships when a second
migration is one command.
