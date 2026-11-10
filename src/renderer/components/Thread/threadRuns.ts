// Runs of messages (K2 Companion's `chatRuns` in src/zen/chatReceipt.ts,
// release 4.2.2): the bubble tail (Glass) and where a message's details go.
//
// A run is consecutive messages from one side no more than `RUN_GAP_S`
// apart: all of the viewer's own, or one other sender's (by the sender key
// the Thread's sender column uses, threadSenders). A row with no time stays
// in the run (as the phone). As on the phone:
//   - the last bubble of a run draws the tail; under your last, the status
//     line (the time, Sending…, Not sent, or the receipt);
//   - the first bubble of another sender's run has their name and time
//     over it (and their picture beside it, when the sender column shows);
//   - the bubbles of one run sit closer together than two runs do.

/** Messages this far apart (seconds) start a new run. */
export const RUN_GAP_S = 120

export interface ThreadRunInput {
  mine: boolean
  /** Who sent it (`threadSender(...).key`); ignored on the viewer's own. */
  from: string
  /** Unix seconds or ms; null when unknown. */
  at: number | null | undefined
  /** Who the viewer's own message went to, where a chat has many
   *  recipients (Chatter): a new recipient starts a new run of yours, so
   *  each run's status line names one. Absent (the Thread): ignored. */
  to?: string | null
}

/** Seconds, from Unix seconds or ms; null when unknown. */
function secondsOf(at: number | null | undefined): number | null {
  if (at == null || !Number.isFinite(at)) return null
  return at > 1e12 ? at / 1000 : at
}

/** `b` right after `a` continues `a`'s run. */
export function sameThreadRun(a: ThreadRunInput, b: ThreadRunInput, gapS = RUN_GAP_S): boolean {
  if (a.mine !== b.mine) return false
  if (!a.mine && a.from !== b.from) return false
  if (a.mine && (a.to ?? null) !== (b.to ?? null)) return false
  const sa = secondsOf(a.at)
  const sb = secondsOf(b.at)
  if (sa == null || sb == null) return true
  return Math.abs(sb - sa) <= gapS
}

/** For each row, whether it ends its run (draws the tail). `next` is the
 *  row after the last one (outside a render window), so the window's last
 *  row isn't taken for a run's end when the run goes on past it. */
export function threadRunEnds(rows: readonly ThreadRunInput[], next?: ThreadRunInput | null, gapS = RUN_GAP_S): boolean[] {
  return rows.map((r, i) => {
    const after = i + 1 < rows.length ? rows[i + 1] : next
    return !after || !sameThreadRun(r, after, gapS)
  })
}

/** For each row, whether it starts its run (the name line, the picture).
 *  `prev` is the row before the first one (outside a render window), so a
 *  run coming in from above the window isn't taken for a new one. */
export function threadRunStarts(rows: readonly ThreadRunInput[], prev?: ThreadRunInput | null, gapS = RUN_GAP_S): boolean[] {
  return rows.map((r, i) => {
    const before = i > 0 ? rows[i - 1] : prev
    return !before || !sameThreadRun(before, r, gapS)
  })
}

/** Starts and ends for rows `lo`..`hi` of `all` (a render window): the rows
 *  just outside it count, as above. */
export function threadRunWindow(
  all: readonly ThreadRunInput[],
  lo: number,
  hi: number,
  gapS = RUN_GAP_S,
): { starts: boolean[]; ends: boolean[] } {
  const rows = all.slice(lo, hi)
  return {
    starts: threadRunStarts(rows, lo > 0 ? all[lo - 1] : null, gapS),
    ends: threadRunEnds(rows, hi < all.length ? all[hi] : null, gapS),
  }
}
