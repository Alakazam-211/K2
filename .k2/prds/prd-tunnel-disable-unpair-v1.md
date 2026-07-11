# PRD: Tunnel Disable vs. Release Subdomain (Unpair) — v1

**Target:** 0.40.43 (after 0.40.42 email system). Approved direction
(Rosson 2026-07-11). Born from a real incident: a decommissioned machine
kept a valid tunnel identity, its orphaned daemon auto-reconnected for
days, and it repeatedly stole its old subdomain from the replacement
server during reconnect windows. The app's tunnel toggle didn't stop it
because the toggle was app-session state, not device state.

## 1. Problem

"Turn off the tunnel" today conflates two different user intents, and
persists neither strongly enough:

- **Pause**: "go dark for now, I'll be back" — should survive daemon
  restarts, app relaunches, machine reboots, and even orphaned daemon
  processes. Today a restarted/orphaned daemon happily reconnects.
- **Divorce**: "this DEVICE should never own that subdomain again" —
  today there is NO way to do this from the product at all. The old
  device keeps `~/.k2/tunnel.json` (device identity + bearer) forever,
  and any daemon that reads it will re-claim the name. Decommissioning
  a machine (migration! resold hardware! stolen laptop!) requires
  hand-deleting files.

## 2. The two controls

**A. Disable tunnel (pause — reversible, calm UI).**
- Persisted as `tunnel_enabled: false` in the daemon's on-disk config
  (inside `tunnel.json` or a sibling; must be read at every daemon boot
  BEFORE frpc spawn — the gate lives at the spawn site, not in UI state).
- Effect: frpc stops and is not respawned; subdomain goes dark (relay
  shows offline); identity/lease intact; one click re-enables.
- Surfaces: Settings → Tunnel toggle (existing), `k2 tunnel disable|
  enable` CLI, and the daemon HTTP route the app already uses — all
  three write the SAME persisted flag.

**B. Release subdomain from this device (unpair — destructive, confirm).**
- Effect on device: tunnel.json identity (device_id, bearer token, cert
  keypair) is DELETED (a tombstone note `unpaired.json` records when/what
  for support). The device can never re-claim the subdomain; re-pairing
  later mints a FRESH identity through the normal pairing flow.
- Effect on account: the subdomain remains OWNED by the account and
  unassigned — attachable to a new device (this is the migration story:
  release on old box, pair on new box).
- Effect at the relay (contract, not implementation): the release is
  reported upstream so the relay refuses any future connection
  presenting the released identity — a zombie process holding copies of
  the old files gets REJECTED, not raced. (Offline release: if the
  device can't reach the relay, local deletion still proceeds and the
  upstream revocation is queued/replayed; the account portal can also
  force-release server-side.)
- Surfaces: Settings → Tunnel → "Release subdomain from this device…"
  (red, confirm dialog naming the subdomain + consequences), `k2 tunnel
  release --confirm`, and an account-portal force-release for the
  stolen-laptop case.

## 3. Semantics table

| | frpc | identity on disk | can self-reconnect? | reversible |
|---|---|---|---|---|
| Disable | stopped | kept | no (flag gates spawn) | toggle back |
| Release | stopped | DELETED | never (relay refuses) | re-pair fresh |

## 4. Integration points
- **Migration**: the migration tool's final source-side step = Release
  (the "tombstone" becomes a product feature instead of a runbook).
- **Boot hygiene**: at boot, a daemon whose identity has been released
  upstream must treat the rejection as terminal — log once, do NOT
  retry-loop (today's zombies retry every 33s forever).
- **UI truth**: the Settings tunnel card shows one of: Connected /
  Disabled (by you) / Released (this device unpaired) — never a spinner
  that hides which of the three it is.

## 5. Acceptance
1. Disable → daemon restart → machine reboot → still dark. Re-enable →
   online. (The incident's exact failure, automated as a test.)
2. Release on device A → attach subdomain to device B → power A back on
   with a COPY of the old tunnel.json planted → A is refused by the
   relay and stops retrying; B unaffected.
3. `k2 migrate` completes → source shows Released; source reboot never
   contests the name.
4. Force-release from the portal while device offline → device's next
   connection attempt is refused terminally.

## 6. Pre-mortem
- **"Disable didn't survive because a second daemon never re-read it."**
  The flag must be read at frpc-SPAWN time by whatever process spawns
  frpc — no cached copy. Kill the class, not the instance.
- **"Release deleted the identity but the relay still accepted a stale
  copy."** Local deletion without upstream revocation is cosmetic; the
  relay-side refusal is the load-bearing half. Ship both or neither.
- **"User released thinking it was pause."** Confirm dialog must name
  the subdomain, say "this device" (not "your subdomain — you keep it"),
  and the CLI requires --confirm.
- **"Retry-loop DDoS from refused zombies."** Terminal rejection must be
  distinguishable from transient network failure in the frpc/daemon
  handshake, else refused devices hammer the relay forever.
