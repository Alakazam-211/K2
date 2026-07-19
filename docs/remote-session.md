# Remote Session (thin v1)

Safer-than-SSH, time-boxed, audited shell drive on a K2 device — **as the daemon user only, never root**. Default OFF. Owners enable a master switch, mint a one-time grant token, and drivers use that token to spawn and drive a login shell over the existing daemon/tunnel path.

This doc is the public-safe surface for owners and agents. Full product intent lives in the internal PRD: [`.k2/prds/prd-k2-remote-session-v1.md`](../.k2/prds/prd-k2-remote-session-v1.md) (gitignored on public clones — still the design home for maintainers).

---

## What it is

- A **consent-gated** way for an authorized remote principal (human agent, script, or another K2) to drive a **daemon-user shell** on a target box.
- **Time-boxed grants** (`k2rs_…` tokens) with revoke/disable kill paths.
- **Audited denials** — attempts while OFF (or without a live grant) are logged and visible on `status`.
- Transport reuses the daemon CLI routes (local or via K2 Connect `--host`), not a new network service.

## What it is not

| Not this | Reality today |
|---|---|
| OpenSSH published to the internet | No SSHD exposure; daemon still binds loopback; remote reach is tunnel/Connect |
| OS password login for agents | Agents use grant tokens, not `/etc/passwd` or sudo |
| Root / privilege escalation | PTY runs as the **K2 daemon user** only |
| Full setup / soul-transplant migration product | Thin substrate only — runbook automation is deferred |
| Runbook-scoped allowlists / path-jail | Stage 2 is **shell-only**; `runbook` scope returns not-implemented |
| Federation pairing as a shell gate | Device pairing / `k2 talk` is a separate relationship layer; **it does not gate** remote-session shell today |

---

## Auth layers

Checks are independent and fail-closed. An earlier NO short-circuits.

### 1. Device Connect username / password

Human login to the box UI (and owner/admin CLI token for admin verbs). This is how an **owner** reaches the target to enable the feature and mint grants. It is **not** the drive credential agents use for shell I/O.

### 2. Layer 0 — master wall (default OFF)

Per-device setting `remoteSessionsEnabled` / `remote_sessions_enabled`. Checked **first** on every drive attempt (`shell` spawn, and write/read on grant-bound PTYs).

- **OFF** → `403` with code `REMOTE_SESSIONS_DISABLED`, denial recorded, no PTY.
- **ON** → continue to grant checks.
- Owner can still **mint** grants while OFF; **use** is blocked until enable.

### 3. Grant token `k2rs_…` (drive credential)

Owner/admin mints a shell grant. The raw token is returned **once** (shown at create time only). The daemon stores a hash, never the secret again.

- Grant ids look like `rs_…`.
- Default TTL 30m; clamp 60s–24h.
- Only scope implemented: `shell`.
- Drive path: present `k2rs_…` on `shell` / `write` / `read`.

### 4. Optional later — federation pairing

Cross-device relationship / `k2 talk` style peering is a separate layer. **Do not treat pairing as authorizing remote-session shell** unless a future release wires that explicitly. Thin v1 gates on Layer 0 + live grant token only.

---

## CLI cookbook

Owner/admin verbs need an owner or admin token (local daemon token, or Connect session / `--token` / connect-tokens when using `--host`). Drive verbs prefer a live `k2rs_…` grant.

```sh
# ── On target device (owner) ──────────────────────────────────────────
k2 remote-session enable
k2 remote-session grant --ttl 45m --label "mac2linux"
# save the printed k2rs_… token once — it is not shown again

k2 remote-session status          # enabled?, grants, sessions, recent denials
k2 remote-session grants          # list grants (no secrets)

# ── Driver (local daemon, or --host target via Connect) ───────────────
k2 remote-session shell --token k2rs_… [--host …] [--cwd PATH]
# note sessionId from the response

k2 remote-session write <session-id> "ls -la" --token k2rs_… [--host …]
k2 remote-session read  <session-id> --lines 80 --token k2rs_… [--host …]

# ── Tear down ─────────────────────────────────────────────────────────
k2 remote-session revoke rs_…     # revoke grant; kills shells bound to it
k2 remote-session disable         # Layer 0 OFF; kills all remote shells
```

### Exit codes (CLI)

| Code | Meaning |
|------|---------|
| 0 | OK |
| 1 | Unexpected / transport |
| 2 | Usage / bad request |
| 3 | Teaching auth: `REMOTE_SESSIONS_DISABLED`, `NO_GRANT`, `GRANT_EXPIRED`, `GRANT_REVOKED`, owner-only |

### Useful flags

| Flag | Notes |
|------|--------|
| `--ttl 30m\|45m\|1h\|90s\|1800` | Grant lifetime |
| `--label "…"` | Human label on the grant |
| `--scope shell` | Only scope implemented (default) |
| `--cwd PATH` | Shell spawn working dir (default: daemon HOME; soft default only — no path-jail yet) |
| `--host HOST` | Remote daemon URL/host; reuses connect-tokens / `$K2_REMOTE_TOKEN` like `k2 talk` |
| `--token TOK` | Owner/connect token (admin) or `k2rs_…` (drive) |
| `--json` | Machine-readable `status` / `grants` |

---

## Security invariants

1. **Default OFF, fail-closed.** Missing decision = deny.
2. **Layer 0 is independent of grants.** A grant bug must not bypass the toggle.
3. **Denials are logged** (including while OFF) and appear on `status` / owner events.
4. **Revoke and disable kill.** `revoke` kills PTYs bound to that grant; `disable` kills all remote-session shells and turns the wall OFF.
5. **Grant ≠ password.** `k2rs_…` is a time-boxed drive credential, not the Connect user password and not an OS login.
6. **Daemon user only, never root.** No sudo path, no privilege escalation surface in this feature.
7. **Token shown once.** List/status never re-emit the raw grant secret.

---

## Migration intent

Remote Session is the **substrate** for laptop → server “soul transplant” and similar K2↔K2 ops: move `~/.k2`, agent creds, workspace data, etc. **without handing agents root SSH**.

What thin v1 unlocks now:

- Owner enables the wall and mints a short-lived shell grant.
- A driver (human or agent) runs **manual** steps over `shell` / `write` / `read`.
- Every attempt is auditable; the owner can cut the session instantly.

What comes later (runbook scope — not in thin v1):

- Signed allowlisted steps (`--scope runbook` + runbook file).
- Path-jail and tighter scope enforcement.
- Codified migration runbooks taught from **real** successful manual migrations, not invented up front.

**Practice:** start with manual steps over this shell; record what actually worked; only then promote steps into a runbook product.

---

## Related

- Internal PRD: `.k2/prds/prd-k2-remote-session-v1.md`
- K2 Connect (tunnel / remote UI): README **Remote Access — K2 Connect**
- Daemon routes: `/cli/remote-session/*` plus grant-aware `/cli/terminal/write` and `/cli/terminal/read`
- Implementation touchpoints: `crates/k2-core/src/remote_sessions.rs`, `crates/k2-daemon/src/remote_session_routes.rs`, `cli/k2` (`remote-session` verb)
