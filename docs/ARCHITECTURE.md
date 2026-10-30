# Architecture

```
 type packs (one repo each)       markdown · docx · csv · ...
   schema on primitives, codecs + fidelity tests, editor binding, verb map + parity list
 ───────────────────────────────────────────────────────────────────────────────────────
 k2-mpc-base (this repo)
   Doc + primitives ─ text algorithms ─ codecs ─ sessions ─ presence ─ store ─ history
   parity framework (verb registry + harness) ─ Host trait (identity, permission, files)
 ───────────────────────────────────────────────────────────────────────────────────────
   engine (private): Yrs today, behind the EngineDoc seam
```

## Rules

1. **Packs never touch the engine.** The public API is `Doc`, `Schema`/`Kind`, `Edit` (data),
   opaque wire/store bytes, and engine-independent algorithms. Replacing the engine changes
   `src/engine/` only.
2. **The host owns identity, permission, transport, time and file watching.** The kit asks
   through the `Host` trait (rights, clock, audit) and takes file events as calls
   (`file_changed`, `tick`). It reads and writes the files itself (atomic write-back, outside
   edits merged in) but starts no threads and watches nothing. No host-specific code here.
   The session API the host codes against is in [SESSIONS.md](SESSIONS.md).
3. **Agents are participants.** Every capability a person's editor uses is in the API, so a
   command-line verb can do the same thing (see the parity framework).
4. **No untrusted bytes reach the engine without the strict decoder.** Decoding client bytes
   (`encode_diff`, `check_client_update`, `apply_client_update`) sits behind the
   `untrusted-wire` feature, and a host doesn't enable it until the strict decoder lands. That
   decoder decodes strictly, bounds every count, rejects overlapping block ranges, validates,
   and **re-encodes canonical bytes** before the engine or any other client sees them. The
   daemon never relays raw client bytes. Until then, every byte the engine decodes was made
   by this kit (agents send text). Today's shape pre-checks plus `catch_unwind` are not
   enough on their own.

   **The client-ID rule** (`check_client_update`), for an update from the connection that
   owns client `A`:
   - new content (any block past the session's state) may come only from `A`;
   - blocks from another client `B` are accepted only if **already known**: the update's
     highest clock for `B`, over every item and GC block, is at most the session's state
     vector for `B`. A Yjs client that went offline holding `B`'s update as pending sends it
     back in its resync (step 2), so refusing these would refuse real resyncs. They are
     no-ops on apply: blocks are identified by `(client, clock)` and known ones are skipped,
     so forged content under a known ID changes nothing;
   - one clock past the session's state for `B` is `ForeignClient`;
   - no client's blocks may start past the session's state (`MissingDeps`: resync);
   - the shape pre-check stays O(size): the client count is at most the bytes left / 3, and
     the first client's block count at most the bytes left.

   A doc an apply leaves with pending updates (or that failed or panicked mid-apply) is
   **poisoned**: it never encodes, and sessions reload it from the store.
5. **Apps and editors bind to the kit's SDK** (the JS client and binding adapters), never to
   `y-*` packages. The engine can only be swapped on the server, at migration time; the SDK
   is what keeps packs and Apps unchanged.
6. **One change per participant edit, made on the doc itself as the participant.**
   `Doc::edit_as` applies a list of edits as one change, attributed to one client, in one
   engine transaction **on the doc itself, as that client** (`transact_mut_as`, the vendored
   engine's k2 patch: `docs/YRS-FORK.md`). Nothing per participant is kept. *Why (v0.9.3):*
   until v0.9.2 each edit was made on a throwaway replica built from the doc's full state
   and dropped, so that memory was never the doc times its agents; but the engine keeps an
   item per insertion ever made, so every edit cost O(history) (110 ms at 300k changes,
   `docs/evidence/v0.9.3-edit-path.md`). On the doc itself, an edit costs the change plus
   the engine's walk to its position, and no memory. *Safe because* the doc is the only
   writer of every client ID it edits as: its owner holds it exclusively while it edits (a
   session's lock), and it holds everything that client ever wrote. A second writer of one
   `(client, clock)` would be dropped silently by every peer (`session/one_writer_tests.rs`
   shows it, and checks the invariant under a random workload). *Checked continuously:* one
   edit in `Config::edit_check_every` (default 1,000) is also made the old way, through a
   throwaway replica from the state before it, and the two updates compared byte for byte
   (the engine's text cache too, against a fresh read). A mismatch is never served: the doc
   takes the replica's update, reports `AuditEvent::EditPathMismatch`, and makes its edits
   through the replica until it is reloaded.
7. **The file stays true, and the disk is never the merge authority.** A session writes its
   file back atomically and merges outside edits in as a CRDT change made from the file's
   last synced state (kept in the store, so restarts merge correctly too). It never writes
   CRDT metadata into the file, never rewrites an untouched file, and never replaces the
   session with the file's content.
8. **The kit holds only root shared-type handles** (v0.9.7): a handle on a doc comes from
   the doc's root (`get_or_insert_*`), never from a nested shared type, and the kit never
   keeps a handle across an apply other than a root's. *Why:* a nested handle can outlive
   its type after a remote delete; a root's never does. *Checked:* the engine module takes
   handles in one place, which asserts a root in debug builds, and a test fails if the
   module names a nested type or any doc it makes keeps a non-root handle
   (`engine/yrs.rs`, `root_handles`).

## Tiers

- **Tier 0, any file:** presence, who's viewing, a lease, versions and restore, on the blob
  service. **Leases are not the only guard:** desktop apps ignore them, so tier 0 also merges
  on save, or keeps a conflict copy when it can't. Leases expire, and an agent blocked by
  one gets a clear exit code.
  **Built** (`BlobSession`, the `blob` module; SESSIONS.md "Tier 0"): a save under someone's
  lease stays current, and what it replaced is kept as a version and a conflict copy (in a
  folder the host chooses, `Host::conflict_dir`; deleted after 30 days unless named or
  edited). Its verbs are classified by tier in the registry (`VerbSpec::tiers`,
  `Registry::for_tier`) and proven by the tier-0 parity harness (`parity::check_blob`).
- **Tier 0.5, a lossless codec but no live editor:** an **agent branch plus a 3-way merge
  through the type's codec**: import base, ours and theirs into primitives, merge, export,
  then the fidelity check. Agents get document-level merges years before tier 1.
- **Tier 1, a semantic pack:** live co-editing on the pack's schema.

## Layout of packs and the kit

Packs depend on the kit by a sibling path (`../k2-mpc-base/crates/base`). Check the
repos out side by side; a host that vendors both keeps them side by side too.
