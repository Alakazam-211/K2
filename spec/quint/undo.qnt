// -*- mode: Bluespec; -*-
//
// undo: the kit's undo spec (docs/SPEC-UNDO.md, v1.6; U1–U22a, O1–O5, D1–D3) over abstract
// text, for the model check `MC text-undo` (spec §8). Transcribed definition by definition
// from the oracle of record, `spec_undo::model` (crates/spec-undo/src/model), and driven
// by the same bounded world as the Rust enumerator (crates/spec-undo/src/bounded.rs), so a
// Quint state `st` is exactly a `bounded::View` and TLC's distinct-state counts equal
// `undo_enum bfs`'s. Each definition names the oracle function it transcribes.
//
// ## Abstraction (bounded.rs's module docs say the same)
//
// - One root. A character is an element { ch, au }: its letter (0 = a, 1 = b; no newline,
//   so every report's lines are (1, 1) and are left out) and its author (0: the seed text,
//   an import; pid k: author k). Element identity beyond the author is not kept: `own_only`
//   checks who owns what an undo deletes.
// - Pids 1..PIDS, acting for nobody, every right. EVENTS per pid (an edit or a purge),
//   UNDOS shared (undo all or nothing / --partial, --steps 1 or 2; --drop; redo all or
//   nothing / --partial), REVERTS shared (any recorded change, by any pid, all or nothing
//   or --partial).
// - Edits are explicit canonical hunks: insert a letter, delete one, replace one with the
//   other letter, delete two, and (MULTI) two single-letter deletes or replacements at
//   least one letter apart. A purge (PURGE) deletes every live copy of a letter as the
//   purger's batch, one hunk per copy (touching hunks are not joined: purge_text builds
//   them itself), then clears the stacks (U19).
// - The cut: no state with a text longer than MAXLEN (at most 4). So the merge only diffs
//   texts up to 4 long, where `undo_diff.qnt`'s pinned diff is the kit's diff::hunks
//   (D1–D2) on every pair (crates/spec-undo/tests/quint_diff.rs).
// - REDUCE: once the revert budget is spent, records (and head) are dropped: nothing reads
//   them again. Fields no rule reads are not state (a record's original hunks, a batch's
//   after_partial and on_behalf_of, the authors sets: no restart here).
// - Not modelled (the trace test covers them, §8): restart, the revert window, write
//   --base, replace_text, restore, undo --for, on-behalf edits, the U6 bounds (100 batches
//   and 4 MiB per pid are never reached here), purge redaction of records (R9's 8-byte
//   windows never match a one-letter secret), HistoryLost (needs a restart).
//
// ## Readings (the oracle's; crates/spec-undo/src/model/mod.rs)
//
// R1 (a bound strictly inside a deleted range moves to the range's start), R2 (one step's
// regions merge against the step's starting text), R3 (a clean region applies the merge's
// own hunks), R4 (a redo region is the undo region as the undo left it), R6 (the purge
// notice: said once, withdrawn by a new batch), R7 (--steps N with fewer batches undoes
// what there is), R8 (revert is one undo step; erased before tainted), U22a v1.6 (revert's
// per-edge carry, inheritance at touching regions, then the clamp). This model's own: an
// undo or redo request spends the budget whatever it finds.
//
// ## Invariants (rule ids; each transition records what it violates in `st.viol`, as
// bounded.rs's `State::check` does)
//
//   inv_own_only  hist.undo_own_only (U10 element identity, U18a): every character an undo
//                 or redo by p deletes is p's; every one a revert of change c deletes is
//                 c's author's. FINDING F3: the oracle of record violates it (depth 3), so
//                 it is checked apart (docs/MODEL-CHECK.md).
//   inv_U9        a tainted region is never applied (U9).
//   inv_U13       a refused all-or-nothing request, or a blocked one, changes nothing.
//   inv_U15       all or nothing's conflicts = --partial's reports, first conflicting step.
//   inv_U17       a step pushes one region per region it applied (redo reapplies exactly
//                 the applied regions).
//   inv_U19       a purge leaves every stack empty.
//   inv_U22a      revert's conflicts ⊇ undo's on the same state, same bounds, same text.
//   inv_O5        O5 lands exactly at the doc's end with a non-empty whole-doc base: never
//                 elsewhere, and never refused there (U10's at_end, both ways).
//   inv           all of them but own_only.
//
// ## Mutants (MUTANT; the same set as crates/spec-undo/src/model/mutant.rs)
//
//   "none", "i-no-taint", "iii-partial-hides-taint", "iv-redo-retries-skipped",
//   "vi-o5-every-window-end", "vii-o5-region-base", "skip-u19".

module undo {
  import undo_diff.* from "./undo_diff"

  const PIDS: int
  const EVENTS: int
  const UNDOS: int
  const REVERTS: int
  const MAXLEN: int
  /// the seed text (author 0), letters 0/1
  const INIT: List[int]
  const MULTI: bool
  const PURGE: bool
  const REDUCE: bool
  const MUTANT: str

  // ---------------------------------------------------------------------------------------
  // Types

  type Elem = { ch: int, au: int }
  /// model::Region (root is always 0; `bdn` = base_doc_nonempty)
  type Region = {
    start: int, end: int, before: List[int], after: List[int],
    tainted: bool, bdn: bool, erased: bool,
  }
  /// model::Stacks (a batch is its regions)
  type Stacks = { undo: List[List[Region]], redo: List[List[Region]], cleared: bool }
  /// model::Edges
  type Edges = { s: int, e: int, ts: bool, te: bool }
  /// model::Record (without its original hunks)
  type Record = { author: int, regions: List[Region], edges: List[Edges] }
  type St = {
    doc: List[Elem], stacks: int -> Stacks, records: int -> Record, head: int,
    events: int -> int, undos: int, reverts: int, viol: Set[str],
  }
  /// model::Report, lines left out (always (1, 1)); kind "overlap" | "taint" | "erased"
  type Report = { start: int, end: int, kind: str }
  /// model::Outcome: kind "changed" | "conflicts" | "blocked" | "applied" | "dropped" |
  /// "nothing" (notice "plain" | "cleared")
  type Outcome = {
    kind: str, changed: bool, steps: int, step: int, skipped: List[Report],
    hasBlocked: bool, blocked: List[Report], regions: List[Report], notice: str,
  }
  /// One action (bounded::Act): kind "edit" | "purge" | "undo" | "redo" | "revert";
  /// mode "all" | "partial" | "drop"
  type Act = {
    kind: str, p: int, hunks: List[Hunk], secret: int, mode: str, steps: int,
    change: int, partial: bool,
  }
  /// model::RegionNote (what the checks read)
  type Note = {
    start: int, end: int, liveLen: int, tainted: bool, erased: bool, bdn: bool,
    clean: bool, o5: bool, samePointEnd: bool,
  }
  /// model::StepNote
  type StepNote = { applied: List[int], reports: List[Report], pushed: int }
  type Clean = (int, List[int], List[Hunk])
  type Merges = { clean: List[Clean], conflicts: List[Report], notes: List[Note] }
  type Merged = { clean: bool, text: List[int], edits: List[Hunk], o5: bool, samePointEnd: bool }
  type Commit = { st: St, changed: bool, regions: List[Region], dels: List[int] }
  /// One request's result: the state, the outcome, and what the checks read
  type Res = {
    st: St, out: Outcome, notes: List[Note], steps: List[StepNote], dels: List[int],
    supersetOk: bool,
  }

  val PIDSET: Set[int] = 1.to(PIDS)
  pure val NOSTACKS: Stacks = { undo: [], redo: [], cleared: false }
  pure val NOREPORTS: List[Report] = []
  pure val OUT0: Outcome = {
    kind: "changed", changed: false, steps: 0, step: 0, skipped: NOREPORTS,
    hasBlocked: false, blocked: NOREPORTS, regions: NOREPORTS, notice: "",
  }

  // ---------------------------------------------------------------------------------------
  // Lists

  pure def chars(doc: List[Elem]): List[int] = doc.foldl([], (acc, x) => acc.append(x.ch))
  pure def maxI(a: int, b: int): int = if (a > b) a else b
  pure def dropLast(l: List[a]): List[a] = l.slice(0, l.length() - 1)
  pure def lastOf(l: List[a]): a = l[l.length() - 1]
  pure def growth(h: Hunk): int = h.ins.length() - (h.e - h.s)
  pure def isIns(h: Hunk): bool = h.s == h.e
  pure def has(l: List[int], x: int): bool = l.indices().exists(i => l[i] == x)
  pure def asSet(l: List[a]): Set[a] = l.foldl(Set(), (s, x) => s.union(Set(x)))

  // ---------------------------------------------------------------------------------------
  // model/hunk.rs

  /// hunk::canonical: empty edits dropped, touching ones joined
  pure def canonical(es: List[Hunk]): List[Hunk] = es.foldl([], (out, h) =>
    if (h.s == h.e and h.ins.length() == 0) out
    else if (out.length() == 0) out.append(h)
    else if (lastOf(out).e == h.s)
      dropLast(out).append({ s: lastOf(out).s, e: h.e, ins: lastOf(out).ins.concat(h.ins) })
    else out.append(h))

  /// hunk::apply over letters
  pure def applyL(old: List[int], es: List[Hunk]): List[int] = {
    val r = es.foldl({ out: [], at: 0 }, (acc, h) =>
      { out: acc.out.concat(old.slice(acc.at, h.s)).concat(h.ins), at: h.e })
    r.out.concat(old.slice(r.at, old.length()))
  }

  /// hunk::apply over elements: inserted letters are `au`'s; the deleted elements' authors
  pure def applyE(old: List[Elem], es: List[Hunk], au: int): { doc: List[Elem], dels: List[int] } = {
    val r = es.foldl({ out: [], at: 0, dels: [] }, (acc, h) => {
      out: acc.out.concat(old.slice(acc.at, h.s)).concat(h.ins.foldl([], (l, c) => l.append({ ch: c, au: au }))),
      at: h.e,
      dels: acc.dels.concat(old.slice(h.s, h.e).foldl([], (l, x) => l.append(x.au))),
    })
    { doc: r.out.concat(old.slice(r.at, old.length())), dels: r.dels }
  }

  /// hunk::carry (side "start": stays before an insertion at it; "end": moves past it);
  /// R1: strictly inside a deleted or replaced range, it moves to where the range was
  pure def carry(pos: int, es: List[Hunk], side: str): int = {
    val r = es.foldl({ shift: 0, done: false, at: 0 }, (acc, e) =>
      if (acc.done) acc
      else if (isIns(e) and e.s == pos)
        { shift: if (side == "end") acc.shift + growth(e) else acc.shift, done: false, at: 0 }
      else if (e.e <= pos) { shift: acc.shift + growth(e), done: false, at: 0 }
      else if (e.s < pos) { shift: acc.shift, done: true, at: e.s + acc.shift }
      else acc)
    if (r.done) r.at else pos + r.shift
  }

  /// hunk::crosses (U9): an edit deleting across pos
  pure def crosses(pos: int, es: List[Hunk]): bool = es.indices().exists(i => es[i].s < pos and pos < es[i].e)

  // ---------------------------------------------------------------------------------------
  // model/merge.rs (§4, O1–O5)

  pure def o1(x: Hunk, y: Hunk): bool = not(isIns(x)) and not(isIns(y)) and x.s < y.e and y.s < x.e
  pure def o2(x: Hunk, y: Hunk): bool =
    (isIns(x) and not(isIns(y)) and y.s < x.s and x.s < y.e)
    or (not(isIns(x)) and isIns(y) and x.s < y.s and y.s < x.e)
  pure def samePoint(x: Hunk, y: Hunk): bool = isIns(x) and isIns(y) and x.s == y.s

  /// merge::merge3_window(base, ours, theirs, at_end): ours' changes from base placed on
  /// theirs, or a conflict (O1, O2, O5 outside the doc's end); O4 taken once
  pure def merge3w(base: List[int], ours: List[int], theirs: List[int], atEnd: bool): Merged = {
    val a = hunks(base, ours)
    val b = hunks(base, theirs)
    val n = base.length()
    val pairs = a.indices().map(i => b.indices().map(j => (i, j))).flatten().filter(p => a[p._1] != b[p._2])
    val lands = pairs.exists(p => samePoint(a[p._1], b[p._2]) and atEnd and a[p._1].s == n)
    val spe = pairs.exists(p => samePoint(a[p._1], b[p._2]) and not(atEnd and a[p._1].s == n) and a[p._1].s == n)
    val conflict = pairs.exists(p =>
      o1(a[p._1], b[p._2]) or o2(a[p._1], b[p._2])
      or (samePoint(a[p._1], b[p._2]) and not(atEnd and a[p._1].s == n)))
    val placed = a.foldl([], (acc, x) =>
      if (b.indices().exists(j => b[j] == x)) acc
      else {
        val shift = b.foldl(0, (s, y) => if (y.e <= x.s) s + growth(y) else s)
        acc.append({ s: x.s + shift, e: x.e + shift, ins: x.ins })
      })
    val edits = canonical(placed)
    if (conflict) { clean: false, text: [], edits: [], o5: false, samePointEnd: spe }
    else { clean: true, text: applyL(theirs, edits), edits: edits, o5: lands, samePointEnd: spe }
  }

  // ---------------------------------------------------------------------------------------
  // model/mod.rs: carrying (U8, U9, U22a)

  /// the shared-edge clamp (U8): a region starting before the previous one's end starts there
  pure def clamp(rs: List[Region]): List[Region] = rs.foldl([], (acc, r) =>
    if (acc.length() == 0) acc.append(r)
    else if (r.start < lastOf(acc).end)
      acc.append({ ...r, start: lastOf(acc).end, end: maxI(r.end, lastOf(acc).end) })
    else acc.append(r))

  /// carry_regions: each region carried (tainted on a crossing), then the clamp
  pure def carryRegions(rs: List[Region], es: List[Hunk]): List[Region] = clamp(rs.foldl([], (acc, r) => acc.append({
    ...r,
    tainted: r.tainted or crosses(r.start, es) or crosses(r.end, es),
    start: carry(r.start, es, "start"),
    end: carry(r.end, es, "end"),
  })))

  /// a record's edges carried each on its own, each with its own taint (U22a)
  pure def carryEdges(eds: List[Edges], es: List[Hunk]): List[Edges] = eds.foldl([], (acc, d) => acc.append({
    ts: d.ts or crosses(d.s, es), te: d.te or crosses(d.e, es),
    s: carry(d.s, es, "start"), e: carry(d.e, es, "end"),
  }))

  /// carry_all: every held region (stacks and records)
  pure def carryAll(st: St, es: List[Hunk]): St = {
    ...st,
    stacks: st.stacks.keys().mapBy(k => {
      val x = st.stacks.get(k)
      { undo: x.undo.foldl([], (l, bt) => l.append(carryRegions(bt, es))),
        redo: x.redo.foldl([], (l, bt) => l.append(carryRegions(bt, es))),
        cleared: x.cleared }
    }),
    records: st.records.keys().mapBy(k => {
      val r = st.records.get(k)
      { author: r.author, regions: carryRegions(r.regions, es), edges: carryEdges(r.edges, es) }
    }),
  }

  /// revert_regions (U22a): the record's carried edges; a region whose start is at or
  /// before the previous one's end takes that end's taint; then the clamp
  pure def revertRegions(rec: Record): List[Region] = {
    val r = range(0, rec.regions.length()).foldl({ out: [], hasPrev: false, prevEnd: 0, prevT: false }, (acc, i) => {
      val g = rec.regions[i]
      val d = rec.edges[i]
      val inherits = acc.hasPrev and d.s <= acc.prevEnd and acc.prevT
      { out: acc.out.append({ ...g, start: d.s, end: maxI(d.e, d.s), tainted: d.ts or d.te or inherits }),
        hasPrev: true, prevEnd: d.e, prevT: d.te }
    })
    clamp(r.out)
  }

  // ---------------------------------------------------------------------------------------
  // model/mod.rs: changes

  /// commit: applied, every held region carried, recorded (U22); nothing when no edit
  pure def commit(st: St, author: int, es: List[Hunk]): Commit =
    if (es.length() == 0) { st: st, changed: false, regions: [], dels: [] }
    else {
      val before = chars(st.doc)
      val after = applyL(before, es)
      val regions = es.foldl({ out: [], delta: 0 }, (acc, e) => {
        val start = e.s + acc.delta
        { out: acc.out.append({
            start: start, end: start + e.ins.length(), before: before.slice(e.s, e.e),
            after: e.ins, tainted: false, bdn: after.length() > 0, erased: false }),
          delta: acc.delta + growth(e) }
      }).out
      val carried = carryAll(st, es)
      val ap = applyE(st.doc, es, author)
      val nh = st.head + 1
      val rec = { author: author, regions: regions,
                  edges: regions.foldl([], (l, g) => l.append({ s: g.start, e: g.end, ts: false, te: false })) }
      { st: { ...carried, doc: ap.doc, head: nh, records: carried.records.put(nh, rec) },
        changed: true, regions: regions, dels: ap.dels }
    }

  /// new_batch: a participant's batch on their stack; it empties their redo stack (U7) and
  /// withdraws the purge notice (U19)
  pure def newBatch(st: St, p: int, es: List[Hunk]): Commit = {
    val c = commit(st, p, es)
    if (not(c.changed)) c
    else {
      val x = c.st.stacks.get(p)
      { ...c, st: { ...c.st, stacks: c.st.stacks.set(p, { undo: x.undo.append(c.regions), redo: [], cleared: false }) } }
    }
  }

  // ---------------------------------------------------------------------------------------
  // model/mod.rs: undo, redo, revert

  pure def report(r: Region, kind: str): Report = { start: r.start, end: r.end, kind: kind }

  /// U10's at_end, and the mutants (vi), (vii)
  def atEndFor(r: Region, n: int): bool =
    if (MUTANT == "vi-o5-every-window-end") true
    else if (MUTANT == "vii-o5-region-base") r.end == n and r.after.length() > 0
    else r.end == n and r.bdn

  /// merge_regions (U10, U11): the clean regions and the conflicting ones' reports;
  /// mutant (i) has no taint rule
  def mergeRegions(st: St, rs: List[Region]): Merges = {
    val live = chars(st.doc)
    val n = live.length()
    range(0, rs.length()).foldl({ clean: [], conflicts: NOREPORTS, notes: [] }, (acc, i) => {
      val r = rs[i]
      val nt = { start: r.start, end: r.end, liveLen: n, tainted: r.tainted, erased: r.erased,
                 bdn: r.bdn, clean: false, o5: false, samePointEnd: false }
      if (r.erased)
        { clean: acc.clean, conflicts: acc.conflicts.append(report(r, "erased")), notes: acc.notes.append(nt) }
      else if (r.tainted and MUTANT != "i-no-taint")
        { clean: acc.clean, conflicts: acc.conflicts.append(report(r, "taint")), notes: acc.notes.append(nt) }
      else {
        val m = merge3w(r.after, r.before, live.slice(r.start, r.end), atEndFor(r, n))
        val nt2 = { ...nt, clean: m.clean, o5: m.o5, samePointEnd: m.samePointEnd }
        if (m.clean)
          { clean: acc.clean.append((i, m.text, m.edits)), conflicts: acc.conflicts, notes: acc.notes.append(nt2) }
        else
          { clean: acc.clean, conflicts: acc.conflicts.append(report(r, "overlap")), notes: acc.notes.append(nt2) }
      }
    })
  }

  /// step_edits: each clean region's merged edits offset by its start, canonical (R2, R3)
  pure def stepEdits(rs: List[Region], clean: List[Clean]): List[Hunk] = canonical(clean.foldl([], (acc, c) => {
    val r = rs[c._1]
    acc.concat(c._3.foldl([], (l, e) => l.append({ s: e.s + r.start, e: e.e + r.start, ins: e.ins })))
  }))

  /// apply_step: one undo or redo step's clean regions as one change by p; the regions for
  /// the opposite stack (R4), in the coordinates after the step
  pure def applyStep(st: St, p: int, rs: List[Region], clean: List[Clean]): { st: St, inverse: List[Region], dels: List[int], edits: List[Hunk] } = {
    val pre = chars(st.doc)
    val es = stepEdits(rs, clean)
    val inv = clean.foldl({ out: [], shift: 0 }, (acc, c) => {
      val r = rs[c._1]
      val start = r.start + acc.shift
      { out: acc.out.append({ start: start, end: start + c._2.length(), before: pre.slice(r.start, r.end),
                              after: c._2, tainted: false, bdn: true, erased: false }),
        shift: acc.shift + c._2.length() - (r.end - r.start) }
    }).out
    val cm = commit(st, p, es)
    val nonEmpty = cm.st.doc.length() > 0
    { st: cm.st, inverse: inv.foldl([], (l, g) => l.append({ ...g, bdn: nonEmpty })), dels: cm.dels, edits: es }
  }

  /// the reports `mode` shows for a step (mutant (iii): --partial leaves taints out)
  def shown(conflicts: List[Report], mode: str): List[Report] =
    if (mode == "partial" and MUTANT == "iii-partial-hides-taint")
      conflicts.foldl([], (l, c) => if (c.kind != "taint") l.append(c) else l)
    else conflicts

  /// undo_redo (U13, U14, U17; R7): plan the top `steps` batches on a scratch copy
  def undoRedo(st: St, p: int, mode: str, steps: int, redo: bool): Res = {
    val x = st.stacks.get(p)
    val src = if (redo) x.redo else x.undo
    val want = if (mode == "drop") 1 else maxI(steps, 1)
    val k = if (src.length() < want) src.length() else want
    val res0 = { st: st, out: OUT0, notes: [], steps: [], dels: [], supersetOk: true }
    if (k == 0)
      // nothing (U18; U19's notice, said once)
      { ...res0, st: { ...st, stacks: st.stacks.set(p, { ...x, cleared: false }) },
        out: { ...OUT0, kind: "nothing", notice: if (x.cleared) "cleared" else "plain" } }
    else if (mode == "drop")
      // U14's escape: the top step goes, unapplied
      { ...res0, st: { ...st, stacks: st.stacks.set(p, if (redo) { ...x, redo: dropLast(x.redo) } else { ...x, undo: dropLast(x.undo) }) },
        out: { ...OUT0, kind: "dropped" } }
    else {
      val a = range(0, k).foldl({
          work: st, done: 0, skipped: NOREPORTS, blocked: NOREPORTS, hasBlocked: false, stop: false,
          cstep: 0, cregions: NOREPORTS, steps: [], notes: [], dels: [] }, (acc, i) =>
        if (acc.stop) acc
        else {
          val xs = acc.work.stacks.get(p)
          val batch = if (redo) lastOf(xs.redo) else lastOf(xs.undo)
          val m = mergeRegions(acc.work, batch)
          val applied = m.clean.foldl([], (l, c) => l.append(c._1))
          val rep = shown(m.conflicts, mode)
          val notes = acc.notes.concat(m.notes)
          if (m.conflicts.length() > 0 and mode == "all")
            { ...acc, stop: true, cstep: i + 1, cregions: m.conflicts, notes: notes,
              steps: acc.steps.append({ applied: [], reports: rep, pushed: 0 }) }
          else if (m.conflicts.length() > 0 and m.clean.length() == 0)
            // --partial: every region conflicts, the step stays on top (U14)
            { ...acc, stop: true, hasBlocked: true, blocked: rep, notes: notes,
              steps: acc.steps.append({ applied: [], reports: rep, pushed: 0 }) }
          else {
            val ap = applyStep(acc.work, p, batch, m.clean)
            // mutant (iv): the redo entry also carries the skipped regions
            val extra = if (not(redo) and MUTANT == "iv-redo-retries-skipped")
              range(0, batch.length()).foldl([], (l, j) =>
                if (has(applied, j)) l
                else {
                  val r = batch[j]
                  val s = carry(r.start, ap.edits, "start")
                  l.append({ ...r, start: s, end: maxI(carry(r.end, ap.edits, "end"), s), before: r.after, after: r.before })
                })
              else []
            val inverse = ap.inverse.concat(extra)
            val ws = ap.st.stacks.get(p)
            val ws2 = if (redo) { undo: ws.undo.append(inverse), redo: dropLast(ws.redo), cleared: false }
                      else { undo: dropLast(ws.undo), redo: ws.redo.append(inverse), cleared: false }
            { ...acc, work: { ...ap.st, stacks: ap.st.stacks.set(p, ws2) }, done: acc.done + 1,
              skipped: acc.skipped.concat(rep), notes: notes, dels: acc.dels.concat(ap.dels),
              steps: acc.steps.append({ applied: applied, reports: rep, pushed: inverse.length() }) }
          }
        })
      // a refused or blocked request applies nothing: the scratch copy's deletions are not
      // the request's (own_only reads only what is applied)
      val base = { ...res0, notes: a.notes, steps: a.steps }
      if (a.cstep > 0)
        { ...base, out: { ...OUT0, kind: "conflicts", step: a.cstep, regions: a.cregions } }
      else if (a.done == 0)
        { ...base, out: { ...OUT0, kind: "blocked", regions: a.blocked } }
      else
        { ...base, st: a.work, dels: a.dels,
          out: { ...OUT0, kind: "applied", steps: a.done, skipped: a.skipped, hasBlocked: a.hasBlocked, blocked: a.blocked } }
    }
  }

  /// the regions that conflict (by index): those not clean
  pure def conflicting(n: int, clean: List[Clean]): Set[int] =
    0.to(n - 1).filter(i => not(clean.indices().exists(q => clean[q]._1 == i)))

  /// revert (U22, R8): the record's regions merged as one undo step; a new batch of p; and
  /// U22a's property against undo of the same change on the same state
  def revert(st: St, p: int, change: int, partial: bool): Res = {
    val rec = st.records.get(change)
    val rs = revertRegions(rec)
    val m = mergeRegions(st, rs)
    val u = mergeRegions(st, rec.regions)
    val sameBounds = rs.length() == rec.regions.length()
      and rs.indices().forall(i => rs[i].start == rec.regions[i].start and rs[i].end == rec.regions[i].end)
    val sameText = m.clean.indices().forall(q =>
      u.clean.indices().forall(w => u.clean[w]._1 != m.clean[q]._1 or u.clean[w]._2 == m.clean[q]._2))
    val ok = sameBounds and sameText
      and conflicting(rec.regions.length(), u.clean).subseteq(conflicting(rs.length(), m.clean))
    val res0 = { st: st, out: OUT0, notes: m.notes, steps: [], dels: [], supersetOk: ok }
    if (m.conflicts.length() > 0 and not(partial))
      { ...res0, out: { ...OUT0, kind: "conflicts", step: 1, regions: m.conflicts } }
    else if (m.conflicts.length() > 0 and m.clean.length() == 0)
      { ...res0, out: { ...OUT0, kind: "blocked", regions: m.conflicts } }
    else {
      val c = newBatch(st, p, stepEdits(rs, m.clean))
      { ...res0, st: c.st, dels: c.dels, out: { ...OUT0, kind: "applied", steps: 1, skipped: m.conflicts } }
    }
  }

  /// purge_text (U19, R9) of a one-letter secret: every live copy deleted as one batch by p
  /// (a hunk per copy), then every stack cleared (mutant skip-u19: not)
  def purge(st: St, p: int, secret: int): Res = {
    val t = chars(st.doc)
    val es = range(0, t.length()).foldl([], (l, i) => if (t[i] == secret) l.append({ s: i, e: i + 1, ins: [] }) else l)
    val c = newBatch(st, p, es)
    val cleared = if (MUTANT == "skip-u19") c.st
      else { ...c.st, stacks: c.st.stacks.keys().mapBy(k => {
        val x = c.st.stacks.get(k)
        { undo: [], redo: [], cleared: x.cleared or x.undo.length() > 0 or x.redo.length() > 0 }
      }) }
    { st: cleared, out: { ...OUT0, changed: c.changed }, notes: [], steps: [], dels: c.dels, supersetOk: true }
  }

  // ---------------------------------------------------------------------------------------
  // The bounded world (bounded.rs)

  pure val ACT0: Act = { kind: "", p: 0, hunks: [], secret: 0, mode: "", steps: 0, change: 0, partial: false }

  pure def hk(s: int, e: int, ins: List[int]): Hunk = { s: s, e: e, ins: ins }

  /// a single-letter delete or replacement at i (bounded::edits' two-region pairs)
  pure def opsAt(t: List[int], i: int): Set[Hunk] = Set(hk(i, i + 1, []), hk(i, i + 1, [1 - t[i]]))

  /// bounded::edits
  def editsOf(t: List[int]): Set[List[Hunk]] = {
    val n = t.length()
    val ins = if (n < MAXLEN) 0.to(n).map(i => Set([hk(i, i, [0])], [hk(i, i, [1])])).flatten() else Set()
    val one = if (n > 0) 0.to(n - 1).map(i => Set([hk(i, i + 1, [])], [hk(i, i + 1, [1 - t[i]])])).flatten() else Set()
    val two = if (n > 1) 0.to(n - 2).map(i => [hk(i, i + 2, [])]) else Set()
    val multi = if (MULTI and n > 2)
      0.to(n - 3).map(i => (i + 2).to(n - 1).map(j =>
        opsAt(t, i).map(x => opsAt(t, j).map(y => [x, y])).flatten()).flatten()).flatten()
      else Set()
    ins.union(one).union(two).union(multi)
  }

  /// bounded::State::actions
  def acts(st: St): Set[Act] = {
    val es = editsOf(chars(st.doc))
    PIDSET.map(p => {
      val ev = if (st.events.get(p) < EVENTS)
          es.map(h => { ...ACT0, kind: "edit", p: p, hunks: h })
            .union(if (PURGE) Set(0, 1).map(c => { ...ACT0, kind: "purge", p: p, secret: c }) else Set())
        else Set()
      val un = if (st.undos < UNDOS)
          Set(("all", 1), ("partial", 1), ("all", 2), ("partial", 2), ("drop", 1))
            .map(ms => { ...ACT0, kind: "undo", p: p, mode: ms._1, steps: ms._2 })
            .union(Set("all", "partial").map(m => { ...ACT0, kind: "redo", p: p, mode: m, steps: 1 }))
        else Set()
      val rv = if (st.reverts < REVERTS)
          st.records.keys().map(c => Set(false, true).map(b => { ...ACT0, kind: "revert", p: p, change: c, partial: b })).flatten()
        else Set()
      ev.union(un).union(rv)
    }).flatten()
  }

  /// U15 on one request planned both ways: the first step all or nothing refuses reports, as a
  /// set, what --partial reports for that step; with no refusal, --partial reports nothing
  pure def u15(allN: List[StepNote], part: List[StepNote]): bool = {
    val ks = allN.indices().filter(i => allN[i].reports.length() > 0)
    if (ks.size() == 0) part.indices().forall(i => part[i].reports.length() == 0)
    else {
      val k = ks.fold(allN.length(), (m, i) => if (i < m) i else m)
      k < part.length() and asSet(part[k].reports) == asSet(allN[k].reports)
    }
  }

  /// bounded::State::check: the rules this transition violates
  def checks(pre: St, a: Act, r: Res): Set[str] = {
    val owner = if (a.kind == "undo" or a.kind == "redo") a.p
      else if (a.kind == "revert") pre.records.get(a.change).author
      else -1
    val ownOnly = owner >= 0 and r.dels.indices().exists(i => r.dels[i] != owner)
    val u9 = r.notes.indices().exists(i => r.notes[i].tainted and not(r.notes[i].erased) and r.notes[i].clean)
    val o5 = r.notes.indices().exists(i => {
      val n = r.notes[i]
      val atDocEnd = n.end == n.liveLen and n.bdn
      // O5 landed off the doc's end, or was refused at it
      if (atDocEnd) n.samePointEnd else n.o5
    })
    val u13 = (r.out.kind == "conflicts" or r.out.kind == "blocked")
      and (r.st.doc != pre.doc or r.st.stacks != pre.stacks or r.st.records != pre.records)
    val u17 = r.steps.indices().exists(i => r.steps[i].applied.length() > 0 and r.steps[i].pushed != r.steps[i].applied.length())
    val u15v = if ((a.kind == "undo" or a.kind == "redo") and a.mode != "drop") {
        val other = undoRedo(pre, a.p, if (a.mode == "all") "partial" else "all", a.steps, a.kind == "redo").steps
        if (a.mode == "all") not(u15(r.steps, other)) else not(u15(other, r.steps))
      } else false
    val u19 = a.kind == "purge" and r.st.stacks.keys().exists(k =>
      r.st.stacks.get(k).undo.length() > 0 or r.st.stacks.get(k).redo.length() > 0)
    val u22a = a.kind == "revert" and not(r.supersetOk)
    Set(("own_only", ownOnly), ("U9", u9), ("O5", o5), ("U13", u13), ("U15", u15v), ("U17", u17),
        ("U19", u19), ("U22a", u22a)).filter(x => x._2).map(x => x._1)
  }

  /// one action: the oracle's result, budgets spent
  def perform(st: St, a: Act): Res =
    if (a.kind == "edit") {
      val c = newBatch(st, a.p, canonical(a.hunks))
      { st: { ...c.st, events: c.st.events.set(a.p, c.st.events.get(a.p) + 1) },
        out: { ...OUT0, changed: c.changed }, notes: [], steps: [], dels: c.dels, supersetOk: true }
    } else if (a.kind == "purge") {
      val r = purge(st, a.p, a.secret)
      { ...r, st: { ...r.st, events: r.st.events.set(a.p, r.st.events.get(a.p) + 1) } }
    } else if (a.kind == "undo" or a.kind == "redo") {
      val r = undoRedo(st, a.p, a.mode, a.steps, a.kind == "redo")
      { ...r, st: { ...r.st, undos: r.st.undos + 1 } }
    } else {
      val r = revert(st, a.p, a.change, a.partial)
      { ...r, st: { ...r.st, reverts: r.st.reverts + 1 } }
    }

  /// bounded::State::step: the next state (checks recorded, records dropped once spent),
  /// the outcome, and whether the cut allows it
  def nextOf(st: St, a: Act): { ok: bool, st: St, out: Outcome } = {
    val r = perform(st, a)
    val v = { ...r.st, viol: st.viol.union(checks(st, a, r)) }
    val red = if (REDUCE and v.reverts == REVERTS) { ...v, records: Map(), head: 0 } else v
    { ok: r.st.doc.length() <= MAXLEN, st: red, out: r.out }
  }

  val INIT_ST: St = {
    doc: INIT.foldl([], (l, c) => l.append({ ch: c, au: 0 })),
    stacks: PIDSET.mapBy(p => NOSTACKS), records: Map(), head: 1,
    events: PIDSET.mapBy(p => 0), undos: 0, reverts: 0, viol: Set(),
  }

  // ---------------------------------------------------------------------------------------
  // State machine

  var st: St

  action init = st' = INIT_ST

  action step = {
    nondet a = acts(st).oneOf()
    val r = nextOf(st, a)
    all { r.ok, st' = r.st }
  }

  // ---------------------------------------------------------------------------------------
  // Invariants

  val inv_own_only = not(st.viol.contains("own_only"))
  val inv_U9 = not(st.viol.contains("U9"))
  val inv_U13 = not(st.viol.contains("U13"))
  val inv_U15 = not(st.viol.contains("U15"))
  val inv_U17 = not(st.viol.contains("U17"))
  val inv_U19 = not(st.viol.contains("U19"))
  val inv_U22a = not(st.viol.contains("U22a"))
  val inv_O5 = not(st.viol.contains("O5"))
  val inv = inv_U9 and inv_U13 and inv_U15 and inv_U17 and inv_U19 and inv_U22a and inv_O5
}
