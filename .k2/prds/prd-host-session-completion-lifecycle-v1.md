# Host-session completion lifecycle — PRD v1

**Status:** **Draft / ready for implementation review**  
**Owner:** K2 daemon + `k2` CLI  
**Requester:** Rosson (product); Julie / Scout (async pilot consumers)  
**Related:** `sandbox_reaper.rs`; `prd-v1-host-session-kill-v1.md` (implemented); `prd-host-session-hung-orphan-cleanup-v1.md`; `prd-host-session-cli-startup-hang-v1.md`; `docs/agent-contract.md`; Scout dual-signal (Supabase done-marker + K2 lifecycle)

---

## 1. Goal

Make host-session **completion** explicit, flexible, and fully cleaned up:

1. Agents can declare **done** without using the **respond content** channel.  
2. **Grace** after done is **integrator-configurable** (warm multi-turn vs fast teardown).  
3. When grace ends, the daemon reaps **process + live session map + UI tab events** via the **same chokepoint as `/kill`**, not a shallower `sess.kill()`-only path.  
4. Agents that **never** call `respond --final` or `done` may keep running (**accepted for now**); hung cleanup is a later / parallel PRD.

---

## 2. Background (code-grounded gaps)

### Today

| Step | Behavior |
|------|----------|
| `k2 respond` (non-final) | Append to message ring; reaper stays **Working** |
| `k2 respond --final` | Message + `on_respond_final` → **Grace** (~**10s hardcoded**) |
| Grace expiry (`sandbox_reaper`) | `lookup_by_session_id` → **`sess.kill()` only** + drop reaper REG entry |
| Full teardown | `v2_session_map::unregister(agent_name)`: kill + map remove + `SessionRemoved` + DB active_terminal cleanup |
| Explicit kill | **Uses full `unregister`** — fuller than Grace reaper |

### Gaps

1. **Respond is the only completion signal** — wrong for agents that write elsewhere (Scout direct-write, batch jobs) and never use the API message ring.  
2. **Grace is fixed at 10s** — multi-turn chat that wants a warm session after a “turn done” cannot ask for longer idle-after-complete.  
3. **Grace reaper ≠ kill path** — tabs / map / events can lag or stick if ChildExit is delayed or missed (observed: many live `api-…` tabs under Scout `sales-interview`).

---

## 3. Non-goals (v1)

- Auto-reap agents that never call `done` / `--final` (review later; hang PRD + Scout reconciler).  
- Soft model cancel (“please stop thinking”).  
- Replacing Scout Supabase done-marker (product completion for Scout stays app-side).  
- Deleting durable `workspace_tab_sessions` history required for resume identity (optional soft-delete later).  
- A2A as the sole completion protocol (may feed the same hook later).

---

## 4. Product decisions

### D1 — Two completion CLIs (content vs lifecycle)

| CLI | Content to integrator drain | Lifecycle |
|-----|----------------------------|-----------|
| `k2 respond "…"` | Yes, non-final | Stay **Working** |
| `k2 respond --final "…"` | Yes, final message | Enter **Grace** |
| **`k2 done`** (new) | No product payload required (optional system ack) | Enter **Grace** (same reaper path) |

- Agents that only need lifecycle: call **`k2 done`**.  
- Agents that need to answer the API: `--final` (implies done).  
- Calling both is OK (idempotent Grace re-arm).  
- **No respond/done → session may remain forever** (v1 acceptable; document it).

### D2 — Integrator-chosen grace (`grace_secs`)

| Field | On spawn (and optionally message-live body later) |
|-------|-----------------------------------------------------|
| Name | `grace_secs` (alias docs: completion grace) |
| Default | **10** (current behavior) |
| Clamp | **0 … 86400** (0 = reap ASAP after grace arm; large = keep warm) |
| Semantics | After `--final` or `k2 done`, `grace_until = now + grace_secs` |
| Cancel | Inject / message-live / non-final `respond` → **Working**, grace cleared |
| Separate from | `timeout_secs` (client/JWT budget; **not** Working hard wall, 0.40.81+) |

**Intent:** long-lived chat can set e.g. `grace_secs: 1800` so a turn’s `--final`/`done` leaves the session warm for follow-ups; one-shot jobs keep `10` or `0`.

### D3 — Full reap on grace expiry (same as kill)

When Grace expires, the daemon **MUST**:

1. Resolve `agent_name` for the daemon `SessionId` (same helper as host-session kill).  
2. Call **`v2_session_map::unregister(agent_name)`** (force; no subscriber guard).  
3. `sandbox_reaper::unregister` for all known id aliases.  
4. Emit the same events kill/unregister already emit (`SessionRemoved`, activity idle, active recompute).  

**MUST NOT** rely only on `sess.kill()` + hoping ChildExit runs unregister (current gap).

Idempotent if already unregistered / dead (mirror kill `not_live` semantics internally).

### D4 — Cost / spend unchanged

Long `grace_secs` keeps a process alive → still subject to integrator **kill** and any **cost/spend** policy. Document that warm grace is not free.

### D5 — Scout dual-signal (no conflict)

| Consumer | Signal |
|----------|--------|
| Scout product | Supabase done-marker (their async plan) |
| K2 runtime | `k2 respond --final` **or** `k2 done` |

Agent authors targeting Scout async should do **both** product marker and K2 done/`--final`. K2 does not read Supabase.

### D6 — Wire / CLI surface

**Spawn / resume body (additive):**

```json
{
  "prompt": "…",
  "timeout_secs": 600,
  "grace_secs": 300
}
```

**CLI:**

```bash
k2 done
k2 done --reason complete   # optional, logged only
```

Implementation: host-session cells already have `K2_HOOK_*`; `k2 done` → `POST /cli/respond` with `final=true` and empty/minimal text **or** dedicated `POST /cli/session/complete` that only calls `on_respond_final`. Prefer **one reaper entrypoint** `mark_complete(session_id)` shared by respond-final and done.

**Store `grace_secs` on reaper Entry** at `register` / update on stamp if body carries override.

---

## 5. Implementation sketch

1. **`sandbox_reaper::Entry`:** add `grace_secs: u64` (from register).  
2. **`on_respond_final`:** use `e.grace_secs` instead of `FINAL_GRACE_SECS` constant (keep constant as default).  
3. **Grace tick:** replace `sess.kill()` with kill-equivalent **unregister** path (extract shared `force_teardown_host_session(session_id)` used by kill + reaper).  
4. **`k2 done`:** thin CLI → complete endpoint → `mark_complete`.  
5. **v1 host spawn:** parse `grace_secs`, pass into `register`.  
6. **Tests:**  
   - grace_secs=0 completes and full unregister within one tick.  
   - grace_secs=60 + inject before deadline → Working, not reaped.  
   - `--final` and `done` both arm grace.  
   - After grace, map lookup miss + SessionRemoved once.  
7. **Docs:** agent-contract + envelope + public API page (`grace_secs`, `k2 done`, lifecycle).

---

## 6. Acceptance

| # | Criterion |
|---|-----------|
| A1 | After `--final` or `done`, when grace elapses, session is **not** in `v2_session_map` and process is dead. |
| A2 | UI/subscribers receive **SessionRemoved** (or equivalent) without requiring a separate `/kill`. |
| A3 | `grace_secs` from spawn is honored (± one reaper tick). |
| A4 | Inject during grace cancels reap and accepts another turn. |
| A5 | Agent that never calls done/final is **not** auto-reaped by this PRD (by design v1). |
| A6 | Headless: no webview required. |

---

## 7. Out of scope / follow-ups

| Item | Where |
|------|--------|
| Startup-hang detect/auto-kill | `prd-host-session-cli-startup-hang-v1.md` |
| Stalled-hung heuristics | `prd-host-session-hung-orphan-cleanup-v1.md` |
| Scout reconciler + cost cap | Scout staged async plan |
| A2A task-complete → `mark_complete` | Later optional producer |
| Configurable grace on message-live mid-session | Nice-to-have v1.1 |

---

## 8. Risks

| Risk | Mitigation |
|------|------------|
| Long grace + high spawn rate = more RSS | Document; cost cap + kill; hang auto-kill PRD |
| Agents forget done/final | Integrator poll + Scout reconciler; document contract |
| Double unregister race | Idempotent unregister + kill |

---

## 9. Ship note

This PRD **agrees** with Rosson:

- Configurable post-completion grace for warm multi-turn.  
- Full reap of session/terminal/tab path (via unregister), not process-only.  
- Separate **`k2 done`** when work isn’t a respond conversation.  
- No forced auto-reap if they never complete-signal (review later).
