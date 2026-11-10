// The Thread's side of Thread view's search (threadSearch.ts): it finds the
// matches in the messages it holds, reads the rest of the history once a
// search starts, draws and scrolls to the current match, and paints the
// highlights over the drawn rows. The search object in the floating header
// owns the field and the count.

import { useCallback, useEffect, useMemo, useRef, useState, type RefObject } from 'react'
import { parseThreadReply } from '@/lib/thread-reply'
import { threadBubbleText } from '@/components/Thread/ThreadZenKit'
import type { OverlayDoc, OverlayThreadItem } from './overlayThread'
import {
  THREAD_SEARCH_CLOSED,
  clearThreadSearchPaint,
  currentMatchIndex,
  findThreadMatches,
  markdownSearchText,
  messageBodies,
  paintThreadSearch,
  searchRanges,
  searchableQuery,
  threadSearchHost,
  updateThreadSearch,
  useThreadSearchState,
} from './threadSearch'

const textByDoc = new WeakMap<OverlayDoc, string>()

/** What a message's search reads: the words its bubble draws (a reply's
 *  own words, not its quote of the other message). */
export function threadItemSearchText(item: OverlayThreadItem): string {
  const hit = textByDoc.get(item.doc)
  if (hit !== undefined) return hit
  const text = threadBubbleText(item)
  const reply = item.doc.kind === 'text' ? parseThreadReply(text) : null
  const out = markdownSearchText(reply ? reply.rest : text)
  textByDoc.set(item.doc, out)
  return out
}

/** Put the match a third of the way down the list's visible part (under the
 *  floating objects, above the message box), unless it is already in view. */
function scrollMatchIntoView(list: HTMLElement, target: Range | HTMLElement, onScrollWrite: (top: number) => void): void {
  const rect = typeof target.getBoundingClientRect === 'function' ? target.getBoundingClientRect() : null
  if (!rect) {
    if (target instanceof HTMLElement) target.scrollIntoView?.({ block: 'center' })
    onScrollWrite(list.scrollTop)
    return
  }
  const box = list.getBoundingClientRect()
  const style = getComputedStyle(list)
  const top = box.top + (parseFloat(style.scrollPaddingTop) || 0)
  const bottom = box.bottom - Math.max(0, (parseFloat(style.paddingBottom) || 0) - 8)
  if (rect.top >= top && rect.bottom <= bottom) return
  list.scrollTop += rect.top - (top + Math.max(0, bottom - top) / 3)
  onScrollWrite(list.scrollTop)
}

export function useThreadSearchPane(o: {
  /** The scrolling list, inside the pane's floats host. */
  list: RefObject<HTMLElement | null>
  /** The rows' parent. */
  content: RefObject<HTMLElement | null>
  /** Every message in memory, oldest first (as drawn). */
  items: readonly OverlayThreadItem[]
  hasMore: boolean
  loadAll?: () => Promise<void>
  /** The render window: draw a row; hold rows loaded above. */
  show: (key: string) => boolean
  holdOldest: (on: boolean) => void
  /** Stop following the newest (the reader is going to a match). */
  unfollow: () => void
  onScrollWrite: (top: number) => void
}): void {
  const latest = useRef(o)
  latest.current = o
  const resolveHost = useCallback(() => threadSearchHost(latest.current.list.current), [])
  const state = useThreadSearchState(resolveHost)
  const active = state.open && searchableQuery(state.query)
  const query = state.query

  const rows = useMemo(
    () => (active ? o.items.map((it) => ({ id: it.id, text: threadItemSearchText(it) })) : []),
    [active, o.items],
  )
  const matches = useMemo(
    () => (active ? findThreadMatches(rows, query) : THREAD_SEARCH_CLOSED.matches),
    [active, rows, query],
  )

  // The rest of the history, once per search.
  const [searching, setSearching] = useState(false)
  const startedRef = useRef(false)
  useEffect(() => {
    if (!state.open) {
      startedRef.current = false
      return
    }
    if (!active || !o.hasMore || startedRef.current || !o.loadAll) return
    startedRef.current = true
    setSearching(true)
    o.holdOldest(true)
    void o.loadAll().finally(() => {
      latest.current.holdOldest(false)
      setSearching(false)
    })
  }, [state.open, active, o.hasMore, o.loadAll, o.holdOldest])

  useEffect(() => {
    if (!state.open) return
    updateThreadSearch(resolveHost(), { matches, searching: searching && o.hasMore })
  }, [state.open, matches, searching, o.hasMore, resolveHost])

  const index = currentMatchIndex(matches, state.current)
  const target = index >= 0 ? matches[index] : null
  const targetKey = target?.key ?? null
  const targetRef = useRef(target)
  targetRef.current = target
  const matchIds = useMemo(() => new Set(matches.map((m) => m.id)), [matches])

  // Paint the drawn rows; scroll to the current match once it is drawn.
  const ownerRef = useRef({})
  const revealRef = useRef<string | null>(null)
  const paint = useCallback(() => {
    const owner = ownerRef.current
    const content = latest.current.content.current
    const list = latest.current.list.current
    if (!content || !list || !active) {
      clearThreadSearchPaint(owner)
      return
    }
    const t = targetRef.current
    const all: Range[] = []
    let current: Range | null = null
    let currentRow: HTMLElement | null = null
    for (const row of Array.from(content.querySelectorAll<HTMLElement>(':scope > [data-zen-message]'))) {
      const id = row.getAttribute('data-zen-message') ?? ''
      if (!matchIds.has(id)) continue
      const ranges = searchRanges(messageBodies(row), query)
      for (const r of ranges) all.push(r)
      if (t && id === t.id) {
        currentRow = row
        current = ranges[Math.min(t.n, ranges.length - 1)] ?? null
      }
    }
    paintThreadSearch(owner, all, current)
    if (t && revealRef.current === t.key && currentRow) {
      revealRef.current = null
      scrollMatchIntoView(list, current ?? currentRow, latest.current.onScrollWrite)
    }
  }, [active, matchIds, query])

  const rafRef = useRef(0)
  const schedulePaint = useCallback(() => {
    if (rafRef.current) return
    rafRef.current = requestAnimationFrame(() => {
      rafRef.current = 0
      paintRef.current()
    })
  }, [])
  const paintRef = useRef(paint)
  paintRef.current = paint

  // A new current match: draw it, then paint and scroll on the commit.
  // The first one shown is pinned in the shared state, so a new message
  // that matches doesn't move the reader.
  useEffect(() => {
    const t = targetRef.current
    if (!t) return
    if (state.current == null) updateThreadSearch(resolveHost(), { current: t.key })
    revealRef.current = t.key
    latest.current.unfollow()
    latest.current.show(t.id)
    schedulePaint()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [targetKey])

  useEffect(() => {
    schedulePaint()
  }, [paint, schedulePaint])

  // Rows the window draws (scrolling, a page landing) get painted too.
  useEffect(() => {
    const content = o.content.current
    if (!active || !content || typeof MutationObserver === 'undefined') return
    const mo = new MutationObserver(schedulePaint)
    mo.observe(content, { childList: true, subtree: true, characterData: true })
    return () => mo.disconnect()
  }, [active, o.content, schedulePaint])

  useEffect(
    () => () => {
      if (rafRef.current) cancelAnimationFrame(rafRef.current)
      rafRef.current = 0
      clearThreadSearchPaint(ownerRef.current)
    },
    [],
  )
}
