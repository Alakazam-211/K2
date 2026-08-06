# Host-session CLI startup hang — PRD v1

**Status:** **Open / draft for implementation** (investigation complete 2026-08-05…06; strace pin incomplete)  
**Owner:** K2 daemon / agent spawn path  
**Requester:** Julie / Scout pilot (reliability; hang rate ~10–20% staggered, was ~66% concurrent)  
**Related:** wiki `Bug - Host Session CLI Spawn Hang`; `prd-v1-host-session-kill-v1.md`; S9 work-completion reaper; Scout #113 retry (integrator mitigation)

---

## 1. Goal

Eliminate (or reduce to noise) the class of failures where a **host-session spawn returns success**, the agent **process is alive**, but the CLI **never becomes interactive** — no transcript, no LLM call, no `k2 respond --final` — until the **integrator** times out / retries.

**Success (product):** Under Scout-like concurrent cold starts, hang rate for “spawned but never-progressed” host-sessions is **&lt; 2%** over a 50-spawn canary, and when hang still occurs the daemon surfaces a **loud, actionable** failure (not a silent Working immortal).

---

## 2. Problem (verified)

### Signature

| Signal | Hung | Healthy |
|--------|------|---------|
| Process | Alive | Alive |
| CPU over minutes | Very low (~seconds CPU / minutes wall) | Rising |
| Transcript / scrollback | Empty / splash only | Growing |
| Network (TCP to model API) | Often never opened | Present after start |
| `bracketed_paste` / readline | Never | Present when ready |
| Integrator view | No `final:true` until client ceiling | Drain completes |

### Class (current root)

**Claude Code (and similar) startup-init deadlock / spawn-time race** under concurrent cold starts — **code-inherent**, intermittent.

Not: ambient credential as primary (zero TCP), missing Enter, sustained RAM starvation (reproduced at ~50 Gi free), Scout async logic.

### Integrator impact

- Scout poll ceiling (~290 s) fires → `k2_timeout` / `K2NetworkError`.  
- Always-on retry (#113) mitigates but **amplifies load** and can exhaust.  
- K2 Working-immortal reaper **will not** clean these up (never `--final`).

### Related latent race (separate ship)

`82107168` — bind per-cell `K2_HOOK_SOCK` **before** exec (connect-before-bind). Helps sessions that *reach* hook-connect; **does not** fix pre-REPL hang. Ship in normal release train; not the root for this PRD.

---

## 3. Non-goals

- Removing Scout’s Vercel sync-poll ceiling (Scout async-decouple).  
- Changing Working-immortal / Grace-after-`--final` product lock.  
- Fixing Claude Code upstream (we may work around; not block on Anthropic).  
- Full MCP/plugin redesign.

---

## 4. Product decisions

| # | Decision |
|---|----------|
| H1 | **Detect** never-progressed host-sessions in the daemon (not only integrator poll). |
| H2 | Detection signals (any sufficient; combine for confidence): no `bracketed_paste` / readiness within T_ready; no non-splash grid progress; no hook UDS peer connect; no respond entries; optional CPU/idle heuristic. |
| H3 | On detect: **loud** daemon log + optional event; **do not** report successful prompt inject if readiness never met (see inject honesty). |
| H4 | **Auto-action v1:** full teardown via shared **`force_teardown_host_session`** (same as `/kill` path, not bare `sess.kill()` only) and surface a loud failure the integrator can retry on — *or* leave kill to Scout reconciler once Stage 2 is live. **Daemon auto-kill default OFF until canary**, then on (`K2_HOST_SESSION_AUTO_KILL`); **integrator kill route remains SSOT**. See `prd-host-session-completion-lifecycle-v1.md` for happy-path Grace full reap. |
| H5 | **Inject honesty:** post-spawn inject must not claim `delivered=true` when readiness never arrived for RequireReady profiles (follow-up to latent inject work; secondary to hang root). |
| H6 | **Cold-start concurrency:** optional queue / stagger for simultaneous host-session cold spawns when load high (daemon-side cap). Does not replace root fix. |
| H7 | Tests: unit for readiness oracle; integration with fake agent that never advertises ready → detect path fires; healthy fake agent is not killed. |

---

## 5. Implementation sketch

1. **`host_session_startup_watch.rs`** (or extend reaper): after host-session register, arm a watch for T_ready (default 45–90 s, env override).  
2. Signals: `bracketed_paste_active`, scrollback growth, optional `k2 respond` seq.  
3. On timeout: log `[host-session] startup_hung session=…`; call same teardown as kill; mark list row dead.  
4. Optional: emit session_events / activity for Scout.  
5. Metrics: counter `host_session_startup_hung_total`.  
6. Ship `82107168` in same or prior release train (latent).  

---

## 6. Acceptance

| # | Test |
|---|------|
| A1 | 50 concurrent cold host-sessions on Scout-like box: hang rate ≤ 2% *or* every hang is detected + killed within T_ready + 30 s. |
| A2 | Healthy long silent gen (Working, no mid-output) is **not** classified as startup_hung. |
| A3 | Integrator can still `POST …/kill` any live session. |
| A4 | Headless: works with no webview open. |

---

## 7. Out of scope / later

- Continuous spend metering.  
- Scout done-marker protocol (Scout PR).  
- Public website docs (separate k2-dev-web work).

---

## 8. Open investigations

- Exact worker syscall (needs `sudo strace -ff` on a preserved hung PID).  
- Claude version pin / flags that reduce hang rate as interim.

**Ship signal:** green A1–A4 + Julie hang-rate drop on canary without retry amplification.
