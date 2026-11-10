// Runs of messages, for the bubble tail (Glass, from K2 Companion's
// `chatRuns` in src/zen/chatReceipt.ts, release 4.2.2).
//
// A run is consecutive messages from one side no more than `RUN_GAP_S`
// apart: all of the viewer's own, or one other sender's (by the sender key
// the Thread's sender column uses, threadSenders). Only the last bubble of
// a run draws a tail. A row with no time stays in the run (as the phone).

/** Messages this far apart (seconds) start a new run. */
export const RUN_GAP_S = 120

export interface ThreadRunInput {
  mine: boolean
  /** Who sent it (`threadSender(...).key`); ignored on the viewer's own. */
  from: string
  /** Unix seconds or ms; null when unknown. */
  at: number | null | undefined
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
