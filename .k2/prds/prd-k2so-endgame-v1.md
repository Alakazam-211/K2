# PRD: K2SO Endgame v1 — the full crossover, staged by dependency not by date

**Status:** approved direction (Rosson, 2026-07-07). Written as a COOKBOOK:
a future implementer should be able to execute any stage from this document
alone, without re-deriving the analysis. Companion to
`prd-k2so-cleanup-v1.md` (Phase 1 shipped @ e2b03a3: home-path exodus +
CI gate; Phase 2 = identifiers/branding).

Stages are ordered by DEPENDENCY, not bound to specific version numbers —
hotfixes and unrelated releases can interleave freely. The only ordering
law is §0. Line numbers below are as of commit e2b03a3; they will drift —
the grep commands given with each edit are the authoritative locator.

## 0. The one governing rule

**Never rename a load-bearing thing in the release that makes it
load-bearing.** Every risky item is a two-step:

    Stage N:   ship the TOLERANT READER (new name preferred, old honored)
    Stage N+1: flip the WRITER to the new name
    Stage N+2: (optional) retire the old-name reader

A stage ships only when the previous stage has ≥1 release of clean field
time (no support reports attributable to the lane). Lanes are independent
— any lane can pause or stop permanently without blocking the others.

## 1. Lane map

| Lane | Item | End state |
|---|---|---|
| L1 | `k2so.db` filename | rename → `k2.db` (or KEEP, §8) |
| L2 | `agentType='k2so'` DB value | migrate → `'k2'` |
| L3 | `~/.k2so` compat symlink | stop creating on FRESH installs |
| L4 | Dual-reads (env `K2SO_*`, `.k2soignore`, frpc legacy probe) | retire old side |
| L5 | Compat readers for OLD installs | keep until fleet is upgraded (§7) |
| L6 | frps proxy name `k2so-<sub>` | KEEP — relay contract (§7) |

---

## 2. STAGE A — tolerant readers (ship any time after cleanup-v1)

### A-L1: DB path resolvers accept both names
Locator: `git grep -n 'k2so\.db' -- crates cli`

1. `crates/k2-core/src/db/mod.rs` (~:152, fn that builds the DB path):
   ```rust
   // BEFORE
   let db_path = db_dir.join("k2so.db");
   // AFTER — prefer the new name IF IT EXISTS; else legacy. Writer is
   // NOT flipped in this stage: when neither exists (fresh install),
   // still create k2so.db so a Stage-A-only release changes nothing.
   let db_path = {
       let new = db_dir.join("k2.db");
       if new.exists() { new } else { db_dir.join("k2so.db") }
   };
   ```
   Same treatment at `db/mod.rs:~868` (`base.join("k2so.db")` — test/aux
   path builder).
2. `cli/k2` — three sites (`:~2780` python `os.path.expanduser`,
   `:~3647`/`:~3651` sqlite3 shell): mirror the same
   "k2.db if exists else k2so.db" resolution:
   ```sh
   K2_DB="$K2_HOME/k2.db"; [ -f "$K2_DB" ] || K2_DB="$K2_HOME/k2so.db"
   ```
3. Unit test (k2-core): create temp dir with only `k2.db` → resolver
   picks it; with only `k2so.db` → picks that; with BOTH real files →
   pick the one with more live `projects` rows (exclude `_orphan` /
   `_broadcast`; size fallback; tie → `k2so.db`). **Do not** blindly
   prefer `k2.db` — that hid live `k2so.db` behind a stub (2026-08-25).
   Stage B's boot guard §3-L1 still must not delete either file; after
   B, `k2so.db` is a symlink and is not a dual-real case.

### A-L2: agentType readers accept 'k2' as synonym of 'k2so'
Locator: `git grep -rn "\"k2so\"\|'k2so'" -- crates src | grep -i agent`
Current comparison sites (all must route through ONE helper):
- `crates/k2-core/src/workspace/agent_identity.rs:~284`
- `crates/k2-core/src/workspace/migrations.rs:~59`
- `crates/k2-core/src/skills/content.rs:~1154`
- renderer: `src/renderer/stores/heartbeat-sessions.ts:~167`,
  `stores/tabs.ts:~1201`, `:~2140`,
  `components/Settings/sections/ProjectsSection.tsx:~2470`
```rust
// k2-core: add next to the agent_type definitions
/// Endgame L2: 'k2' and legacy 'k2so' are the SAME agent type. All
/// comparisons go through here so the Stage-B value migration can't
/// strand a reader.
pub fn is_builtin_agent_type(t: &str) -> bool { t == "k2" || t == "k2so" }
```
```ts
// renderer equivalent (single util, e.g. src/renderer/lib/agent-type.ts)
export const isBuiltinAgentType = (t?: string) => t === 'k2' || t === 'k2so'
```
Post-edit check: `git grep -n "== \"k2so\"\|=== 'k2so'" -- crates src`
returns ZERO (everything routes through the helper).

### A exit gate
- `cargo check --workspace` with `RUSTFLAGS="-D warnings"`; full vitest;
  `bash scripts/k2so-gate.sh`.
- Manual: on a dev box, `mv ~/.k2/k2so.db ~/.k2/k2.db` (daemon stopped!),
  boot, exercise app + `k2` CLI. Then move it back.
- ≥1 release of field time before Stage B.

---

## 3. STAGE B — writer flips

### B-L1: boot-time DB rename (THE careful one)
Location: daemon boot, in `crates/k2-core/src/db/mod.rs` inside the
open-path fn, BEFORE any connection opens. Exact algorithm:
```rust
let new = db_dir.join("k2.db");
let old = db_dir.join("k2so.db");
if !new.exists() && old.exists() {
    // Atomic same-volume rename; siblings must move with it.
    std::fs::rename(&old, &new)?;
    for ext in ["-wal", "-shm"] {
        let o = db_dir.join(format!("k2so.db{ext}"));
        if o.exists() { let _ = std::fs::rename(&o, db_dir.join(format!("k2.db{ext}"))); }
    }
    // Compat symlink: user scripts/agents that memorized
    // `sqlite3 ~/.k2/k2so.db` keep working (SQLite resolves symlinks).
    #[cfg(unix)]
    let _ = std::os::unix::fs::symlink(&new, &old);
    // Support marker: when/what/by which version. NEVER deleted.
    let _ = std::fs::write(db_dir.join("db-renamed.json"),
        format!(r#"{{"from":"k2so.db","to":"k2.db","daemon":"{}"}}"#,
                env!("CARGO_PKG_VERSION")));
} else if new.exists() && old.exists()
    && !old.symlink_metadata().map(|m| m.file_type().is_symlink()).unwrap_or(false) {
    // BOTH real files — never guess which is live. Keep running on the
    // resolver's pick (k2.db per Stage A) but scream:
    log_error!("[db] BOTH k2.db and k2so.db exist as real files — \
                refusing to touch either; investigate before deleting");
}
```
Fresh installs: flip the Stage-A fallback so `k2.db` is created when
NEITHER exists. The Stage-A resolver line becomes:
```rust
let new = db_dir.join("k2.db");
let db_path = if !new.exists() && db_dir.join("k2so.db").exists() {
    /* pre-rename boot, resolver still honors legacy */ db_dir.join("k2so.db")
} else { new };
```
(The rename above runs first, so in practice this resolves to `k2.db`.)

### B-L2: agentType value migration
Normal numbered SQL migration (see `crates/k2-core/src/db/schema.rs` for
the migration table pattern — copy the latest `migration_00NN` shape):
```sql
UPDATE agents SET agent_type='k2' WHERE agent_type='k2so';
```
(Adjust table/column to the live schema — locate with
`git grep -n "agent_type" crates/k2-core/src/db/schema.rs | head`.)
Safe because Stage A's readers accept both values — including on a
DOWNGRADE to the Stage-A release.

### B-L4: drop the legacy frpc probe
`crates/k2-core/src/tunnel/connector.rs:~57`
(`v.push(home.join(".k2so/bin/frpc"));`) — delete the line + its comment,
remove the matching entry from `scripts/k2so-allowlist.txt`.

### B exit gate
- Upgrade test: copy a previous-release `~/.k2` fixture → boot RC →
  assert `k2.db` exists, `k2so.db` is a symlink, marker written, second
  boot is a no-op (marker unchanged, no re-rename).
- Old-binary probe: run the PREVIOUS release's `k2` CLI against the
  renamed home — must work via the `k2so.db` symlink.
- Fresh install: `k2.db` created directly; no `k2so.db` anywhere.
- ≥1 release of field time before Stage C.

---

## 4. STAGE C — symlink retirement (the true end; treat like a signing change)

Location: `crates/k2-core/src/migration_home.rs`. Two arms create the
symlink today — locator: `git grep -n "symlink(&new, &old)" crates/k2-core/src/migration_home.rs`
1. The FRESH-INSTALL arm (bottom of `run()`, returns
   `HomeMigration::FreshInit`, big 0.40.33 warning comment above it):
   delete ONLY the `symlink` call — keep `create_dir_all(&new)`.
2. The SELF-HEAL arm (~:109-114, heals a missing symlink on existing
   installs): KEEP for installs that ever had `.k2so`; guard it so it
   does NOT fire for post-Stage-C fresh installs (e.g. only heal when
   `~/.k2/migrated-from-k2so` marker exists — write that marker in the
   move arm; existing installs without the marker: healing stays ON for
   one more release, then re-evaluate).
3. The MOVE arm (real `~/.k2so` dir → move + symlink) stays FOREVER —
   pre-0.40.4 machines upgrading late still need it.
4. Update `migration_home.rs` tests: `fresh_install_creates_symlink`
   flips to asserting it does NOT (rename the test accordingly).

PRECONDITIONS (all, no exceptions):
- Stages A+B shipped, each with ≥1 release clean field time.
- k2so-gate green with NO allowlist growth throughout.
- Rosson's fresh-computer test on the RC: install → pair → terminals →
  hooks fire → heartbeat ticks → K2 Connect host works, with `~/.k2so`
  never existing. Plus upgrade-from-previous test.
- After one boot of a fresh install:
  `ls ~/.k2so` → "No such file or directory" AND everything above works.
- Signed-bundle launch test; release on Rosson's word only
  (feedback_high_blast_radius).

Rollback: one-line revert (restore the symlink call), patch release.

---

## 5. STAGE D — reader retirement (optional hygiene, no deadline)

Only after B+C field silence. Each is one edit + allowlist shrink:
- env fallbacks: `crates/k2-core/src/perf.rs:~56` (`K2SO_PERF`),
  `workspace/wake_prompts.rs:~265` + `k2-daemon/src/wake_headless.rs:~67`
  (`K2SO_TRACE_WAKE_SPAWN`) — drop the legacy `env::var`, keep `K2_*`.
- `.k2soignore`: `crates/k2-core/src/clone/inventory.rs:~166` — drop the
  legacy filename from the ignore-file candidate list, keep `.k2ignore`.
- Stage-A DB resolver fallback + Stage-B both-exist guard can simplify
  to `k2.db`-only once telemetry shows no `k2so.db` opens.
- L2 synonym helper: leave it — one string comparison is free, and DB
  backups restored from pre-Stage-B eras remain readable.

---

## 6. What each release cycle looks like (name-agnostic)

| Cycle | Ships | User-visible? | Abortable? |
|---|---|---|---|
| cleanup-v1 P1 (DONE @ e2b03a3) | path exodus + gate | no | shipped |
| cleanup-v1 P2 | branding strings, log prefixes, env dual-READS, `.k2ignore` dual-read, Tauri command/event renames | "K2" in menus | yes |
| Endgame A | tolerant DB/agentType readers | no | yes |
| Endgame B | DB rename + value migration + probe drop | no | yes (revert; symlink covers) |
| Endgame C | fresh installs get no `~/.k2so` | no (unless something was missed — that's what the preconditions are for) | yes (one-line revert) |
| Endgame D | dual-read retirement | no | yes |

Hotfixes and feature releases interleave freely between cycles; a cycle's
edits always ride WHATEVER release comes next after its gate passes.

## 7. Deliberate keepers (documented so nobody "cleans" them)

- **frps proxy name** — `crates/k2-core/src/tunnel/render.rs:~45`
  (`format!("k2so-{sub}")`, `"k2so-daemon"` fallback at :~43). This is a
  RELAY-VISIBLE contract: frps enforces one live proxy per NAME, and that
  same-name conflict is the single-holder tunnel property the 2026-07-07
  nsi cutover depended on (old holder dies → ~30s reap → new holder
  claims THE SAME NAME). Renaming mid-fleet lets an old and a new daemon
  register DIFFERENT names for the same subdomain — an undefined fight.
  Rename only as a coordinated relay+client project with its own PRD.
- **L5 compat readers**: `migration_launchd.rs:~30-32/93-95`
  (`com.k2so.*` labels the RETIREMENT code must name),
  keychain legacy reads (`tunnel/lease.rs:~69`,
  `src-tauri/commands/secrets.rs:~204`, `connect-host.ts:~49/63`,
  `K2ConnectSection.tsx:~60`, `companion/keychain.rs:~14`),
  `k2so-daemon` binary-name discovery of old installs. Retire when the
  fleet does, not when the codebase feels tidy.
- **`K2SO-notarize` keychain profile** — build-machine local; keep.
- **Migration modules** (`migration_home.rs`, `dot_dir_migration.rs`,
  `migration_launchd.rs`) — migration code names the past by definition.
- **Internal thread names** `k2so-frpc-supervisor` etc.
  (`connector.rs:~421/546/625`) — debugger labels; rename freely in
  cleanup-v1 P2 or leave; zero risk either way.

## 8. Acceptable end states (decide at leisure)

1. A+B+C+D complete — only §7 keepers remain, each with a reason comment.
2. A+B+C — dual-readers linger (invisible, zero risk).
3. A+C only — `k2so.db` keeps its name forever behind an allowlist entry.
The gate + `scripts/k2so-allowlist.txt` is the single source of truth for
"what remains and why" at every point in time.

## 9. Verification kit (copy-paste per stage)

```sh
RUSTFLAGS="-D warnings" cargo check --workspace
npx tsc --noEmit && npx vitest run
bash scripts/k2so-gate.sh
# residual scan (expect only allowlisted + §7 keepers):
git grep -InE '~/\.k2so|\$HOME/\.k2so|home(_dir\(\))?\.join\("\.k2so' -- src-tauri crates src cli scripts
# post-boot machine scan (fresh + upgraded):
grep -r k2so ~/.k2 ~/.claude/settings.json ~/.cursor/hooks.json 2>/dev/null
```
Fresh-install matrix: macOS + Linux deb — pair, spawn terminal, hooks
fire (run a claude turn), heartbeat ticks, tunnel host reachable.
Cross-version: RC client ↔ previous-release remote host, and inverse.
