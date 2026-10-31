# The live editor, M2: Re-apply in place, and the text store

**Status:** built on branch `live/m2` (head f988ae6); see "Built (live/m2)" just below. The
rest of the note is the design and the measurements it was decided on, as written before the
code. M1 (the live editor) is merged. The two M2 topics: Re-apply for rejections with several
hunks, through the server's rebase rule, and `LiveText`'s full-string rebuild on every edit.

## Built (live/m2)

Four steps, each gated (typecheck, the client suite, Chromium Playwright, the licence and
privacy checks, and the bench gate below). The coordinator's decisions on the questions at the
end of this note are in it: Q1 (b), Q2 `already-present`, Q3 our own rope, Q4 its own undo step.

**Topic 2, the text store**
- **The rope** (`client/src/rope.ts`, built in two steps). `LiveText`'s texts (the server's, the
  client's, each pending entry's base) are a persistent two-level rope: chunks of about 1,024
  UTF-16 units, never splitting a surrogate pair, in blocks of about 32 chunks, each level with
  its own UTF-16 and UTF-8 starts.
  - An edit rebuilds only the chunks its hunks touch, in at most three blocks, and copies the
    list of blocks (about 30 at 1 MB). Every other block and chunk is shared with the previous
    version.
  - Two ropes compare by identity where they share, so the equality checks no longer scan the
    text.
  - The API is unchanged: `text` is still a string, built on its first read after a change and
    cached.
- **`recarry`'s self-check** is a length check on the plain incoming path (nothing pending, no
  rejection being placed); the full check (an apply plus an equality) still runs whenever
  something was carried.
- **The editor's byte index** keeps line lengths in persistent blocks of 256 lines: an update
  copies the touched block and the list of blocks.
- **During an IME composition** (option C's concern, settled the other way): while remote changes
  are held, the binding keeps the held document and its byte index up to date change by change,
  so a keystroke no longer builds an index of the whole document.
- **Tests:**
  - a differential property test of the rope and the byte index against plain strings: random
    edits, CJK, emoji, surrogate pairs at chunk edges, tiny chunks and tiny blocks, empty and
    1 MB documents, every offset conversion, and that an edit never changes the version it came
    from;
  - planted faults in a chunk's cached UTF-8 length and in a block's total, which it must catch;
  - a test of the held document and index against `live.text` through a composition.
- **The bench gate** (`client/test/bench/live-text-bench.ts`, `M2_BENCH_GATE=1`): the three
  targets, pinned to one core. Every line carries the node's load. A miss is re-run once and
  fails only if the re-run misses too (a pass on the re-run prints `BENCH RETRY`); limits never
  change with load.

**The bench at the final head** (a build node, Node 22.23, one core; p95 per acknowledged
keystroke, typing at the end, middle and start; "Pre-M2" is the run this note was decided on):

| Target | Limit | Pre-M2 (ASCII / CJK) | Built: ASCII (end / middle / start) | Built: CJK (end / middle / start) |
|---|---|---|---|---|
| keystroke p95, 1 MB | 1 ms | 18.1 / 13.4 ms (middle) | 0.083 / 0.095 / 0.091 ms | 0.091 / 0.102 / 0.090 ms |
| keystroke p95, 4 MB | 2 ms | 73.2 / 63.7 ms (middle) | 0.069 / 0.110 / 0.123 ms | 0.093 / 0.114 / 0.108 ms |
| 500-event `since`, 1 MB (worst of 5) | 100 ms | 6.9 s / 5.1 s | 40.7 ms | 46.4 ms |

| At the final head, p95 | 1 MB ASCII | 1 MB CJK | 4 MB ASCII | 4 MB CJK |
|---|---|---|---|---|
| a remote change, with a pending edit | 0.053 ms | 0.072 ms | 0.061 ms | 0.081 ms |
| the binding's keystroke (CodeMirror `Text` plus the byte index) | 0.055 ms | 0.038 ms | 0.113 ms | 0.105 ms |
| a keystroke during a composition, remote changes held | 0.052 ms | 0.053 ms | 0.120 ms | 0.101 ms |
| the same, re-indexing per keystroke as before M2 (same run) | 3.5 ms | 3.2 ms | 15.3 ms | 13.8 ms |

The `since` replay, before and after each change, both versions interleaved in one job (p50 of
20 runs):
- the self-check trim and the copied chunk list: ASCII 85 → 60–63 ms, CJK 42–45 → 19–21 ms;
- the second level: ASCII 50 → 23–33 ms. CJK went 13 → 16 ms, but its per-method totals were
  equal, so that is within the noise at that size.

**Topic 1, Re-apply**
- **By the server's rule.** `LiveText` records each `edit` frame as sent. `placeRejection(id)`
  rebases it from its sent base through the history (`rebaseOurs`, as `edit_at_base` does), then
  through the person's own pending changes (ours after).
  - Refused: `conflict` (either rebase meets competing text, or the change never went out) and
    `cant-place` (a reload since, or past the 512-change history). Never placed by its region.
  - The result goes out as a fresh edit at the current version and is its own undo step.
  - Presence for a lost reply is unchanged and comes first. Nothing left to apply is
    `already-present`.
- **v0.9.8.2's single-hunk guards** (`not-insertion`, `reloaded`, `tie`, `deleted-around`, added
  after this note to stop M1's wrong placements) are replaced.
  - Their tests stay as regression tests, each with a planted fault (M1's placement at the
    region's start) that brings the old wrong result back.
  - `reapplyGuard` is a deprecated stub over `placeRejection` for one release.
- **Tests:**
  - the kit's `live_rebase` vectors through a replica, in both orders (the changes since before
    the refusal and after it);
  - the history's reach: placed after 512 changes, refused after 513 and after a reset;
  - placement through the person's own pending changes;
  - a server-log oracle on every applied Re-apply in the convergence harness, with fixed seeds 77
    and 213 for the multi-hunk race.
- **"Insert my text here"** (Q1 (b)), with the Chromium keyboard and `role="status"` tests:
  - a single-hunk action the person chooses;
  - it previews the typed text in the editor where it would go and announces the line;
  - it inserts only on Insert, as its own undo step; Cancel or Escape inserts nothing;
  - every rejected text has Copy, or is selected when the clipboard refuses.

**The Re-apply scan at the final head** (`client/test/bench/reapply-scan.test.ts`, seeds 1–500):

| Host | Applied | Same as `rebaseOurs` from the sent base | Different | Applied where the rule refuses | Refused that the rule would place |
|---|---|---|---|---|---|
| strict | 102 | 102 | 0 | 0 | 0 |
| strict, slow | 132 | 132 | 0 | 0 | 0 |
| no reconnect rule, slow | 120 | 120 | 0 | 0 | 0 |

- Under v0.9.8.2's guards the same runs applied 8, 10 and 4 and refused 94, 122 and 116 that the
  rule places; all of those are given back.
- The rest: `conflict` and `already-present`, plus `cant-tell` (12) on the host without the rule.
- The client's history agreed with the server's log in all 724 placements.

**What differs from the design above**
- **The rope is two-level, not one.** The one-level rope met the keystroke targets, but its edit
  copied the whole chunk list, which left the `since` replay with little headroom. Larger chunks
  were tried for that and dropped (the cost moved into scanning inside them, and the 4 MB
  keystroke got worse). Typed-array starts were tried and dropped too (no gain).
- **The equality checks are by identity**, not "O(chunks that differ)" in general: shared
  blocks and chunks compare by identity, and the rest by content. `recarry`'s check is a length
  check on the plain incoming path.
- **"Insert my text here" inserts only.** It puts the typed text at the change's tracked place
  and never deletes. For a replacement or a deletion, the preview's note adds "Your edit also
  removed text; that part isn't redone here." (`strings.insertRemovedText`). It previews before
  inserting, because it doesn't follow the server's rule.
- **Copy is a button on every row with text**, not only on refused ones.
- **The binding's composition path changed too** (the held document's index). The design listed
  only the byte index's per-line arrays as a follow-up.
- **Not built:** carrying open rejections eagerly past the 512-change history (the option under
  "When the history doesn't reach"). `cant-place` stays the answer there.
- **Browsers:** Chromium was gated. The IME check on the macOS web view stays the host's, as for
  M1.

The measurements come from two env-gated scripts under `client/test/bench/` (neither is product
code, and the suite never runs them):
- `reapply-scan.test.ts` (`M2_REAPPLY_SCAN=from-to`) watches M1's Re-apply in the mounted-editor
  convergence runs and works out the M2 answer for each rejection;
- `live-text-bench.ts` (`M2_BENCH=1`, bundled with esbuild, run with `node --expose-gc`) times
  `LiveText` by document size.

Both ran on a build node: Node 22.23, 4 CPUs allotted, pinned to one core with `taskset` (an
8-core x86-64 desktop part).
- **The numbers of record** are from a run at bb70b50: this branch with main 222b54f merged.
- An earlier run at 368ba1a agrees within about 5%. The scan is deterministic
  and gave the same counts.
- Numbers are V8's. The macOS web view is JavaScriptCore, so the browser check stays the
  host's, as for IME.

---

## Topic 1: Re-apply through the server's rebase rule

### What M1 does

- **One hunk:** Re-apply inserts `text` (the change's inserted text) at `region.start`, as a
  new edit. Presence by record comes first for a change whose reply was lost.
- **Several hunks:** refused: `{status: "unknown", reason: "multi-hunk"}`. The text is
  unchanged and the row is kept.

Reading the single-hunk path turned up four places where M1's result is not the change the
person made:

| Rejected change | M1's Re-apply |
|---|---|
| a deletion | returns `applied` and changes nothing (`!r.text`: dismissed) |
| a replacement | puts the new text back beside the old; the old text is never deleted again |
| an insertion at a point someone else also inserted at | `region.start` moves with `Bias.Before`, but the server puts ours after at a tie: the text can land on the other side |
| anything pending when the replica reloaded (`rejectAll`: `MPC-E112`, a divergence, a welcome with `text`) | the region is `{0, 0}`: the text goes in at the start of the document |

The scan counts the first three where both rules place a change: 33 of the single-hunk
changes M2 places (strict host, 500 seeds) come out with different text. 30 are tie
insertions, 2 deletions and 1 a replacement. The harness never reloads, so it doesn't reach
the fourth.

### The rule

Re-apply restores the change exactly as the server would apply it if the change were sent
now at its original base. If it can't do that, it refuses.

1. **Gates: unchanged from M1, in the same order.**
   - `landed`: returns `already-present`.
   - No such rejection, read-only, or an IME composition under way: returns `unavailable`.
   - A change whose reply was lost (`mayHaveApplied`) goes to `presence`:
     - `present`: returns `already-present`, and the rejection is dismissed;
     - `unknown` (no reach, or an undo unanswered on that connection): returns
       `unknown/cant-tell`, and the rejection is kept;
     - `absent`: carry on.
   - The rebase runs only after `absent`. `onLanded`, `onDuplicateRisk` and `noteReapplied`
     work exactly as in M1.
2. **The change as sent.** A new private field records the `edit` frame's own `hunks` and
   `base` (`Entry.sent`, set in `flush`), and the rejection keeps it.
   - `Rejection.hunks` and `Rejection.base` don't serve. They are the as-made form: for an edit
     typed behind a pending one, its hunks are on the server's text *plus* the pending changes,
     which is not the text of any server version.
   - A change refused at `flush` for its size (`MPC-E113`) has a sent form: the frame it would
     have been.
   - A queued change that never went out has none. It left in `recarry` because it met
     competing text (or because an earlier change it built on left). Re-apply refuses it with
     `unknown/conflict`.
3. **Since: the history after `sent.base`.**
   - This is the client's record of every server change, its own acknowledged edits included,
     which is the server's window for that base.
   - Since starts at the edit's base, not at `asOf.version`, because the hunks are on the text
     at `sent.base`. `asOf` only gates presence.
   - It requires `sent.base >= historyFrom` and consecutive versions up to `version`.
   - (Carrying from the refusal version instead gives the same result, because the carry goes
     step by step and the client already carried the in-flight change that far. It isn't
     needed.)
4. **`rebaseOurs(sent.hunks, since)`**, the function the server's `edit_at_base` runs. If it
   conflicts, Re-apply refuses (`unknown/conflict`) and the rejection is kept.
5. **Through the replica's own pending changes:** the in-flight change if it isn't doomed,
   then the queue. Each step is `Carried.through`, ours after, because the server will apply
   those changes first. A conflict with the person's own pending text refuses too.
6. **Nothing left:** for example, a deletion whose text someone else deleted since. The server
   would apply nothing, so Re-apply dismisses the rejection and returns `already-present`.
   Today M1 says `applied`. (Q2.)
7. **Otherwise the hunks go in as one fresh local edit** (see "The send" below).

**When the history doesn't reach, Re-apply refuses** (`unknown/cant-place`). This covers past
the 512-entry cap, and anything after a reset: `reset` clears the history, so every rejection
from `rejectAll` refuses this way. It never falls back to the region. That removes the
offset-0 case above.

A later option would remove the cap's limit: carry each open rejection's sent form through every
server change as it arrives, the way regions move today. It costs state and per-change work,
and is only worth it if dogfood shows `cant-place` from reach. It can't help after a reset.

### What the scan shows

`reapply-scan.test.ts` ran seeds 1–500 under three harness settings, each with `reapplyAll`:
- strict: the host follows the reconnect rule;
- strict, slow: 50 steps, 6 drops, latency up to 120;
- no-rule, slow: keepalive leave, fast notice, late old frames.

For each Re-apply, the table shows M1's answer and what M2 would do. "placed" includes changes
placed through pending edits; "(empty)" means nothing was left to apply.

**Strict host (3,452 Re-applies):**

| Rejection | Count | M1 | M2 |
|---|---|---|---|
| several hunks, lost reply (`E000`, `asOf`) | 17 | unknown (multi-hunk) | placed 13, placed (empty) 3, conflict 1 |
| several hunks, `MPC-E111` | 350 | unknown (multi-hunk) | conflict 350 |
| several hunks, foreseen conflict whose refusal was lost (`E000`, no `asOf`) | 59 | unknown (multi-hunk) | conflict 59 |
| several hunks, queued, met competing text | 488 | unknown (multi-hunk) | refused (no sent form) 488 |
| one hunk, lost reply | 215 | applied | placed 89, placed (empty) 100, conflict 26 |
| one hunk, `MPC-E111` or foreseen | 1,096 | applied | conflict 1,096 |
| one hunk, queued, met competing text | 1,227 | applied | refused (no sent form) 1,227 |

**No-rule slow host (4,532):**
- Several hunks: 1,329, of which 25 are placed (all lost replies, 4 of them empty).
- One hunk: 3,203, of which 230 are placed; 12 are `cant-tell`, as in M1.

**The oracle agrees every time.** For each placed change, the scan compared `rebaseOurs` over
the client's history with `rebaseOurs` over the fake server's own log. They matched in 724 of
724 cases, and every placed edit fit the server's text.

**What this means: the rule buys correctness, not coverage.**
- 98% of the multi-hunk rejections stay refused under M2: 898 of 914 on the strict host. The
  M1 no-rule figure of 206 unknowns in 690 Re-applies is the same shape.
- The ones M2 restores are lost replies. Every other multi-hunk rejection met competing text.
  Running the rule that refused a change again reaches the same conflict at the same step, by
  construction: the carry is a pure function over the same prefix of changes.
- The harness types random edits, so conflicts are commoner here than in real use. The split by
  class is still the point.
- The same holds for one hunk. On the rebase path, 2,349 of 2,538 single-hunk Re-applies that M1
  applies would refuse.

### Single hunk: move it to the rebase path?

Split by why the change was refused:

- **Not competing text** (a lost reply found absent, a rate limit, a claim or backlog refusal,
  the frame cap): **move it.**
  - M1 gets deletions, replacements, ties and reloads wrong (the table above). The rule gets
    them right, and the vectors pin it.
  - There's one exception: a lost reply that met competing text while it was lost (26 of 215
    on the strict host). That is the next class.
- **Competing text** (`MPC-E111`, foreseen, or queued): the rule refuses all of these. This is
  the risk. In the strict runs they are 93% of single-hunk Re-applies (2,349 of 2,538), and on
  a hot spot they are the button's main use.

**Recommendation (Q1):**
- Re-apply goes through the rule for every hunk count.
- A conflict row loses the in-place Re-apply.
- For a **single-hunk** conflict, keep M1's insertion as its own, honestly labelled action:
  "Insert my text here". It inserts the typed text at the tracked region and never deletes.
  - Its worst case is visible extra text, never lost or misplaced deletions.
  - It isn't a guess about where *each* hunk goes, because there is one.
- A multi-hunk conflict gets no action beyond copying its text (the row already shows it).
- Partial Re-apply (place the hunks that don't conflict, list the rest) stays out: it makes an
  edit the person didn't make. That is the same reason `partly-present` went.

### The send: a fresh edit at the current version

| | Fresh edit at the current version (chosen) | Edit at the old base, server rebases (`edit_at_base`) |
|---|---|---|
| Who rebases | the client, before any text changes; the server then carries it only through what arrives while it's in flight | the server, over the whole window since the base |
| WIRE.md | as every edit: `base` is the latest version seen, rule 0 holds | breaks "base is the latest version it has seen"; the client's carry of its in-flight change assumes `e.base == version` |
| Optimistic text | the client must compute the placement to show it anyway | the same computation, then the server's again: two chances to differ, no gain |
| Base past the window (1,000 changes / 4 MiB) | can't happen: the client refuses locally (`cant-place`) | `MPC-E112`: `reloadNeeded` → `rejectAll`. One click on Re-apply would reload the editor and reject every other pending edit |
| A conflict | found locally: nothing shown, row kept | the text goes in, then comes out on `MPC-E111`: a flicker and a new rejection for a conflict the client could see |
| A conflict with changes not yet seen | normal path: `MPC-E111` or foreseen; the re-applied edit becomes a *new* rejection (the old one was dismissed) | the same |

The fresh edit goes through the editor:
- one CodeMirror transaction built with `changesOf(doc, byteIndex, hunks)`, `userEvent:
  "input.reapply"`;
- the binding turns it into one `applyLocal`;
- the binding today folds `input.reapply` into the person's typing group. **Proposed:** its own
  undo group, so one undo takes back exactly the Re-apply (Q4).

### API and docs

**`LiveText`** (additive):
- `placeRejection(id): {hunks: Hunk[]} | {refused: "conflict" | "cant-place"}`: the hunks are on
  `text`, steps 2–6. The editor needs the placement before dispatching through CodeMirror.
- `reapply(id, opts)` keeps its signature: `OpId | null`, with `null` on any refusal.
- The sent form stays private (`Entry.sent`, and a map by rejection id, pruned on dismiss and
  landing).

**`ReapplyResult.reason`:**
- add `conflict` and `cant-place`;
- `cant-tell` stays;
- `multi-hunk` is never returned again (keep it in the type for one release);
- strings: `conflictReapply`, `cantPlace`; `multiHunk` goes.

**Docs:**
- EDITOR.md: rewrite the Rejections section, including the conflict action from Q1.
- WIRE.md: no frame changes. One line under "What the client guarantees": Re-apply places a
  change by the server's rule, or refuses.
- CHANGELOGs.

### Tests to come

1. **Planted fault: "first hunk place".**
   - A test-only switch puts all the typed text at `region.start`: M1's behaviour before the
     refusal, as pinned in 86dba8f.
   - The multi-hunk race seed and the vectors must fail with it.
   - Seeds where it differs from M2: strict 77 and 213; strict slow 26, 171, 214 and 404; no-rule
     slow 3, 151 and 167.
2. **Multi-hunk race seed.** A fixed convergence seed with a lost multi-hunk change found
   absent and placed. Strict seeds include 39, 77, 83 and 131; strict slow seeds include 2, 26
   and 74. Prefer 77 or 213, where the planted fault also shows.
   - Oracle: the re-applied edit's hunks equal `rebaseOurs(sent, server log since base)`,
     computed from the fake server's log, not the client's history.
   - Then every editor converges with the server.
3. **Conflict refusal**, scripted:
   - a two-hunk change, one hunk replacing text someone else replaced: `MPC-E111` →
     `unknown/conflict`, the text unchanged, the row kept;
   - the same foreseen (doomed in flight) and queued.
4. **Reach:**
   - 513 changes after the base: `cant-place`;
   - after a reload (`MPC-E112`): `cant-place`, never offset 0.
5. **Pending:**
   - Re-apply with the person's own edits in flight places through them, ours after;
   - a conflict with their own pending text refuses.
6. **Vectors.** `client/test/vectors/live_rebase.json` has 215 cases: 128 ok, 87 conflict, 80
   with several hunks of ours. For each:
   - a replica at `base`, `ours` sent and refused (`MPC-E901`), `since` delivered as changes;
   - Re-apply must give `apply(after since, server.ok)`, and refuse on every conflict case;
   - run twice: `since` delivered before the refusal (carried in flight, doomed when
     conflicting) and after.
   - Also run through the mounted editor with CJK and emoji in the hunks: the UTF-16
     dispatch, and the hunks the binding sends back equal the placed hunks.
7. **Single-hunk regressions:**
   - a deletion is deleted again (or `already-present` when nothing is left);
   - a replacement replaces;
   - a tie insertion lands on the server's side.
8. **The convergence harness's `reapplyAll`** gains the oracle check of item 2 on every
   `applied`.

---

## Topic 2: LiveText's full-string rebuild per edit

### What it costs now

**Typing, each keystroke acknowledged before the next** (`applyLocal` + `setCursor`, then the
`ack`). Time per keystroke in ms, p50 / p95 / max:

| Document | At the end | In the middle | At the start |
|---|---|---|---|
| 100 KB ASCII | 2.37 / 2.74 / 5.9 | 1.51 / 1.71 / 5.4 | 0.65 / 0.69 / 4.0 |
| 100 KB CJK/emoji | 1.66 / 1.71 / 5.5 | 1.08 / 1.11 / 4.8 | 0.50 / 0.53 / 3.8 |
| 1 MB ASCII | 25.0 / 28.3 / 35.5 | 16.3 / 18.1 / 22.0 | 7.54 / 9.13 / 15.1 |
| 1 MB CJK/emoji | 17.8 / 19.5 / 29.6 | 11.9 / 13.4 / 21.5 | 6.07 / 7.59 / 15.2 |
| 4 MB ASCII | 97.3 / 107 / 114 | 63.2 / 73.2 / 80.1 | 29.8 / 41.5 / 45.2 |
| 4 MB CJK/emoji | 71.4 / 84.9 / 88.5 | 48.1 / 63.7 / 69.6 | 24.9 / 38.0 / 41.4 |

At 10 KB: p50 0.16 ms, p95 0.17 ms.

**Other paths, p50 / p95:**

| Document | Typing while one edit is in flight (queued, compose), per keystroke | Remote change, per change | `since` reply of 500 events |
|---|---|---|---|
| 100 KB ASCII | 1.00 / 1.10 | 1.36 / 1.91 | 636 ms (1.27 ms/event) |
| 1 MB ASCII | 10.4 / 12.0 | 14.1 / 19.6 | 6.9 s (13.8 ms/event) |
| 1 MB CJK/emoji | 8.0 / 9.6 | 10.7 / 14.6 | 5.1 s (10.2 ms/event) |
| 4 MB ASCII | 41.3 / 43.9 | 56.2 / 77.7 | 27.3 s (54.6 ms/event) |
| 4 MB CJK/emoji | 32.2 / 33.8 | 43.2 / 59.2 | 19.6 s (39.3 ms/event) |

- With a local edit pending, a remote change costs about the same as without.
- The CJK/emoji texts have fewer UTF-16 units for the same bytes, so the same bytes cost less
  here.
- A `since` replay at 1 MB blocks the page for 5–7 s, after every reconnect that resumes 500
  changes behind.

### Where the time goes

A CPU profile of 150 acknowledged keystrokes at 1 MB, by self time:
- typing: `validate` 37%, `apply` 33%, `utf8Len` 22%, `settleInflight` 6%, GC 2%;
- remote changes with a pending edit: `validate` 45%, `apply` 41%, `recarry` 7%.

None of the cost is hunk logic. Every one of these is a scan of the whole text, character by
character, in JavaScript.

**The primitives at 1 MB ASCII (and 1 MB CJK/emoji), p50 ms:**

| Primitive | ms | |
|---|---|---|
| `utf8Len` (the whole text) | 2.17 (1.79) | about 2 ns per UTF-16 unit, linear in size |
| `validate`, one hunk | 4.21 (3.52) | `utf8Len` plus `isBoundary` (a `byteToIndex` from 0) per end |
| `apply`, one hunk | 1.45 (1.01) | the scan from 0 to the hunk; the concat itself is a V8 cons string |
| flatten after a splice | 0.46 (0.47) | the copy V8 makes at the next read of a spliced string |
| `compose`, 1+1 | 2.33 (1.86) | `utf8Len(t0)` and the walk |
| two equal strings compared | 0.03 | `===` on distinct strings is a memcmp, cheap |
| `Buffer.byteLength` / `TextEncoder.encodeInto` | 0.09 / 0.05 (1.1 / 1.2) | native byte counts are fast only for one-byte strings |

**One keystroke at 1 MB, and how each step scales:**
- `applyLocal`: `validate` (one `utf8Len`, two `byteToIndex`) and `apply` (a scan to the
  hunk).
- `setCursor`: returns early while an edit is in flight.
- The ack: `settleInflight`, which runs two `apply`s and a comparison.
- Then `flush` → `sendCursor` → one `utf8Len`.
- Every step is O(document); typing at the end costs most because the scans run to the hunk.
- A queued keystroke adds `compose` (`utf8Len(t0)` again).
- A remote change runs `validate` and `apply` on `serverText`, one `apply` per pending entry,
  `recarry`'s check `apply(oldText, x)`, the comparison, and `sendCursor`.

**History** costs nothing measurable (`pushHistory` shifts at most 512 entries).

**GC** is not where the time goes: about 2–3% in the profiles, one or two collections per
scenario. A few runs (1 MB CJK at the end, 4 MB ASCII at the end) hit bursts of about 75
collections and 160 ms. Each edit leaves several whole-document strings behind, about 2 bytes
per unit for CJK; at 4 MB that is tens of MB of garbage per second of typing.

**The binding also has O(lines) work per keystroke.**
- The editor's own byte index (`ByteIndex.update`) copies its per-line arrays on every
  transaction.
- The CodeMirror model below includes it: 0.30 / 0.37 ms (p50 / p95) at 4 MB ASCII and
  0.27 / 1.99 ms at 4 MB CJK, against 0.03 / 0.05 at 1 MB.

### The target, and the baseline against it

**Proposed targets** (acknowledged typing in the middle, remote changes, on this class of
machine):

| Metric | Target | Baseline (ASCII / CJK) | Factor |
|---|---|---|---|
| keystroke p95 at 1 MB | under 1 ms | 18.1 / 13.4 ms | ~18× |
| keystroke p95 at 4 MB | under 2 ms | 73.2 / 63.7 ms | ~37× |
| remote change p95 at 1 MB | under 1 ms | 19.6 / 14.6 ms | ~20× |
| `since` of 500 at 1 MB | under 100 ms in all | 6.9 s / 5.1 s | ~69× |

Today p95 under 1 ms holds only below about 50 KB.

### Options

Two bench-only models give the floor of each kind of store. They cover one keystroke's store
work only, not all of `LiveText`'s.

| Model | 1 MB p50 / p95 | 4 MB p50 / p95 |
|---|---|---|
| CodeMirror `Text.replace` + `ByteIndex.update` + one lookup | 0.032 / 0.045 ms (CJK 0.025 / 0.043) | 0.30 / 0.37 ms (CJK 0.27 / 1.99): the index's O(lines) copies |
| flat string spliced, one 2,048-unit chunk re-measured, later chunk starts shifted | 0.46 / 1.88 ms (CJK 0.47 / 1.91) | 1.85 / 3.04 ms (CJK 1.82 / 2.90) |

**A. An own rope (recommended).**
- **What it is:** a balanced tree of chunks (about 1–2 K UTF-16 units). Each node caches its
  UTF-16 length and its UTF-8 length, so:
  - a hunk applies in O(log n + chunk);
  - byte↔index and boundary checks descend the tree;
  - `utf8Len(text)` is the root's count.
- **The replica's strings become ropes:** `serverText`, `_text` and each entry's `before`.
  They share structure, so three copies of a 4 MB text become one plus the changed chunks.
- **The checks stay exact and cost O(chunks that differ):** `settleInflight`'s "the server's
  rebase differs from the client's" and `recarry`'s self-check compare trees with an identity
  shortcut over shared nodes.
- **Every caller moves to the rope:** `validate`, `apply`, `compose` (its `t0` length),
  `invert` (today it encodes the whole text with `TextEncoder`, on the rejection path only)
  and `diffText` (resets only).
- **Why it's recommended:**
  - one structure answers both units the client needs (UTF-8 on the wire, UTF-16 for JS);
  - the root entry stays free of dependencies;
  - nothing to vet for a licence.
- **Cost:** about 400–600 lines, plus a differential property suite against today's string
  functions. Those stay in the tests as the reference.
- **Expected:** near the CodeMirror model, which is a rope of lines, so well under 1 ms at 1 MB.
  This is to be shown by the bench.

**B. A flat string with a chunked UTF-8 offset index.**
- **What it is:** keep the JS string; replace the scans with a chunk table (binary search, then
  scan inside one chunk).
- **The smallest change.**
- **But it misses the target:** V8 copies the whole string after every splice, at the next
  read. That is 0.46 ms per MB, and the model's p95 is 1.88 ms at 1 MB.
- The equality checks stay O(n), though they are cheap (0.03 ms/MB).
- Fresh whole-document strings keep the GC pressure.
- **A fallback** if A slips: it still gives roughly 8× at 1 MB.

**C. CodeMirror's `Text` as the store.**
- **What's good:** proven, MIT, and fastest in the model. The editor's document could be the
  replica's (one copy).
- **Against:**
  - `LiveText` is in the root entry, which works without CodeMirror (the peers are optional
    there). C makes `@codemirror/state` required for every host, including ones with another
    editor or none.
  - `Text` is UTF-16 lines, so the UTF-8 index still has to be ours, and `ByteIndex` needs a
    tree at 4 MB anyway.
  - The replica and the editor's document are deliberately apart during an IME composition
    (held changes), so sharing one store fights that design.

**Either way, as a follow-up in the same milestone:** `ByteIndex`'s per-line arrays become a
Fenwick or segment tree, so the binding's per-keystroke cost is O(log lines), not O(lines).

### The public API

Unchanged:
- `text` stays a `string` getter. It is built on first read after a change and cached. Nothing
  in the editor reads it per keystroke (`repair` only), but a host that reads it per change
  pays O(n).
- `applyLocal`, `onChange` and every event keep their shapes. The hunks are UTF-8 as today.

Docs:
- **EDITOR.md:** add a performance line ("`LiveText.text` is O(document) on first read after a
  change; prefer `onChange` hunks") and the measured targets.
- **WIRE.md:** no change.

---

## Questions for the coordinator

- **Q1. Conflict rejections.** Under the rule they never Re-apply in place, whatever their hunk
  count. Choose one:
  - (a) refuse all; copy the text only;
  - (b) recommended: refuse in place, and keep M1's insertion as a labelled single-hunk action,
    "Insert my text here";
  - (c) keep M1's behaviour for single-hunk conflicts under the Re-apply name.
- **Q2. Nothing left to apply** (an empty rebase): `already-present` (proposed) or `applied`.
- **Q3. The store:** an own rope (proposed) or `@codemirror/state`'s `Text`, which would make it
  a required peer of the root entry.
- **Q4. Re-apply's undo group:** its own undo step (proposed), or folded into the typing group
  as in M1.
- **Q5. Unverified, found while reading `onEvent`.**
  - If one change touches two text roots and the other root's event arrives first, the client
    takes that version for that root and drops this root's event at the same version
    (`ev.version <= this._version`).
  - That would lose a change, and the history M2 rebases over would miss it.
  - Can a multi-root change reach the live wire (a pack with more than one text root)? If yes,
    it needs a test and a fix before M2 relies on the history.
