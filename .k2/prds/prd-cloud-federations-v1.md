# PRD: Cloud Federations v1 — groups of people who manage groups of servers

**Status:** direction approved (Rosson, 2026-07-08). Nothing built.
**Audience note:** written for a future implementer who was NOT in the
room — possibly junior. Read §2 (trust model) and §9 (pre-mortem) before
writing any code. When this document and convenience disagree, this
document wins until Rosson says otherwise.

## 1. What it is

A **Federation** is a k2.dev-cloud object: a named group of PEOPLE
(K2 personal accounts) granted managed access to a group of SERVERS
(K2 daemons with k2.dev subdomains).

- **Website:** new dashboard tab "Federations". Create federation → add
  servers → invite members → set policy.
- **Desktop + Companion:** when a member signs into their personal K2
  account, every server their federations grant them appears
  automatically in their Connections — no manual add, no per-server
  password exchange (subject to §5 policy).
- **Policy (per federation, set by its manager):** whether login to a
  federated server requires the member's password re-entry, and how long
  a member's access credential (certificate) lives before it must be
  renewed from the cloud.

### 1.1 Naming — read this twice
K2 ALREADY has a feature called "federation": daemon↔daemon peering
(`federation-peers.json`, `k2 talk <ws>@<host>`, tunnel-key fingerprint
pinning — shipped 0.40.14–0.40.21). That is a DIFFERENT SYSTEM and this
PRD does not touch it. Internally, name everything in THIS feature
`cloud_federation` / `CloudFederation` (code, tables, routes, events) —
never bare "federation" — or the two systems will bleed into each other
in search results, bug reports, and junior heads. User-facing copy may
say "Federations" (the site context disambiguates).

## 2. The trust model — the part that makes this hard

Today, every daemon is SOVEREIGN: its `connect-users.json` (argon2
hashes, roles owner/admin/member/viewer — `k2_core::connect_users`) is
the only authority over who gets in. Cloud Federations inverts that for
enrolled servers: **the cloud asserts identities and the daemon chooses
to believe it.** Every design decision follows from four rules:

- **T1 — Daemons opt in, per server, with owner consent.** Enrollment is
  a handshake the DAEMON completes (§4), never a cloud-side list edit.
  A federation can never "claim" a server whose owner didn't act.
- **T2 — The daemon verifies, never trusts.** Members authenticate to a
  daemon with a **federation grant**: a short-lived document SIGNED by
  the cloud (Ed25519), carrying {federation_id, member identity, server
  scope, role, not_before, expires}. The daemon verifies the signature
  against a cloud public key it pinned AT ENROLLMENT (key rotation =
  overlapping keys in the enrollment record, same pattern as updater
  signing). The relay CANNOT mint access: E2E tunnels are TLS-terminated
  at the daemon, and the relay never holds the cloud signing key.
- **T3 — The local owner always outranks the federation.** Owner-role
  on the box beats any grant. The owner panel gets "Federated access"
  with: view enrolled federations + live grants, suspend federation
  access instantly (local kill switch, works offline), unenroll.
- **T4 — Fail closed, degrade gracefully.** Cloud unreachable → no NEW
  grants can be minted, but UNEXPIRED grants keep working (that is
  exactly what the §5 lifetime knob trades off). A daemon that cannot
  check revocation honors the grant until expiry — which is why maximum
  lifetime is bounded (§5).

## 3. Objects & where they live

| Object | Lives | Notes |
|---|---|---|
| Federation {id, name, owner_account, policy} | k2.dev cloud (Supabase, k2-connect repo) | RLS: only owner/managers read-write |
| Membership {federation_id, account_id, fed_role: manager\|member, server_role: admin\|member\|viewer} | cloud | `server_role` maps DIRECTLY onto the daemon's existing `connect_users::Role` — do NOT invent a new role lattice. Federated grants can never carry `owner`. |
| Server enrollment {federation_id, subdomain, device_id, enrolled_by, cloud_pubkey_ids, status} | cloud + a mirror record on the daemon (`~/.k2/cloud-federation.json`) | daemon mirror is the daemon's source of truth for "am I enrolled + which keys do I trust" |
| Grant (the signed credential) | minted by cloud on demand; cached on the member's DEVICE (keychain via existing `dev.k2.connect.*` service pattern — NEVER localStorage) | one grant per (member, server); renewed silently before expiry while the account session is valid |
| Policy {require_password_on_connect: bool, grant_ttl: duration (cap: 30d, default: 24h), } | cloud, embedded in every grant | daemon enforces what the GRANT says, not what the cloud currently says — changing policy takes effect as grants renew |

## 4. The flows (v1 scope)

1. **Create + invite (web):** Federations tab → create → invite by
   email (reuse the existing k2.dev auth/invite machinery). Invitee
   accepts with their personal K2 account.
2. **Enroll a server (web + daemon handshake):** manager clicks "Add
   server" → cloud issues an enrollment code → the SERVER OWNER runs
   `k2 cloud-federation enroll <code>` on the box (or clicks approve in
   the owner panel). The daemon fetches the federation record + cloud
   pubkeys over its OWN tunnel-authenticated channel, writes
   `cloud-federation.json`, and confirms to the cloud. Result: T1 is
   structural — the code proves manager intent, the daemon action proves
   owner control. (Same consent shape as the existing subdomain lease.)
3. **Member connects (desktop/mobile):** app sign-in → fetch federated
   server list → merge into Connections (§6) → on connect, present the
   grant to the daemon (`Authorization: this is a new auth CLASS in the
   dispatcher — sits BESIDE owner-token / connect-user session / stream
   token, reusing the `ConnIdentity` seam in sessions_grid_ws.rs and the
   route dispatcher`). Daemon verifies (T2), creates/refreshes a local
   shadow user `fed:<account_id>` with the grant's `server_role`, and
   everything downstream (presence, viewer/claimer gates, kick) works
   UNCHANGED because it already keys off connect-user roles.
4. **Revoke:** remove member on web → cloud stops renewing their grants
   (hard stop at TTL) AND pushes a best-effort revocation to each
   enrolled daemon over the tunnel (instant when online). Manager-facing
   copy must be honest: "Access ends immediately on reachable servers,
   and within <grant TTL> everywhere."
5. **Audit (v1-minimal but NOT optional):** daemon logs
   {grant_id, account, action-class, timestamp} for federated sessions
   into the existing feedback/log surface; cloud logs mint/renew/revoke.
   Shared authority without attribution is how teams lose trust in the
   feature after the first incident.

## 5. The policy knobs (and their real meaning)

- `require_password_on_connect` — OFF: possession of the signed grant +
  the member's cloud session is sufficient (SSO feel). ON: daemon
  additionally requires the member's federated-profile password (a
  cloud-verified secret — NOT a per-daemon password; v1 can implement as
  cloud re-auth that stamps the grant `recently_verified`).
- `grant_ttl` — the revocation-latency dial (T4). UI must say so
  explicitly: "If you remove someone, offline servers honor their old
  credential for up to this long." Cap 30 days, default 24h, minimum 1h.

## 6. Client integration rules (desktop + Companion)

- Federated connections are a SEPARATE list merged at render time —
  never write them into the user's saved-hosts file. Tag with the
  federation name in UI. Manual entries always win a hostname collision.
- Sign-out / defederation removes federated entries + their cached
  grants (keychain), touches nothing manual.
- Grants cache in the OS keychain under the existing
  `dev.k2.connect.*` naming; Companion uses the same keychain pattern
  proven in v3.0.1.
- The auto-connect list comes from ONE cloud endpoint
  (`GET /federations/mine/servers`) — clients never enumerate
  federations and join lists themselves.

## 7. Explicit non-goals for v1 (the over-thinking guard)

- NO per-member-per-server role matrices. Role is per membership,
  applies to all servers in the federation. (Split into two federations
  if you need two postures. Revisit only with real demand.)
- NO SSO/OIDC/SAML. Personal K2 accounts only.
- NO cross-federation server sharing dedupe cleverness: a server may be
  in several federations; a member connecting through either gets the
  higher of their roles; that's it.
- NO daemon↔daemon implications. Enrolling a server changes nothing
  about `federation-peers.json` peering.
- NO billing build-out beyond a tier gate (Federations = Team tier per
  the fleet-console pricing direction; one boolean check at create).
- The relay learns NOTHING new. It stays a dumb SNI passthrough for E2E
  hosts. If a design step requires the relay to parse or inject
  anything, the design is wrong — stop and re-read T2.

## 8. Build order (each stage independently shippable)

1. Cloud schema + Federations tab CRUD + invites (k2-connect repo,
   Supabase RLS reviewed by a second person — RLS bugs are silent).
2. Enrollment handshake (cloud endpoint + `k2 cloud-federation enroll`
   + owner-panel approve + `cloud-federation.json`).
3. Daemon grant verification (new auth class in the dispatcher +
   `ConnIdentity::CloudFederation` + shadow users + T3 kill switch).
4. Desktop auto-connections (merge list + grant cache + connect path).
5. Companion parity.
6. Revocation push + audit surfaces.
Ship 1–2 dark. 3 is the security-review moment (see §9-P1/P2). 4 makes
it real.

## 9. PRE-MORTEM — "it's a year later and Federations failed. What happened?"

Written for the implementer. Each failure is one we are choosing, today,
to prevent. If your implementation makes one of these possible, the
implementation — not the pre-mortem — is what changes.

- **P1. "Anyone could enroll anyone's server."** The enrollment code was
  accepted cloud-side without the daemon handshake, so a manager listed
  a server they didn't control and the cloud started minting grants for
  it. → The daemon's signed confirmation (step §4.2) is the ONLY thing
  that flips an enrollment to active. Test: attempt to activate an
  enrollment with no daemon confirmation — must be impossible at the
  DATABASE level (state machine), not just hidden in UI.
- **P2. "A forged grant worked."** Verification checked expiry but not
  the signature, or trusted a pubkey fetched at CONNECT time (attacker
  supplies key+grant together). → Keys are pinned at enrollment,
  delivered over the daemon-authenticated channel, and grant
  verification is a pure function with test vectors (valid / expired /
  future nbf / wrong key / tampered payload / role escalation to owner).
- **P3. "Fired admin still had access for a month, nobody knew."** A
  manager set ttl=30d for convenience; revocation push silently failed;
  nothing surfaced. → Revocation push failures are VISIBLE on the web
  dashboard per server ("revocation pending until <date> — server
  offline"). The honest-copy rule in §4.4 is a requirement, not tone.
- **P4. "Cloud outage locked every federated team out."** Grants were
  validated ONLINE against the cloud on every connect. → T4: validation
  is offline against pinned keys; the cloud is needed only to MINT and
  RENEW. Test: kill cloud, connect with cached unexpired grant — works.
- **P5. "A compromised cloud signing key owned every enrolled server."**
  One key, no rotation story, no local ceiling. → Rotation via
  overlapping enrolled keys; grants can never carry `owner` role
  (§3 Membership); T3 kill switch works with the cloud fully
  compromised because it is LOCAL state.
- **P6. "The two federations."** Support tickets, code, and docs mixed
  daemon-peering federation with cloud federations until nobody could
  reason about either. → §1.1 naming rule enforced in review; add
  `cloud_federation` to the repo glossary; the k2so-gate pattern shows
  how to make a naming rule mechanical if drift starts.
- **P7. "It deleted my connections."** Sign-out or a cloud hiccup wiped
  manually-added hosts because federated and manual entries shared a
  store. → §6 separate-list rule; test: defederate → manual hosts
  untouched, byte-identical file.
- **P8. "Role escalation by shadow user."** The daemon-side shadow user
  (`fed:<account>`) got created as a REAL connect-user row once, then
  drifted — grant expired but the row lived on with a password someone
  set. → Shadow users are ephemeral, marked, unable to hold passwords,
  and reaped when their grant expires. Test: expire grant → presence
  drops, re-auth fails, no residue in connect-users.json.
- **P9. "Grants in localStorage."** A junior cached the grant next to
  the UI state because the keychain API was annoying. It synced to a
  backup, leaked in a screen-share, lived forever. → Keychain only
  (§6); reviewer greps for the grant type name in renderer stores —
  zero hits outside the keychain bridge module.
- **P10. "Policy change did nothing (or did too much)."** Manager
  flipped require_password and either nothing changed for 30 days
  (grants embed policy, all long-lived) or every session died at once
  (daemon re-read policy live and dropped everyone mid-work). → Policy
  rides the GRANT (§3) — takes effect on renewal; UI states this; ttl
  cap keeps "on renewal" meaningfully soon. Session-drop on revocation
  is deliberate and scoped to the revoked member only.
- **P11. "Nobody could tell who broke the server."** Three federations,
  eleven members, one deleted workspace, zero attribution. → §4.5 audit
  is in v1. If audit slips a release, Federations does not ship to a
  second customer.
- **P12. "The demo worked; the fleet didn't."** Everything was tested
  against one server, one federation, one member, always online. → The
  §8 stage gates each include the ugly matrix: N servers (one offline),
  member in 2 federations with different roles hitting the same server,
  revoke-while-connected, enrollment of an already-enrolled server,
  daemon older than the feature (must degrade to "unknown auth class →
  uniform 404", never crash — the no-oracle convention holds).

## 10. Open questions for Rosson (decide before stage 3)

1. Grant TTL default 24h / cap 30d — confirm the numbers.
2. Can a federation manager also be granted `admin` server_role, or is
   manager a purely cloud-side role? (Recommend: independent axes.)
3. Tier gating: Federations at Team tier — confirm against current
   pricing sheet.
4. Should enrolled-server owners see the member LIST (privacy vs
   transparency)? Recommend yes — it's their machine.
