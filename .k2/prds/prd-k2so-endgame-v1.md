# PRD: K2SO Endgame v1 — the full crossover, staged by dependency not by date

**Status:** approved direction (Rosson, 2026-07-07): "clean up all of that
and still make sure the app is running correctly." Written to be followed
WHENEVER — stages are ordered by dependency, not bound to specific
releases. Companion to `prd-k2so-cleanup-v1.md` (Phase 1 shipped @ e2b03a3:
path exodus + gate; Phase 2 = identifiers/branding).

## 0. The one governing rule

**Never rename a load-bearing thing in the release that makes it
load-bearing.** Every risky item is a two-step:

    Stage N:   ship the TOLERANT READER (new name preferred, old honored)
    Stage N+1: flip the WRITER to the new name
    Stage N+2: (optional) retire the old-name reader

A stage ships only when the previous stage has ≥1 release of clean field
time (no support reports attributable to the lane). Any lane can pause or
stop permanently without blocking the others — the lanes are independent.

## 1. Lane map (what's left after cleanup-v1 Phases 1–2)

| Lane | Item | End state |
|---|---|---|
| L1 | `k2so.db` filename | rename → `k2.db` (or KEEP, §7) |
| L2 | `agentType='k2so'` DB value | migrate → `'k2'` |
| L3 | `~/.k2so` compat symlink | stop creating on FRESH installs |
| L4 | Dual-reads (env `K2SO_*`, `.k2soignore`, frpc legacy probe) | retire old side |
| L5 | Compat readers for OLD installs (launchd `com.k2so.*` retirement code, keychain `com.k2so.connect.*`/`K2SO-companion-auth` read paths, `k2so-daemon` binary discovery) | keep until fleet telemetry says zero old installs; cheap forever |
| L6 | frps proxy name `k2so-<sub>` | KEEP (relay contract, §6) unless a coordinated relay change is ever scheduled |

## 2. Stage A — tolerant readers (safe to ship any time after cleanup-v1)

- **L1a:** every DB-path resolver (daemon `db::db_path`, the 3 `cli/k2`
  sites, any tooling) resolves `~/.k2/k2.db` IF IT EXISTS, else
  `~/.k2/k2so.db`. Writer unchanged (still creates `k2so.db`). Ships with
  a unit test per resolver.
- **L2a:** every consumer of `agentType` accepts `'k2'` as a synonym of
  `'k2so'` (single normalization helper at the read seam; no scattered
  `== 'k2so'` comparisons survive — grep-verified).
- **L4a:** already true for env vars and `.k2soignore` after cleanup-v1
  Phase 2 (new name wins, old honored). No action.
- Exit criteria: gates green; a DB manually renamed to `k2.db` on a dev
  box runs the full app/CLI surface; one release of field time.

## 3. Stage B — writer flips

- **L1b:** daemon boot, BEFORE opening the DB: if `k2.db` absent and
  `k2so.db` present → `rename()` `k2so.db` + `-wal` + `-shm` (same
  volume, atomic, no copy). Write `~/.k2/db-renamed.json` marker
  (timestamp, old/new, daemon version) for support. Fresh installs create
  `k2.db` directly.
  - Guards: refuse (and keep old name) if a `k2.db` ALREADY exists
    alongside `k2so.db` (never guess which is real — log loudly, keep
    running on `k2so.db`, surface in `k2 doctor`).
  - **Compat symlink `k2so.db → k2.db` is created at rename time** so
    user scripts/agents that memorized `sqlite3 ~/.k2/k2so.db` keep
    working (SQLite resolves symlinks; WAL siblings live next to the real
    file). Retire with L3's timetable, not before.
- **L2b:** SQL migration `UPDATE ... SET agent_type='k2' WHERE
  agent_type='k2so'` (normal numbered migration; L2a readers make
  downgrade-tolerance a non-issue).
- **L4b:** drop the legacy frpc probe (`connector.rs`), remove its
  allowlist entry.
- Exit criteria: upgrade test (0.40.36-era home → this release) AND fresh
  install AND a deliberately-old CLI binary against the renamed DB (works
  via the `k2so.db` symlink); one release of field time.

## 4. Stage C — symlink retirement (L3, the true end of the era)

- `migration_home.rs`: fresh-install arm stops creating `~/.k2so`;
  existing installs KEEP their symlink forever (never delete user-visible
  paths). The self-heal arm (0.40.35's fix) also retires for
  fresh-install cases only.
- Preconditions (ALL required):
  1. Stages A+B shipped with clean field time.
  2. k2so-gate has been green (no new allowlist growth) the whole time.
  3. Rosson's fresh-computer test on the RC build: install → pair →
     terminals → hooks fire → heartbeat ticks → K2 Connect host works —
     with `~/.k2so` never existing.
  4. `grep -r k2so` over a fresh install's `~/.k2` + CLI configs after
     one boot: zero hits.
- This is the stage most like 0.40.33 — treat its RC like a
  signing/updater change (feedback_high_blast_radius): signed-bundle
  launch test mandatory, release on Rosson's word only.

## 5. Stage D — reader retirement (optional hygiene)

- Drop `K2SO_*` env fallbacks, `.k2soignore` read path, the `k2so.db →
  k2.db` symlink creation for NEW renames, and the L2a synonym reader.
- Only after telemetry/support silence through Stages B+C. There is no
  deadline; these readers cost bytes, not risk.

## 6. Deliberate keepers (documented so nobody "cleans" them)

- **frps proxy name `k2so-<sub>`** — relay-visible contract. Same-name
  registration conflict IS the single-holder tunnel property (the
  2026-07-07 nsi cutover depended on it: old holder dies → ~30s reap →
  new holder claims). A rename would let old+new daemons register
  DIFFERENT names for the same subdomain during mixed-version windows —
  undefined fight. Rename only as a coordinated relay+client project
  with its own PRD; default is keep forever.
- **L5 compat readers** — they service machines running old versions;
  they retire when the fleet does, not when the codebase feels tidy.
- **`K2SO-notarize` keychain profile** — build-machine local, never
  ships; rename is cosmetic churn with signing-pipeline risk. Keep.
- **Migration modules** (`migration_home.rs`, `dot_dir_migration.rs`,
  `migration_launchd.rs`) — migration code names the past by definition.
  Allowlisted permanently.

## 7. Explicit "acceptable end states"

Full crossover does NOT require every lane to finish. These end states
are all acceptable, in Rosson's preference order to be decided later:
1. A+B+C+D complete — only §6 keepers remain, each with a reason comment.
2. A+B+C — dual-readers linger (invisible, zero risk).
3. A+C only — `k2so.db` keeps its name forever behind an allowlist entry
   (users never see it; the gate documents the choice).
The gate + allowlist is the single source of truth for "what remains and
why" at every point.

## 8. Verification kit (reusable per stage)

- `scripts/k2so-gate.sh` green (pattern widens as lanes land).
- Fresh-install matrix: macOS + Linux (deb) — pair, spawn, hooks,
  heartbeat, tunnel host.
- Upgrade matrix: previous-release home dir → RC; assert markers/renames
  happened exactly once (second boot = no-op).
- Cross-version: RC client ↔ previous-release remote host, and inverse.
- Old-binary probe: previous release's `k2` CLI against an RC home dir.

## 9. Rollback per stage

- A: revert commit (readers only — nothing observable changed).
- B: daemon refuses nothing on downgrade — old daemon finds `k2so.db`
  missing BUT the compat symlink resolves it; worst case rename back by
  hand (documented in the marker file). agentType: L2a readers in the
  prior release already accept both values.
- C: re-enable symlink creation in a patch release; no data touched.
- D: re-add a reader; no data touched.
