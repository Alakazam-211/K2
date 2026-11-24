// Jump to one Thread message and highlight it (thread-reply-to-v1, 0.46.18):
// a reply's `↩ #N` line and a `#N` reference in a message (common/thread-refs)
// both land here.
//
// Matched to K2 Companion's jump (src/zen/threadFocus.ts and
// src/zen/chatBubbles.css `.zen-outlined`, Companion sha 69923c8): the
// target scrolls to the MIDDLE of the list; a target older than the oldest
// loaded row loads older history first (at most FOCUS_MAX_PAGES rounds); a
// target that isn't there (gone, newer than the tail) gives up quietly; the
// row wears `data-highlight="true"` for FOCUS_HIGHLIGHT_MS. A bubble's
// highlight is a ring that follows its real shape (tail included): chained
// drop-shadows, 1.5px of the canvas colour, then 1.5px of the accent, on
// all four sides. A row with no bubble (files only) gets a rounded 2px
// accent ring instead.

import { createContext, useCallback, useContext, useEffect, useLayoutEffect, useRef, useState, type CSSProperties } from 'react'

/** At most this many history loads while looking for the target. */
export const FOCUS_MAX_PAGES = 4
/** How long the landed message stays highlighted. */
export const FOCUS_HIGHLIGHT_MS = 2000

/** The row's attribute while highlighted (the phone's tests read it too). */
export const THREAD_HIGHLIGHT_ATTR = 'data-highlight'

/** The ring on a highlighted bubble (Companion `.zen-outlined`). */
const GAP = 'var(--zen-canvas, var(--color-bg))'
const ACCENT = 'var(--color-accent)'
const ring = (c: string): string =>
  `drop-shadow(1.5px 0 0 ${c}) drop-shadow(-1.5px 0 0 ${c}) drop-shadow(0 1.5px 0 ${c}) drop-shadow(0 -1.5px 0 ${c})`
export const THREAD_FOCUS_BUBBLE_STYLE: CSSProperties = { filter: `${ring(GAP)} ${ring(ACCENT)}` }
/** A highlighted row with no bubble (a post that is only files). */
export const THREAD_FOCUS_ROW_STYLE: CSSProperties = {
  borderRadius: 20,
  boxShadow: `0 0 0 2px ${ACCENT}`,
  transition: 'box-shadow .4s',
}

/** Jump to message `seq` of the Thread drawn here (the desktop Thread
 *  provides it; elsewhere a reply line is plain text). */
export const ThreadJumpContext = createContext<((seq: number) => void) | null>(null)

export function useThreadJump(): ((seq: number) => void) | null {
  return useContext(ThreadJumpContext)
}

export interface ThreadFocusRow {
  id: string
  seq: number
}

/** Find `seq` in the Thread drawn in `list()`: draw its row (the render
 *  window), scroll it into the middle and highlight it briefly. Older than
 *  every loaded row: `loadBack(seq)` (the pane reads back through it), at
 *  most FOCUS_MAX_PAGES times. Gone, or newer than the newest: give up
 *  quietly. Returns `focus` and the highlighted seq. */
export function useThreadFocusSeq(o: {
  list: () => HTMLElement | null
  /** Every message in memory, oldest first. */
  items: readonly ThreadFocusRow[]
  hasMore: boolean
  loadingOlder: boolean
  /** Read older history back through `seq` (or one more page). */
  loadBack: (seq: number) => Promise<void> | void
  /** The render window: draw this row; true when it is (or will be) drawn. */
  show: (key: string) => boolean
  /** The drawn range: changes when the window moves. */
  drawn: string
  /** Stop following the newest (the reader is going back). */
  unfollow: () => void
  onScrollWrite: (top: number) => void
}): { focus: (seq: number) => void; highlight: number | null } {
  const [focusSeq, setFocusSeq] = useState<number | null>(null)
  const [highlight, setHighlight] = useState<number | null>(null)
  // Bumped when a load settles, so a load that changed nothing still ends the search.
  const [tick, setTick] = useState(0)
  const pages = useRef(0)
  const latest = useRef(o)
  latest.current = o

  const focus = useCallback((seq: number) => {
    if (!Number.isSafeInteger(seq) || seq <= 0) return
    pages.current = 0
    setHighlight(null)
    setFocusSeq(seq)
  }, [])

  useLayoutEffect(() => {
    if (focusSeq == null || o.loadingOlder) return
    const cur = latest.current
    const done = (): void => setFocusSeq(null)
    const list = cur.list()
    const el = list?.querySelector<HTMLElement>(`[data-seq="${focusSeq}"]`)
    if (el) {
      cur.unfollow()
      el.scrollIntoView?.({ block: 'center' })
      if (list) cur.onScrollWrite(list.scrollTop)
      setHighlight(focusSeq)
      done()
      return
    }
    const row = cur.items.find((it) => it.seq === focusSeq)
    if (row) {
      // In memory, outside the drawn window: draw it; the next pass scrolls.
      cur.unfollow()
      if (!cur.show(row.id)) done()
      return
    }
    let oldest = Number.POSITIVE_INFINITY
    for (const it of cur.items) if (it.seq > 0 && it.seq < oldest) oldest = it.seq
    if (Number.isFinite(oldest) && focusSeq < oldest && cur.hasMore && pages.current < FOCUS_MAX_PAGES) {
      pages.current += 1
      cur.unfollow()
      void Promise.resolve(cur.loadBack(focusSeq))
        .catch((err: unknown) => console.warn('[thread] loading back to a message failed:', err))
        .finally(() => setTick((t) => t + 1))
      return
    }
    done()
  }, [focusSeq, o.items, o.hasMore, o.loadingOlder, o.drawn, tick])

  useEffect(() => {
    if (highlight == null) return
    const t = setTimeout(() => setHighlight(null), FOCUS_HIGHLIGHT_MS)
    return () => clearTimeout(t)
  }, [highlight])

  return { focus, highlight }
}
