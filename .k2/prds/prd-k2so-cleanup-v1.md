# PRD: K2SO Cleanup v1 — retire the legacy name safely (0.40.37)

**Status:** approved (Rosson, 2026-07-07) — "fully clean up all the references
and make sure the app is running correctly."
**Owner:** release session. **Ships in:** 0.40.37 (Phase 1+2), with the
symlink retirement deliberately deferred to a later release (§7).

## 1. Why now, and why carefully

0.40.33 removed the `~/.k2so → ~/.k2` compatibility symlink on the belief it
was cosmetic. Every fresh install broke (dead UI: the app reads pairing files
through hardcoded `~/.k2so` paths). 0.40.35 restored the symlink. The lesson:
`k2so` references are not cosmetic until *proven* cosmetic, one by one.

This PRD is that proof. A full agent-audited catalog (2026-07-07) found:
- **~35 real `~/.k2so` filesystem reader/writer sites** (the dangerous class)
- ~16 stale strings (messages say `.k2so`, code already uses `.k2`)
- 3 misleading Settings UI paths, 6 visible "K2SO" brand strings
- ~250 Tauri command/event names crossing Rust↔JS as strings
- A handful of persisted/external contracts that must NOT be renamed blindly

## 2. Invariants (the "don't mess this up" rules)

I1. **The `~/.k2so` symlink keeps being created in 0.40.37.** Zero behavior
    depends on it after this release, but external things (user scripts,
    agents' memorized paths, third-party CLI configs not yet re-injected)
    may. Retirement is §7, a LATER release.
I2. **No persisted value is renamed without a migration.** DB enum
    `agentType='k2so'`, `k2so.db` filename, keychain service names,
    launchd labels: all stay (allowlisted) in 0.40.37.
I3. **No external contract breaks.** Env vars and ignore-files get
    dual-read (new name wins, old honored). Nothing that a user's machine
    or script supplies stops working.
I4. **Every phase lands with green gates** (tsc, vitest full, cargo check
    -D warnings) before the next phase starts.
I5. **The CI gate ships in the same release** as the cleanup, so regression
    is structurally impossible, not culturally discouraged.

## 3. Phase 1 — the path exodus (the 0.40.33 bug class, killed)

### 3.1 Canonical helper
`k2_core::paths::k2_home() -> PathBuf` = `home_dir().join(".k2")` (panic-free
fallback mirrors existing db_dir behavior). src-tauri gets the same via
k2-core. Existing ad-hoc helpers (`k2_dir()` in federation/peers.rs,
tunnel/tls.rs, federation/outbox.rs; `k2so_dir()` in daemon_client.rs,
commands/connect_hosts.rs) are re-pointed to it and renamed `k2_home`.

### 3.2 The 35 real sites (from the catalog; file:line at audit time)
Fresh-install-critical (fix + test first):
- pairing reads: src-tauri lib.rs:104-105, daemon_events.rs:183-184,
  commands/daemon.rs:254-255
- notify.sh template: src-tauri agent_hooks.rs:216-217 (embedded body)
- heartbeat.sh + plists: k2-core heartbeats/install.rs:142-144, 256, 465
- frpc: src-tauri lib.rs:554 (stage), k2-core tunnel/connector.rs:57 (probe
  — keep the `.k2so` probe as a LAST candidate for one release, after `.k2`)
- claude-auth-refresh: k2-daemon claude_auth_host.rs:212, 336
Lower risk:
- templates/skills: lib.rs:763, skill_layers.rs:28, skills/content.rs:139
- awareness: ingress.rs:61 (+ rename /tmp/.k2so-awareness-inbox →
  /tmp/.k2-awareness-inbox with old-path fallback read for one release)
- hook detection fragments: src-tauri agent_hooks.rs:354,403,462,493;
  k2-core agent_hooks.rs:394; k2-daemon misc_routes.rs:563 (become part of
  the §3.4 migration, not just detection)
- dev tooling: scripts/kessel-*.ts ×5, cli/k2so-test, fetch-frpc.sh:148

### 3.3 Embedded script re-staging
notify.sh, heartbeat.sh, claude-auth-refresh script + both plists are
REGENERATED artifacts. Each carries a version marker; bump every marker so
one boot on 0.40.37 rewrites all on-disk copies with `.k2` paths. Acceptance:
after first boot, `grep -r k2so ~/.k2/hooks ~/.k2/*.sh` is empty.

### 3.4 CLI-config rewrite migration (the sneakiest item)
Users' own Claude/Cursor/Gemini config files contain the injected path
`~/.k2so/hooks/notify.sh`. On hook registration (runs at every app boot),
when the existing entry contains the `.k2so` fragment: rewrite it in place
to `~/.k2/hooks/notify.sh` (same entry shape, path swap only). Idempotent;
detection fragments then accept BOTH paths for one release. Acceptance:
boot once → configs contain no `.k2so`.

### 3.5 Stale + misleading strings
All ~16 stale message strings → `.k2`. The 3 misleading Settings paths
(LocalLLMSettings ×2, GeneralSection ×1) → real paths.

## 4. Phase 2 — identifiers and branding

- 6 visible "K2SO" strings (tray.rs tooltip/Quit, menu.rs title/About/zoom
  title/new-window title) → "K2".
- `[k2so]` log prefixes → `[k2]`. Rust fn/var renames (compiler-checked).
- `__k2soNativeZoom` → `__k2NativeZoom` (JS-only global).
- Env dual-read: `K2_PORT|K2SO_PORT`, `K2_PERF|K2SO_PERF`,
  `K2_TRACE_WAKE_SPAWN|K2SO_TRACE_WAKE_SPAWN`, `K2_BARE_TAB_PRUNE_THRESHOLD`
  (already K2_). New name wins; old logged as deprecated once per boot.
- `.k2ignore` honored alongside `.k2soignore` (union; `.k2ignore` wins on
  conflict — documented in clone/inventory.rs).
- **Tauri command/event names (the ~250):** renamed `k2so_*` → `k2_*` and
  `k2so:*` → `k2:*` ATOMICALLY (both sides, one commit per subsystem).
  Because these are strings, tsc cannot catch a miss. Verification is
  mechanical, not hopeful:
  (a) rename by exact-match sweep;
  (b) `git grep -nE "invoke\(['\"]k2so_|emit\(['\"]k2so:|listen\(['\"]k2so:"`
      must return zero;
  (c) the §5 gate counts EVERY residual `k2so` token against the allowlist;
  (d) runtime smoke of each renamed surface group (agents list, heartbeat
      list, inbox, sessions-for-workspace, whats-new event, selected-tabs).
  These names never cross a network or persist — same-binary contracts only
  — so a complete atomic rename is safe by construction. If ANY (b)/(c)
  residual survives review ambiguity, Phase 2 ships without that subsystem
  rather than with a guess.

## 5. The CI gate (ships with Phase 1)

`scripts/k2so-gate.sh`: `git grep -In "k2so"` (case-insensitive) across
src-tauri/ crates/ src/ cli/ scripts/, filtered against
`scripts/k2so-allowlist.txt` (exact `path:pattern` entries, each with an
inline reason). Non-allowlisted hit → exit 1. Wired into `checks` CI and
build-app.sh Step 0. The allowlist is the complete, reviewed list of
DELIBERATE survivors:
- `k2so.db` filename (db/mod.rs, schema.rs, cli) — data migration not worth
  the risk; revisit only with a future DB-format release
- `agentType 'k2so'` enum value — persisted; migration deferred
- legacy launchd labels `com.k2so.*` in migration_launchd.rs — the code that
  RETIRES them must name them
- legacy keychain services `com.k2so.connect.*`, `K2SO-companion-auth` —
  read-path compat for existing installs
- `k2so-daemon` binary-name references (process discovery of old installs)
- migration_home.rs / dot_dir_migration.rs — migration code names the past
- `.k2soignore` legacy read path (dual-read)
- comments that document history (pattern-scoped, not blanket)

## 6. Verification matrix (release blockers)

| Scenario | Check |
|---|---|
| Fresh install, no `~/.k2so` ever | pairs, terminals, hooks fire, heartbeat ticks, K2 Connect host works (Rosson, real machine) |
| Upgrade 0.40.36 → 0.40.37 | one boot rewrites scripts + CLI configs; `grep -r k2so ~/.k2 <cli configs>` clean; agents keep firing hooks |
| Symlink present (all real machines) | zero behavior change |
| Remote host on 0.40.36 (not yet upgraded) | client on 0.40.37 connects/works — no wire contract changed |
| Gates | tsc, vitest full, cargo check -D warnings, k2so-gate.sh green |
| Smoke | every §4(d) renamed surface exercised in the running app |

## 7. Explicitly deferred (future releases)
- Stop creating the symlink on FRESH installs (≥0.40.39, after ≥2 releases
  of §6-clean field time; existing installs keep theirs forever).
- `agentType 'k2so'` DB migration; `k2so.db` rename (maybe never).
- Removing `.k2soignore`/env/keychain legacy read paths (≥0.40.40).

## 8. Rollback
Phase commits are per-subsystem and revertible independently. No data
formats change; no wire contracts change; the symlink guarantees any missed
reader still resolves. Worst case = revert the offending commit and ship a
patch release — same-day recovery, no user data at risk.
