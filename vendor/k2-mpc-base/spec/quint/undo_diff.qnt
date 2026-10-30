// -*- mode: Bluespec; -*-
//
// The undo model's diff: the kit's diff::hunks (spec D1–D2), pinned for the texts the model
// diffs. Texts are over {a, b} (a = 0, b = 1), with no whitespace or newline, so diff::hunks
// takes one path: trim the common prefix, then the common suffix; if what is left of either
// side is empty, one hunk; if one side is a subsequence of the other, its character-level
// Myers diff is one-sided (only deletions, or only insertions), else the two middles are one
// replacement hunk (a single token on both sides). For a one-sided diff, Myers' choice of
// which characters to keep is, on every pair of texts up to DIFF_MAX = 4 long, the
// right-greedy embedding below (keep the latest matching characters).
//
// crates/spec-undo/tests/quint_diff.rs transcribes this module and checks it against
// diff::hunks on EVERY pair of texts over {a, b} up to 4 long (961 pairs, all equal), and
// that it differs at 5 (so 4 is the limit). The model's cut keeps every text it diffs
// within MAXLEN (bounded::tests::the_cut_bounds_every_diffed_string), so MAXLEN must be at
// most DIFF_MAX: then on every state the bound reaches, this is diff::hunks.

module undo_diff {
  type Hunk = { s: int, e: int, ins: List[int] }

  pure val DIFF_MAX: int = 4

  /// the length of the common prefix of a and b
  pure def prefixLen(a: List[int], b: List[int]): int = {
    val m = if (a.length() < b.length()) a.length() else b.length()
    range(0, m).foldl({ k: 0, go: true }, (acc, i) =>
      if (acc.go and a[i] == b[i]) { k: acc.k + 1, go: true } else { k: acc.k, go: false }).k
  }

  /// the length of the common suffix of a and b
  pure def suffixLen(a: List[int], b: List[int]): int = {
    val m = if (a.length() < b.length()) a.length() else b.length()
    range(0, m).foldl({ k: 0, go: true }, (acc, i) =>
      if (acc.go and a[a.length() - 1 - i] == b[b.length() - 1 - i]) { k: acc.k + 1, go: true }
      else { k: acc.k, go: false }).k
  }

  /// which characters of `long` the right-greedy embedding of `short` keeps, and how many
  /// of `short`'s characters it could not place (0: `short` is a subsequence of `long`)
  pure def embed(long: List[int], short: List[int]): { keep: List[bool], left: int } = {
    val r = range(0, long.length()).foldl({ j: short.length(), keep: [] }, (acc, k) => {
      val i = long.length() - 1 - k
      if (acc.j > 0 and long[i] == short[acc.j - 1]) { j: acc.j - 1, keep: [true].concat(acc.keep) }
      else { j: acc.j, keep: [false].concat(acc.keep) }
    })
    { keep: r.keep, left: r.j }
  }

  /// diff::hunks(old, new), pinned (see the module comment)
  pure def hunks(old: List[int], new: List[int]): List[Hunk] =
    if (old == new) [] else {
      val p = prefixLen(old, new)
      val oTail = old.slice(p, old.length())
      val nTail = new.slice(p, new.length())
      val s = suffixLen(oTail, nTail)
      val om = oTail.slice(0, oTail.length() - s)
      val nm = nTail.slice(0, nTail.length() - s)
      val whole = [{ s: p, e: p + om.length(), ins: nm }]
      if (om.length() == 0 or nm.length() == 0) whole
      else {
        val del = om.length() > nm.length()
        val long = if (del) om else nm
        val short = if (del) nm else om
        val em = embed(long, short)
        if (em.left > 0) whole
        else {
          // each run of characters not kept is one hunk: a deletion of old's run, or an
          // insertion of new's run at the old position after the characters kept before it
          range(0, long.length() + 1).foldl({ out: [], si: 0, inRun: false, st: 0 }, (acc, i) => {
            val gap = i < long.length() and not(em.keep[i])
            val close = acc.inRun and not(gap)
            val out = if (not(close)) acc.out
              else if (del) acc.out.append({ s: p + acc.st, e: p + i, ins: [] })
              else acc.out.append({ s: p + acc.si, e: p + acc.si, ins: long.slice(acc.st, i) })
            { out: out,
              si: if (i < long.length() and not(gap)) acc.si + 1 else acc.si,
              inRun: gap,
              st: if (gap and not(acc.inRun)) i else acc.st }
          }).out
        }
      }
    }
}
