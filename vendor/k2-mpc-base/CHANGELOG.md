# Changelog

## v0.1.0 (2026-10-27)

First release of the Multi-Player Cores (MPC) base kit. The API contract is
`docs/SESSIONS.md` (amendments 1–6 are listed in its changelog).

**What's in it**
- **Docs on K2-defined primitives.** Text is implemented; the other kinds are specified in
  `docs/PRIMITIVES.md`. The CRDT engine (Yrs 0.28) is private.
- **Sessions.**
  - Presence per connection; rights per participant through the `Host` trait.
  - Three edit paths: `edit_with`, `edit` + `if_version`, and `write_base` (three-way merge,
    no last-writer-wins).
  - Change events; a memory pool with LRU unload; rate limits.
- **Disk sync.**
  - Atomic write-back that keeps the file mode; echo suppression.
  - Outside edits merged as attributed changes, with versions around them.
  - Load reconciliation; crash-safe write-back.
- **Text tools (`run_tool`).**
  - Read verbs `cat/head/tail/wc/grep/sed -n/diff/view`; edit verbs
    `sed -i/patch/append/insert/replace/delete`, each one atomic change.
  - Dangerous `sed` commands are refused; an injection fence with a per-call nonce.
- **History.**
  - Automatic and named versions with retention; `log`, `diff`, `show`.
  - Restore as a forward change.
  - Per-person undo to the undo spec v1.4: per-hunk regions, `--partial`, `--drop`, redo.
  - `revert` of a named change, from a persisted record with an O(log R) position carry.
- **On-behalf edits and approvals; a history right; a host key-value table.**
- **Tier 0 for any file.** Keyed content-addressed blobs, presence, a lease, versions and
  restore, plus conflict copies when a desktop app ignores the lease.
- **Manager.** Listing and rights re-check; `delete_doc` / `delete_space` across every kit
  table.
- **Agent parity framework.** `PersonAction` P1–P31 (exhaustive), the `KIT_VERBS` registry,
  pack verbs (`Pack::verbs`), and the harness comparing the person path with the agent path.
- **Conformance suite** for text. Two genuine Yrs deviations are pinned:
  `text.yata_separating_case` and `text.yata_paper_a19`.
- **Hardening.**
  - Shape pre-checks on outside bytes; client-byte decoding gated behind the off-by-default
    `untrusted-wire` feature.
  - A deterministic, counted-work diff.
  - Encode paths never panic on a pending doc.

**Verified locally (CI-size):** the full test suite (unit, integration, property, the
convergence simulator at 40 seeds, the disk simulator, tier-0 large-file and crash tests,
golden tool parity against hand-written GNU expectations, 57 parity cases), plus clippy
`-D warnings`, rustdoc, and the licence gate.

**Pending, to run on a Linux compute node** (the same tag; any failure ships as v0.1.1):
- the live GNU comparison of the 407 tool cases (`scripts/gnu-parity.py`);
- the convergence sweep at ≈ 200 seeds and the disk simulator at 40+ seeds;
- a 10-minute, 32-participant soak;
- the revert-carry benchmark at R = 10^5 (≤ 20 µs p95).

**Not in this release:** erasure (purge + compaction + byte scan); blame; the WS sync protocol
and JS client; the rich-text, list, map and tree kinds.
