# Wave 0 — Session identity foundation (handoff)

**Branch:** `feat/wave0-session-identity`  
**Stack (oldest → newest):**

1. `1662c0f` — **PR-A** scoped passport at agent spawn; no owner token in env; `K2_HOOK_SCOPED` defaults **ON**
2. `be6b37b` — **PR-B** CLI never re-sources owner token for ID/locked tools
3. `b1e4d56` — **PR-C** `project=` is never identity when a scoped principal is present

**Verified (combined tree):** 40 daemon lib tests + 29 CLI auth script checks, all green.

---

## Contract for DNS / mail teams

### Identity

| Caller | Who they are |
|--------|----------------|
| Validated **scoped** principal (UDS / `require_hook`) | `HookPrincipal.workspace_uuid` (minted at spawn) |
| Owner token / app (no principal) | Owner residual — may still use `project=` claim until Phase 2 |
| Spoofed `project=` under principal | **Ignored** for self-identity |

Helper: `crates/k2-daemon/src/caller_workspace.rs` → `resolve_caller_workspace`.

```rust
// After require_hook / cell stamp:
let ws = resolve_caller_workspace(Some(&principal), Some(client_project_claim))?;
// grants key off ws.workspace_uuid — NOT the claim
```

### Registering a capability (DNS / mail)

1. Add path prefixes to `session_token::is_agent_verb` (allowlist) if agents use the cell channel.
2. Deny owner-only admin surfaces on that allowlist (mail pattern: `is_agent_verb_denies_mail_owner_surfaces`).
3. Handlers **params-driven**; stamp principal via `stamp_principal` / `with_request_principal`.
4. Resolve caller with `resolve_caller_workspace` — never raw `project=` as identity.
5. Optional: catalog entry in `k2_core::cli_tool_policy` (Open / ID / locked). DNS/mail default **ID + locked**.

### CLI

- In-cell ID/locked verbs: scoped Bearer + UDS preferred; **no** `heartbeat.token` inversion.
- Missing passport → exit **3** + teaching text (Settings → CLI Tools).
- Catalog defaults are **static** in `cli/k2` today (not live Settings overrides).

### Ops

| Flag | Effect |
|------|--------|
| `K2_HOOK_SCOPED` unset / `1` | Default **ON** — mint + UDS |
| `K2_HOOK_SCOPED=0` | Opt out mint/UDS; still strips owner from hook env keys |

---

## Residual (do not oversell)

1. **Disk** `~/.k2/heartbeat.token` still owner-readable same-uid (Phase 2 / R8).
2. **Owner token + `project=`** still spoofs without a scoped principal (documented test `claim_only_still_resolves_for_owner_path`).
3. Companion may still carry owner token over tunnel (out of Wave 0).
4. Settings → CLI Tools UI + launch-bar sandbox Option-click may live on other WIP branches; not required for this stack.

---

## Suggested DNS next steps

1. Merge `feat/wave0-session-identity`.
2. `k2 dns` routes under `/cli/dns/*` with `resolve_caller_workspace`.
3. Toggles keyed on principal grants, not claimed workspace string.
4. Regression: scoped principal for workspace A + claim B cannot mutate B’s zones.
