# Follow-up: Servers / pair / “add as connection” UX

**Status:** deferred — product-accepted for now; **review soon**  
**Date:** 2026-07-28  
**Raised by:** Rosson  

Also mirrored under workspace notes (gitignored): `.k2/notes/connect-servers-pair-ux-followup.md`

---

## Current model

| Surface | Data source | Purpose |
|---|---|---|
| **Top-bar switcher** | This Mac’s address book | Navigate daemons; always includes **This Mac** |
| **Settings → Servers (Local)** | Same local address book | Manage saved hosts |
| **Settings → Servers (remote)** | Active host’s **federation peers** | Who *that* cloud is paired with |
| **Pair from this Mac** | Local signed-in hosts | Cloud↔cloud Pair (needs credentials on both ends) |
| **External agents** | Active host workspace links | Agent-level connections |

Top bar must **never** become the remote peer list — that would block returning to Local.

---

## Friction to revisit

1. **Discover + pair is client-credential gated** — to pair A↔B while active on A, this device usually must already have B saved and signed in. Hard to “add a server as a connection” or fully answer “is it a federated peer?” without that.
2. **Three surfaces** — peers list / local book / workspace connections; operators must know all three.
3. **Ideal later** — guided Pair by subdomain (sign into B in-flow), first-class peer status when adding a connection, without pre-seeding the laptop address book.

## Related commits

- Host-scoped Servers again: `6b25fe3d`
- Earlier “always local book while remote” (Pair convenience): `c482d627`
