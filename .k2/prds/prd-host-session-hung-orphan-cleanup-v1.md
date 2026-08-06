# Host-session hung / orphan cleanup — PRD v1

**Status:** **Open / draft for implementation**  
**Owner:** K2 daemon  
**Requester:** Julie / Scout (async-decouple + pilot load); Rosson (Working-immortal safety net)  
**Related:** `prd-v1-host-session-kill-v1.md` (implemented); `sandbox_reaper.rs` (Working/Grace); `prd-host-session-cli-startup-hang-v1.md`; Scout reconciler (Stage 2)

---

## 1. Goal

Provide a **daemon-safe** way to clean up host-session cells that are:

- **Alive as processes** but **never completed** (`k2 respond --final` / `k2 done` never seen), and  
- **Not** legitimately mid-long-generation (Working productively),

…without reintroducing a **hard wall on healthy Working** agents (0.40.81 product lock).

Integrator path **`POST …/kill` already exists**. Happy-path completion teardown (full `v2_session_map::unregister`, configurable grace, `k2 done`) is specified in **`prd-host-session-completion-lifecycle-v1.md`** — this PRD is the **stuck / never-signaled** path only.

**Product note (Rosson 2026-08-06):** agents that never call respond/done may keep existing for now; auto-reap of that class is this PRD + hang PRD, not forced into completion lifecycle v1.

---

## 2. Why this is needed

| Fact | Consequence |
|------|-------------|
| Working is immortal until `--final` | Hung never-final CLIs linger for hours |
| Scout #113 retry | Extra spawns; orphans accumulate (~300 MB each) |
| Async fire-and-forget (future) | No Scout 290 s auto-death; hung = forever without kill |
| Cost cap / reconciler | Need kill; daemon auto-clean reduces wedge risk |

---

## 3. Non-goals

- Killing healthy agents mid-long write (E-1 class).  
- Replacing Scout cost-cap policy.  
- Reaping canonical workspace sessions.  
- Soft “please stop” model cancel (v1 = force kill).

---

## 4. Definitions

| Term | Meaning |
|------|---------|
| **Working (healthy)** | Progress signals present: recent scrollback growth, mid `k2 respond`, paste ready + CPU, etc. |
| **Startup-hung** | Post-spawn, never progressed to interactive (see hang PRD). |
| **Stalled-hung** | Was ready / produced work, then no progress and no `--final` for T_stall. |
| **Orphan process** | Child still alive after map entry gone / reaper unregistered (should be rare). |

---

## 5. Product decisions

| # | Decision |
|---|----------|
| O1 | **Keep** Working immortal for **healthy** work (no `timeout_secs` wall). |
| O2 | **`--final` → Grace (~10 s) → reap** remains the happy path (already shipped). |
| O3 | **Integrator kill** remains SSOT for deliberate stop (`POST …/kill`). |
| O4 | **Daemon auto-cleanup** (optional):  
    - **Startup-hung** after T_startup (align hang PRD).  
    - **Stalled-hung** after T_stall with **no progress signals** (strict; false positives worse than orphans). |
| O5 | Auto-cleanup uses shared **`force_teardown_host_session`** (kill-parity unregister; see completion lifecycle PRD D3). |
| O6 | Emit loud log + optional activity event: `host_session_auto_killed` with reason `startup_hung` \| `stalled_hung`. |
| O7 | Env knobs: `K2_HOST_SESSION_STARTUP_HUNG_SECS`, `K2_HOST_SESSION_STALL_SECS`, `K2_HOST_SESSION_AUTO_KILL=0\|1`. **Default auto-kill OFF until canary**, then flip on (same matrix as hang PRD H4). |
| O8 | **Progress signals** (any keeps stalled timer reset): non-final respond; inject; scrollback growth above threshold; bracketed_paste first-seen. **Absence of all** for T_stall → candidate. |
| O9 | Silent long gen (no mid respond, no scrollback) is a **known hard case** — do **not** auto-kill on silence alone without progress definition agreed with Scout (prefer Scout reconciler + done-marker for v1 of *their* async; daemon stall auto-kill starts **startup-hung only** if silent-gen false positive risk is high). |

**v1 ship slice recommendation:** implement **startup-hung auto-kill** first (high confidence from pilot). Defer **stalled-hung** auto-kill until progress heuristics pass Scout silent-gen canary, or leave stalled entirely to Scout reconciler + kill.

---

## 6. Interaction with Scout async-decouple

| Scout Stage 2 reconciler | Daemon |
|--------------------------|--------|
| Detects no Supabase done-marker | May also detect startup-hung |
| Calls `POST …/kill` | Executes force stop |
| Retries spawn | — |

Daemon auto-kill is **defense in depth**, not a replacement for Scout’s product reconciler.

---

## 7. Acceptance

| # | Test |
|---|------|
| B1 | Fake agent that never becomes ready is auto-killed within T_startup + reaper tick (if auto-kill on). |
| B2 | Fake agent that eventually `--final` is Grace-reaped, not auto-killed early. |
| B3 | E-1 style continuous write for &gt; T_stall is **not** auto-killed (if stalled-hung enabled). |
| B4 | `POST …/kill` on already auto-killed session returns `killed: false, reason: not_live`. |
| B5 | Headless: no webview required. |

---

## 8. Docs

- Update `docs/host-session-capability-envelope.md` § reaper + kill.  
- Public website: timeout is **not** Working hard-kill; use kill + completion for spend/lifecycle (k2-dev-web).

---

## 9. Ship order

1. Hang **detection** + startup-hung auto-kill (this PRD v1 slice + hang PRD).  
2. Scout Stage 2 reconciler (their plan; uses kill).  
3. Optional stalled-hung heuristics after pilot data.  
4. `82107168` latent hook bind in normal release.

**Done when:** B1–B5 green + Scout confirms orphan RSS no longer grows unbound under retry load.
