# Person-writer reuse, r0 (design)

Status: **approved** (rulings below, §9). Built on branch `feat/person-writer-reuse`. Kit tree: `87926539` (v0.13.1). K2 read-only at
`7d37a2b7d`.

**Goal.** Give one person one writer per device, so a long-lived doc's writer count follows
people and devices, not connections. A writer is a `ClientId`; the native engine maps it 1:1 to
an agent. The design must never let two live connections hold one writer, and never fork a
writer's line under Yrs or the native engine. It sits behind a config flag that is off by
default.

---

## 1. What the code does today

| What | Where |
|---|---|
| Every kit client ID comes from one helper: 53 random bits, redrawn until in `2..=MAX_KIT_CLIENT`. | `crates/base/src/session/live.rs:965` `mint_client_id` |
| A fresh ID is never held by a present connection and never has an owner row in this doc. An ID that left without writing has no row and could be drawn again. | `live.rs:1065` `fresh_client` |
| Join: a shared join reuses the pid's shared connection (`join_shared`, CLI verbs). Otherwise the kit tries agent reuse, then mints a fresh ID. Owner and kind rows are written at join only for an agent or an own replica that has edit rights. | `live.rs:1765-1835` `join_impl` |
| Agent reuse picks the lowest ID that is owned by this pid, is in `reusable_clients` (kind `KIND_AGENT`), and is held by no present connection. Never an own replica's ID; never an ID of unknown kind. | `live.rs:1906-1930` `reusable_client`; `config.rs:211-224` `reuse_agent_clients` (default true) |
| Store rows: `clients` is `(doc, client) → pid`. `client_kinds` is `(doc, client) → [kind]`: `0` agent (reusable) or `1` own replica (never reused). Neither table needs a format bump: both are created lazily, and a missing row means "unknown, never reused". | `crates/base/src/store/mod.rs:129-139, 178-199, 1975-2023` |
| A reconnect: `join_replacing` / `JoinOptions::prev` closes the pid's connection that holds `prev`. The new member is pushed **before** the old one is removed, inside the same lock. | `live.rs:1736-1745, 1883-1897` |
| A leave, idle expiry, kick or loss of rights all go through `remove_member`. Nothing is written to the store. | `live.rs:1595`; `LeaveReason`, `host.rs:257` |
| **Every op under a member's writer is made by the session's own doc**, under the session lock, at that writer's next clock in the doc. Yrs: `transact_mut_as(client)`. Native: `next_seq(agent)`. | `doc.rs:609-640` `edit_made`; `engine/yrs.rs:551`; `engine/mpce.rs:737` |
| **Durable before it leaves the session.** `commit_changes` lands in one store transaction (`live.rs:3675-3693`) before any `ChangeEvent` is broadcast (`live.rs:3742, 3762`) and before the disk replica catches up. On any failure, `recover` reloads the doc from the store (`live.rs:3690`, `live.rs:4443`), which drops the op from memory. | |
| Client CRDT bytes (Yjs or native updates from a connection) are refused unless the `untrusted-wire` feature is on. That feature is off by default and guarded at build time; no session path calls `apply_client_update`. | `crates/base/Cargo.toml:11-17`; `doc.rs:541-584` |
| **The browser holds no CRDT replica.** `LiveText` sends hunks at a base version (`edit`), and the session applies them as the connection's writer. `welcome.client` is the kit's `Joined.client`; the browser never picks a Yjs or Yrs ID. | `client/WIRE.md` "Connecting", "Client → host" |
| The JS client uses the writer ID for four things: (a) `isOwn`, to recognise its own in-flight edit (`client/src/live.ts:1260-1268`); (b) the **lost-change watch**, which decides whether an `MPC-E000` edit landed later. It is keyed on the ID the edit went out on, and its docstring assumes "Client IDs are never reused on a doc" (`live.ts:552-600, 1204-1216, 1293-1320`); (c) `undoLostOn` (`live.ts:520`); (d) hiding its own caret among the same user's devices (`live.ts:644`). The client also mints **tagged keys** `<prefix><n>.<tag>`, whose tag is the writer ID in base 36 (`client/src/mint.ts`; kit `crates/base/src/mint.rs`, `session/live/mint.rs`). | |
| A reconnect is a new connection with a **new** ID. Edits queued while disconnected are sent as hunks on the new connection, under its ID. Frames are never replayed. | `client/WIRE.md` "Reconnecting" |
| K2 joins every socket with `JoinOptions::new()` (`crates/k2-daemon/src/docs/live.rs:463`). K2 handles `prev` itself in `retire_prev`, matching on **pid + doc + bare client ID**, and calls `leave` before the new join (`live.rs:267-288, 451-455`). The editor keeps `prev` in memory (`src/renderer/components/FileViewerPane/LiveDocView.tsx:148`). | K2 |

**What this means for forks.** Every writer of a person who edits through the hunk channel or
verbs is written by the session alone. A writer's next op is formed from the doc's own state for
that writer, and nothing outside the session holds an op of that writer that the store lacks.
So handing a person's writer to a later connection cannot fork its line under Yrs or the native
engine. That holds whatever the client does: a client that returns with unsent edits sends
**hunks**, never ops under an old ID. The real risks of reuse are elsewhere:

- two live holders;
- the client's own bookkeeping keyed on the ID: the lost-change watch, `undoLostOn`, minted tag
  keys, and its own caret;
- K2's bare-ID `prev`, which could close a sibling tab that now holds the ID.

The design closes each of these (§3).

---

## 2. Design

### 2.1 Config, options and API (all additive)

- `Config::reuse_person_writers: bool`, **default false**. It is listed in the settings dump at
  `config.rs:473`. When it is false, or when a join carries no device key, the behaviour is
  exactly today's.
- `Config::person_writer_park_ms: Option<u64>`, default `None`: a parked writer is never re-let
  (see §2.3 and question Q1).
- `JoinOptions::device(key: impl AsRef<[u8]>)`: an opaque host key, 1 to 64 bytes, for a client
  that keeps state (`LiveText`). It takes part in reuse only when the participant is not an agent
  and not `own_replica`.
- `JoinOptions::device_stateless(key)`: the same, for a connection with no client-side state
  (CLI verbs, `join_shared`). Every end of such a connection counts as clean.
- `JoinOptions::reclaim(WriterLease)`: replaces `prev` when a device key is set (§2.4).
- `Joined` gains:
  - `lease: Option<WriterLease>`, where `WriterLease { client, token: u64 }` and the token is
    random per hand-out, drawn from `crate::hash::random32`;
  - `kept: bool`: this connection continues the reclaimed lease's writer;
  - `prev_ended: Option<Seq>`: see §2.5.
- `Session::release(conn) -> Result<bool>`: a **clean** leave. The host calls it when the client
  says it holds nothing unsent and will never use this writer again (the `bye` frame, §2.5).
  `leave`, idle expiry, kick, rights lost, a host close or unload, and a crash are all
  **unclean**.
- Agents are unchanged. `reuse_agent_clients` and its rule stay as they are, and a device key on
  an agent's join is ignored.

### 2.2 Writer states

There is one row per person-device writer, plus a state held in memory.

```
            join (fresh or free)                    release (clean)
   (none) ───────────────────────►  HELD(conn, token) ─────────────────► FREE
                                       │      ▲                            │
              leave/idle/kick/rights/  │      │ reclaim(client, token)      │ join, same pid+device
              close/unload/crash       ▼      │ (kept = true, new token)    ▼
                                    PARKED(token) ─────────────────────► HELD
                                       │
                                       │ park expiry (only if person_writer_park_ms is set)
                                       ▼
                                     FREE (row keeps ended_token, ended_seq for the cap)
```

- **HELD** exists only in memory (`Member.client`, plus the token on the member). After a reload,
  a writer whose row says *leased* is **PARKED**: a crash or an unload never frees a writer.
- **Selection for a non-reclaim join.** The kit picks the lowest writer whose row is FREE, whose
  owner is this pid, whose device tag matches, and which no present member holds. If there is
  none, it mints a fresh writer with `fresh_client`. A PARKED writer is never picked, unless the
  park has expired (§2.3).
- **Lease rows** are written when the connection can first write: at join if it has edit rights,
  otherwise on the first call that sees edit rights. This is the `rows_kept` pattern of
  `keep_own_replica_rows` (`live.rs:1632`). A read-only connection never has anything in flight,
  so every end of one is clean, and it needs no row until it can write.
- A person's writer is **never** handed to another pid. Selection filters on the owner row, and a
  participant purge renames owners (`store/purge.rs:606-617`), so an erased person's writers are
  never picked again.

### 2.3 The clean-close rule

A writer becomes FREE only through `release`, or through the end of a `device_stateless`
connection. Before the client sends `bye` it guarantees all of the following:

- no edit, undo or redo in flight;
- an empty queue;
- no lost-change watch keyed on this writer;
- no minted tag key that it has not sent.

After `bye` it never uses this writer or its lease again. A page restored from the back-forward
cache reconnects as a new connection, with no lease.

Every other end **parks** the writer. A parked writer goes back only to a `reclaim` that names its
`(client, token)`, which is only the instance that held it.

**Park expiry** (Q1) is off by default. When set, a parked writer older than
`person_writer_park_ms` is FREE for the same pid and device. The row keeps
`ended_token, ended_seq`, the version at which the parked lease ended (§2.5).

### 2.4 Reclaim and hand-over (a reconnect keeps its writer)

`join_with(p, stay, opts.device(k).reclaim(WriterLease { client: T, token }))` runs under the
session lock:

1. **T is held by this pid's member with this token** (a half-open old socket). The kit removes
   that member first, then pushes the new member with T and a **new** token.
   - Nothing from the old connection can apply after this: its `ConnId` is no longer a member,
     so `edit_at_base` refuses it with `NotJoined`.
   - The pid stays present: no participant `Left`, as `join_replacing` today.
   - The order is the reverse of today's push-then-remove. That way no point inside the lock has
     two members holding T, and a debug assertion after every join and leave checks this.
   - `Joined { kept: true, replaced: Some(old) }`.
2. **T is PARKED for this pid and device with this token.** It becomes HELD with a new token, and
   the row is updated. `kept: true`.
3. **Otherwise** (wrong token, another pid or device, now held by a sibling, or re-let after an
   expiry): no connection is closed and nothing is revealed. The kit does a plain device join
   (FREE or fresh), and returns `kept: false` and `prev_ended` (§2.5).

When a device key is set, a bare `prev` (no token) **closes nothing**. The kit can't tell the old
self from a sibling that now holds the same ID. The host must not retire it either (§4).

### 2.5 The JS client side and the wire

The capability rides on the device key. A host passes `device(..)` only for a client that
declared the capability, for example a connect parameter `writer=lease`. Old clients never send
it, so they get today's behaviour exactly.

- **`welcome`** gains `lease: string` (the token as decimal: u64 does not fit a JS number) and
  `kept: boolean`. It also gains `prevEnded?: number` when the reclaim failed and the kit knows
  the version at which that lease ended.
- **Reconnect request:** `prev=<client>&lease=<token>`. The host passes it as
  `JoinOptions::reclaim`.
- **New client → host frame: `bye {}`.** It is sent on page hide or close only when the guarantee
  of §2.3 holds. The host calls `Session::release(conn)`, then closes. When the guarantee does
  not hold, the client sends no `bye`; the end is unclean, and the writer stays parked for this
  instance's reclaim.
- **`LiveText.reconnected(w)` with `w.kept`:**
  - It does not add T to `undoLostOn`. The kit closed the old lease before this welcome, so an
    undo from it either applied before `w.version`, and shows in the `since` reply as an undo
    change, or never applied.
  - The edit in flight is settled by the `since` reply **definitively**: listed means applied;
    not listed means rejected with no `asOf` and no lost watch. The kit's guarantee replaces the
    watch.
  - Minted tag keys still queued stay valid, since T is still theirs.
- **With `!w.kept`** the behaviour is as today. In addition, every lost watch on the old ID is
  **capped**: only changes at versions `≤ prevEnded` count. Without `prevEnded`, the watch's
  verdict becomes `unknown`: conservative, so it never gives a false `present` or `absent`.
  Queued inserts with the old tag are refused with `KeyNotOwn`, as they are today.
- The client's docstring "Client IDs are never reused on a doc" (`live.ts:566-568`) becomes:
  "an ID is handed to another connection only after its holder released it with `bye`, or after
  its lease's end was announced (`prevEnded`)".
- **Where `prevEnded` comes from:** the row's `ended_seq`, written at each unclean end (one small
  write per leave) and at load for writers left leased by a crash: the loaded version. Every op
  of a parked lease is in the store by then (§1), so that version is a valid upper bound.

### 2.6 Device key: where a host gets it

The key is opaque. The kit stores only `device_tag = HMAC-SHA256(doc hash key, key)[..16]`
(`crate::hash::hmac`, already imported at `live.rs:64`), so the store never holds the host's key.

K2 could derive it in either of two ways:

- **Desktop app:** the id of the authenticated app install or token record (not the token
  itself).
- **Browser:** a random id the page keeps in `localStorage`, sent as a connect parameter.

**Safety does not depend on the key being honest.** A wrong key only picks among the same pid's
FREE writers, which are clean by definition, or mints a fresh one. Parked writers need the token.
So a key may even come from the client.

A purge rotates the doc's hash key (`session/live/purge.rs:713`), so device tags stop matching after one: each
person and device draws one new writer after a purge (see Q3).

### 2.7 Store rows and N−1

The existing `client_kinds` table gets a new kind, `KIND_PERSON_DEVICE = 2`. As with the existing
`client_kinds` table, this needs **no format bump**:

```
[2, 1 /*row version*/, state u8 (0 free, 1 leased), device_tag [16], token u64,
 ended_token u64, ended_seq u64, ended_ms u64]          // 51 bytes
```

It is written in the same transaction as the `clients` owner row (`set_client_kind`'s pattern).
Transitions:

| Event | Row write |
|---|---|
| Lease (join with edit, or rights upgrade) | `state=1`, new `token` |
| Release | `state=0`, `token=0` |
| Unclean end | `ended_token=token`, `ended_seq=version`, `ended_ms`; stays `state=1` |
| Reclaim | new `token` |

Writes per connection are join plus leave: two small transactions, like an agent's join today
plus one more.

**N−1: an older kit (v0.10.2 to v0.13.1) on a store with kind-2 rows.**

- Its `reusable_clients` lists only rows equal to `[0]` (`store/mod.rs:2003-2023`), so it never
  reuses these IDs.
- Its `fresh_client` skips every ID with an owner row, so it never hands one out.
- Blame reads `clients`, so attribution is intact.
- Rewrite, delete and the orphan sweep copy or erase the rows as opaque bytes
  (`DOC_TABLES`/`ADDITIVE_TABLES`, `store/mod.rs:214-240, 350-379`).
- Persons get fresh IDs while the older kit runs. When this kit opens the store again, the rows
  are still valid, and parked tokens still reclaim.
- A kit before `client_kinds` existed refuses a rewrite, as it does today (`MPC-E502`).

This needs one N−1 row in `tools/n1` (§5, R7).

### 2.8 Attribution (unchanged)

- The owner row `(doc, T) → pid` is written at the first lease, and reuse requires
  `owner == pid`.
- `author`, `via` and `on_behalf_of` are per change, in the log entry (`live.rs:3617-3624`).
- Blame, D1 ownership (`owners`, by pid), undo stacks (per pid), "contributors since last read"
  and the activity ring are all keyed by pid. None of them changes.

### 2.9 Own replicas (a CRDT-holding client), for the record

None exists on any session path today: `untrusted-wire` is off, and no session calls
`apply_client_update`. Own-replica writers stay **never reused**, as now.

A later protocol, not built in r0, would work like this:

- The replica keeps its writer. It presents `(client, token, its clock for client)`.
- The kit admits it only if all of these hold:
  - the row is this pid's and this device's;
  - the token matches;
  - the doc's clock for that client equals the presented one (`Doc::clock_of`, `doc.rs:737`).
- Its later updates must be contiguous: Yrs `check_client_update` refuses a skip-ahead; the native
  engine refuses with contiguity checks and E195.
- The token rotates on each resume. A copy of the replica (two tabs on one IndexedDB, a restored
  backup) presents a stale token and gets a fresh writer. It must re-issue its pending ops as
  hunks, never as ops under the old ID.
- Clients that send no token are never reused.

---

## 3. Every way a doubled writer or forked line could happen, and why it can't

| # | Hazard | Why it can't happen |
|---|---|---|
| H1 | Two live connections hold one writer. | Selection excludes held writers. Hand-over removes the holder before pushing. A debug assertion after every join and leave checks distinct clients among members. R9 plants the fault. |
| H2 | A fork under Yrs: two different ops share `(client, clock)`. | Every op under a person writer is formed by the session doc at `clock_of(client)`, under the lock (`yrs.rs:551`). It goes out only after a durable commit (`live.rs:3675` before `3742`), and `recover` reloads on failure. No other replica writes as a person writer (`untrusted-wire` is off; own replicas are excluded). |
| H3 | A fork under native: the agent's previous event is not in the new event's past (E195). | Same as H2. `edit_as` starts at `next_seq(agent)` on the doc that holds the agent's previous event (`mpce.rs:737`), so that event is in the new one's past. |
| H4 | Two sessions on one doc each think T is free. | There is one session per doc in a manager, and redb locks the store file per process. R10 pins this: a second `DocStore::open` of the same file is refused while one is open. |
| H5 | A crash between commit and broadcast, or during a lease. | The op is durable, or it is absent everywhere. Leased rows load as PARKED, never FREE. |
| H6 | A returning tab or device sends unsent edits under its old ID. | They travel as hunks at a base, applied as whatever writer the new connection holds, or refused `base_expired` and reloaded. They are never ops. |
| H7 | A false lost-change landing: another holder's edits under T read as the lost edit. | T goes to another connection only after a `bye` (no watch left on it) or an expiry with a `prevEnded` cap. With `kept`, no watch is created. Old clients get no device key, so no reuse. R3, R5, R11. |
| H8 | `prev` closes a sibling that now holds T. | With a device key, only `(client, token)` closes or reclaims, and a bare `prev` closes nothing. K2's `retire_prev` must match on the token too (§4). R5. |
| H9 | A minted tag key is duplicated or reused (`r5.T` minted by a stale holder and by a later holder). | A stale holder either reclaims T (nobody else held it in between) or gets a new writer, and its old-tag inserts are refused (`KeyNotOwn`, or E804 for a known key). A clean holder holds no unsent minted keys. R12. |
| H10 | An undo is wrongly marked lost (`undoLostOn`) under a kept T. | With `kept`, the client does not add T (§2.5). |
| H11 | An older kit hands out a person-device ID. | Its reuse lists only `[0]` rows, and `fresh_client` skips owned IDs (§2.7). R7. |
| H12 | Attribution drifts between pids. | Reuse only where `owner == pid`. Erasure renames the owner, so the writer is never picked again. |
| H13 | Roster churn at hand-over: the device for T vanishes or appears twice. | Removal and push happen in one lock with one flush, so exactly one device for T remains. R2 checks this from a JS client's view (`live.ts:1380-1390`). |

---

## 4. K2 integration (for K2; not in the kit)

- **`open_live`** (`crates/k2-daemon/src/docs/live.rs:463`) passes
  `JoinOptions::new().device(dev).reclaim(lease)`, only when the client sent `writer=lease`.
  `Handle` keeps the token.
- **`retire_prev`** (`live.rs:276`) matches on `(client, token)` when a token is present. With a
  device-keyed connection, a bare `prev` retires nothing. Better: K2 lets the kit close the
  connection (`Joined.replaced`) and only marks that socket dead.
- **New `bye` frame:** on it K2 calls `Session::release(conn)`, then closes.
- **Device key:** from the app install's auth record, or a browser id (§2.6).
- **Welcome:** `lease`, `kept`, `prevEnded` come from `Joined`.

---

## 5. Rows (tests)

Every row runs on Yrs and on the native adapter (`EngineChoice`). Where a row needs real-engine
evidence, the coordinator runs it as a compute job.

| Row | What it does | Must hold |
|---|---|---|
| R1 two tabs | The same pid and device join twice; both edit. Tab 2 releases; tab 3 joins. | Tabs 1 and 2 have distinct writers. Tab 3 gets tab 2's writer. Both are attributed to the pid. The doc's writer count is 2. |
| R2 reconnect with a half-open socket | Tab 1 holds T; `reclaim(T, token)` while tab 1 is still a member. | `kept`, `replaced = tab 1`. An `edit_at_base` on tab 1's conn afterwards is `NotJoined`. T's clock continues. The roster shows one device for T. |
| R3 crash with pending edits | Tab 1 has an edit in flight and a queue; its socket dies; `leave` (unclean). Tab 2, same device, joins; then tab 1 reclaims. | Tab 2 gets a writer ≠ T. Tab 1 is `kept`; its queued hunks apply under T. Every log entry under T starts at the previous entries' end (the contiguity check, below). Replayed into a fresh Yrs doc and into a fresh Yjs doc (the conformance harness), there are no duplicate `(client, clock)`. Native: a replay through admission never refuses E195. |
| R4 an offline device returns after a reload | T is held; the host closes the session (unload); store reopen; reclaim with the right token, then with a wrong token. | Right token: `kept`. Wrong token: a fresh or free writer, no member closed, `prev_ended` set. Old-base hunks apply or return `base_expired`. |
| R5 a stale lease names a writer a sibling holds | Expiry on; T re-let to tab 2; tab 1 reclaims with the old token; also a bare `prev=T` with a device key. | Tab 2 is untouched. Tab 1 gets a new writer, with `prevEnded` = T's ended version. A bare `prev` closes nothing. |
| R6 old client, no device key | Today's join, `join_replacing` and `join_shared` rows, rerun with the flag on. | Byte-for-byte today's IDs and rows (seeded draws). |
| R7 N−1 | `tools/n1`: the older kit opens a store with kind-2 rows, joins the same pid, edits, rewrites; then this kit reopens. | The older kit never hands out a kind-2 ID. Blame is intact. Rows survive its rewrite. This kit reclaims parked tokens. |
| R8 long history | A seeded simulation of N joins over people × devices: tabs, reconnects, unclean ends, reloads. Flag off and flag on. | Flag off: writers = writing connections. Flag on: writers ≤ Σ(max concurrent tabs per person-device) + unclean ends never reclaimed (without expiry), or ≤ the first term + returns after expiry (with expiry). Measured as the Yrs state-vector length and the native agent count. |
| R9 plant `MintMutant::ReusePersonHeld` | Selection ignores held writers (the person twin of `ReuseHeldClient`, `differential.rs:827`). | R1 must fail on "distinct clients among members" and on tab 1 and tab 2 minting the same tag key (`mint_key`) before either inserts. The churn test (`tests/mint.rs:669`) must fail too. |
| R10 one store, one session | A second open of the store file while one is open; a second `Session` for one doc key through the manager. | Refused, or the same `Arc`. |
| R11 plant `ClientMutant::ReuseParked` | A parked writer is handed to a sibling's plain join, with no expiry and no cap. | R3 must fail. The JS client row (vitest, fake host): tab 1's lost watch on T sees tab 2's edit under T and fires `onLanded` (a false positive). The row asserts no `onLanded`. |
| R12 tag keys across hand-over | Board-style pack: tab 1 mints `r5.T` offline. (a) Tab 1 reclaims: the insert is accepted. (b) With expiry, T is re-let and tab 2 inserts `r5.T`; tab 1 returns. | (a) Accepted. (b) Tab 1's insert is refused (`KeyNotOwn`/E804); no duplicate key in the list. |
| R13 plant `HandoverWithoutClose` | Hand-over keeps the old member. | R2 must fail: the old connection's edit lands after the `kept` welcome. |

**The contiguity check** (R3, R8). For every client in the log, each entry's first clock equals the
sum of that client's earlier entries' lengths. The store's `ClockSpan` rows already record
`(client, start, end)` per change. Yrs `clock_of` must equal the last span's end. Native: replay
every entry through `check_update(.., Admission::client(agent))`.

**Plants** go through the existing `differential` hooks. The selection is via `MPC_STRUCT_MUTANT`,
as in `tests/client_reuse.rs` and `tests/mint.rs`. A plant counts as caught only when the failing
assertion line is the planted step's.

---

## 6. Expected reduction and memory

The model (`sim.py`, seeded, kept in the worker scratch folder; inline below if wanted):

- each person has `D` devices, used 70/30, or 70/15/15 for three devices;
- per writing session, reconnects that write afterwards: mean 0.5;
- a second concurrent writing tab in 5% of sessions;
- a share of ends unclean and never reclaimed (crash, a tab closed while offline, a `bye` lost);
- agents are excluded (already reused).

Writers per doc in the model (off / on / on with expiry):

| Doc | Unclean | Writers, off | Writers, on | On + expiry |
|---|---|---|---|---|
| small: 3 people, 1 device, 120 sessions each | 5% | 597 | 27 (−95.5%) | 6 (−99.0%) |
| team: 8 people, 2 devices, 1000 sessions each | 2% | 12,658 | 193 (−98.5%) | 32 (−99.75%) |
| team | 5% | 12,658 | 439 (−96.5%) | 32 |
| team | 10% | 12,658 | 854 (−93.3%) | 32 |
| heavy: 20 people, 3 devices, 3000 sessions each | 5% | 94,875 | 3,116 (−96.7%) | 120 (−99.87%) |

**Memory per writer, measured** (R8, `tests/person_writers_cost.rs`
`measure_bytes_per_writer`: the same history made by one writer per append and by 10 or 100
writers taking turns, so both docs hold the same blocks; the heap of each doc loaded from its
state, counted by a global allocator):

| Engine | Heap per writer | Stored state per writer |
|---|---|---|
| Yrs | 120–134 B | about 18.5 B |
| Native | 129–205 B | none measurable (the native state doesn't grow with writers here) |

These replace the 300 B (Yrs) and 700 B (native) per writer this note first assumed; those
figures are not to be used.

- **Expected reduction:** on a realistic team doc (5% unclean), writers fall about **30×**
  without expiry and about **400×** with it. At the measured cost that is, per doc, Yrs heap
  from about 1.5 MiB to about 55 KiB (4 KiB with expiry) and stored state from about 230 KiB to
  about 8 KiB; native heap from about 1.6–2.5 MiB to about 55–90 KiB.
- **Through sessions** (R8, 400 seeded sessions of 3 people on 2 devices): 650 writers off, 27
  on (the bound 30).
- **Leaks dominate.** Without expiry, the writers that remain are almost all unclean ends that
  were never reclaimed. That is the case for expiry (Q1).

---

## 7. Slices (no estimates in time; by dependency)

1. Kit core: rows, selection, `release`, reclaim and hand-over (reordered), the assertion. R1,
   R2, R4, R6, R9, R10, R13.
2. Durability and N−1: `ended_*` and the load-time park. R3, R7, plus the contiguity and replay
   checks.
3. Client: `bye`, `kept`, `lease`, the `prevEnded` cap, the `writer=lease` capability, the
   updated docstrings and WIRE.md. R11 (JS), R12.
4. Expiry (if Q1 says build it). R5, and R8 with expiry.
5. K2 integration (§4), on K2's side.

---

## 8. Questions for a ruling

- **Q1. Park expiry.**
  - Without it, r0 is simpler, and every unclean end that is never reclaimed leaks one writer
    (about 3–4% of writers stay on in the model).
  - With it, the cap protocol (`prevEnded`, `ended_*`) is needed. It is designed above.
  - Recommendation: build the rows and the cap now (so the row format doesn't change later),
    ship expiry off by default, and turn it on after R5 and R11 pass.
- **Q2. Device key from the client.** Is a client-supplied browser id acceptable as the device
  key, given that safety doesn't depend on it (§2.6)? Or must K2 derive it server-side only?
- **Q3. Device tag under the doc's hash key.** A purge rotates the key, so each person-device
  draws one new writer after a purge. Accept that, or key the tag under a store-wide salt (a new
  `meta` key, again no format bump)?
- **Q4. Two store writes per connection** (lease at join, end at leave). Acceptable? The
  alternative is to batch the end write into the next commit, at the cost of one more parked
  state on a crash.

---

## 9. Rulings and what the build changed

- **Q1:** build the `prevEnded` cap, and the stale-lease and false-landing rows; expiry ships off
  (`person_writer_park_ms = 0`) and is turned on in a later round once those rows pass. Added row:
  a tab returning after its writer was let go gets a new writer, and its pending hunks land
  exactly once (`r5_returning_tab_after_expiry_lands_its_hunks_once`).
- **Q2:** a browser-supplied device id is acceptable: it isn't a security boundary. The API doc
  (`JoinOptions::device`) says so.
- **Q3:** accept one new writer per person and device after a purge (no store-wide salt).
- **Q4:** the row writes must ride in transactions that are open anyway. Built so: a join and a
  leave open **no** transaction; the rows changed since go with the next change's commit
  (`DocStore::commit_changes_with`) and the session's close. A lease that never landed a change
  and is lost to a crash leaves its writer as the store had it (free or parked): nothing of it
  is in the doc. A release lost to a crash leaves the writer parked (never handed out held).
  Join and leave latency, off and on: `tests/person_writers_cost.rs`.
- The row is 51 bytes (the layout above), not 44.
- **Rows only for writers that can write** (ruling on the build's first cut): a device join
  without edit rights holds a lease in memory but its writer gets no row (every row stays in the
  store for good, and a viewer-heavy doc would pile them up); the row is written once a
  connection holding it can write (the rights change, or its first change's commit).
  Row: `tests/person_writer_leases.rs` `a_read_only_lease_leaves_no_row_until_it_can_write`.
- **Measured memory per writer** (§6): Yrs heap 120–134 B, native heap 129–205 B, Yrs stored
  state about 18.5 B; the first estimate (300 B, 700 B) is withdrawn.
- **Join and leave latency** (release, 2,000 each, p50 / p99, reuse off → on): join 4 / 8 µs →
  2 / 3 µs; leave 1 / 1 µs → 1 / 1 µs; first edit 351 / 560 µs → 288 / 444 µs.
