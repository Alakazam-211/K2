// "See more" on a long Thread message, ported from the phone app's fold so
// both read the same: a message taller than its fold shows its first lines,
// a fade into the bubble's own fill and a "See more" pill with a caret.
// Open, "See less" under the words folds it again.
//
// What counts is the rendered height of the words, against the body's own
// computed line-height, so code blocks, lists and headings count by the room
// they take, not by their characters.
//
// The layout never moves on its own: folded, the clip's max-height is on
// from the very first paint (THREAD_FOLD_LINES lines, plus one for the
// pill's row), so a long message from the cache, an older page or a reply
// still streaming in lays out at its folded height at once. Measuring only
// decides whether the fade and the pill (drawn over the clip's last row)
// show. Only a press changes the height, and the press keeps the bubble's
// top where it was (and, folding, in view).
//
// The fold wraps the words only: a reply's quote strip, cards, the files
// under a message and its status line are never inside it. Copy, Reply and
// Save to stash read the message's text, never the folded view. The look
// is in messageFoldCss.ts.

import {
  createContext,
  useContext,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type JSX,
  type ReactNode,
} from 'react'
import { flushSync } from 'react-dom'

/**
 * Lines a folded message shows on the desktop. A message whose words fit in
 * THREAD_FOLD_LINES + 1 lines (20) never folds; a taller one shows 19 lines
 * and its 20th row holds the fade and "See more". The phone shows 14 of 15:
 * a desktop window has room for more.
 */
export const THREAD_FOLD_LINES = 19

/** The clip holds one row more than it shows: the fade and the pill sit on
 *  it. A message that fits in `lines + 1` never folds (a pill that hid one
 *  line would cost the line it hides). */
export function foldClipLines(lines: number = THREAD_FOLD_LINES): number {
  return lines + 1
}

/** `.markdown-content`'s line-height (globals.css): the clip's height before
 *  the first measurement, in the body's own `em`. */
const CSS_LINE_EM = 1.6
/** A bubble's top left above the view (folding back): it comes back with
 *  this much air under the floating header. */
const VIEW_GAP_PX = 8

/** Dispatched on any node inside a fold: the fold opens (a search hit in
 *  the hidden part). `revealFolded` sends it. */
export const FOLD_REVEAL_EVENT = 'k2-fold-reveal'

/** Open the fold around `node`, if it sits in a folded message. */
export function revealFolded(node: Element): void {
  node.dispatchEvent(new CustomEvent(FOLD_REVEAL_EVENT, { bubbles: true }))
}

/** Is `node` inside a fold that is folded now? */
export function insideFolded(node: Element | null): boolean {
  return !!node?.closest('[data-fold="folded"]')
}

/** The clip's height for a line-height, px. */
export function foldClipPx(linePx: number, lines: number = THREAD_FOLD_LINES): number {
  return Math.round(linePx * foldClipLines(lines))
}

/** Taller than the folded clip (half a pixel of rounding aside). */
export function foldable(contentPx: number, linePx: number, lines: number = THREAD_FOLD_LINES): boolean {
  return linePx > 0 && contentPx > foldClipPx(linePx, lines) + 0.5
}

/** A body's computed line-height, px (WebKit resolves a unitless
 *  `line-height` to px; `normal` is about 1.2em). 0 when unreadable. */
export function readLinePx(el: Element): number {
  const cs = getComputedStyle(el)
  const raw = cs.lineHeight
  const font = parseFloat(cs.fontSize)
  if (raw.endsWith('px')) return parseFloat(raw) || 0
  const n = Number(raw)
  if (raw && Number.isFinite(n) && n > 0) return Number.isFinite(font) ? n * font : 0
  return Number.isFinite(font) ? font * 1.2 : 0
}

/** What the list around the folds lends them: the memory of which messages
 *  are open (kept while the Thread is open; a row the render window drops
 *  comes back as it was), the scroller, and its follow, so opening a
 *  message near the newest doesn't stick the list to the bottom under the
 *  reader. A Thread without a host (a Garden's) draws no folds. */
export type FoldHost = {
  /** Message ids open now. */
  open: Set<string>
  /** Lines a folded message shows (THREAD_FOLD_LINES by default). */
  lines?: number
  scroller(): HTMLElement | null
  /** Stop sticking to the newest (a fold opening under the reader). */
  unfollow(): void
  /** Folding back left the view at the newest: follow again. */
  refollow?(): void
  /** The scroller moved by us (not the reader): its scroll memory takes it. */
  ownScroll?(el: HTMLElement): void
}

export const FoldContext = createContext<FoldHost | null>(null)

/** A new host: one per open list, stable for its life. */
export function createFoldHost(over: Omit<FoldHost, 'open'>): FoldHost {
  return { open: new Set<string>(), ...over }
}

/** The list's fold host, if it has one. */
export function useFoldHost(): FoldHost | null {
  return useContext(FoldContext)
}

function scrollParent(el: HTMLElement): HTMLElement | null {
  for (let p = el.parentElement; p; p = p.parentElement) {
    const oy = getComputedStyle(p).overflowY
    if (oy === 'auto' || oy === 'scroll') return p
  }
  return null
}

/** The px (line-height) the last fold measured: a new fold's clip starts
 *  there (the same body style everywhere), not at the CSS estimate. */
let knownLinePx = 0
/** Tests: forget the measured line-height. */
export function __resetFoldForTests(): void {
  knownLinePx = 0
}

/** The pill's caret, right of its words: down on "See more", up on "See
 *  less". Decoration only: the button's name stays its words. */
function FoldChevron({ up }: { up: boolean }): JSX.Element {
  return (
    <svg
      className="thread-fold-chevron"
      data-testid="fold-chevron"
      data-dir={up ? 'up' : 'down'}
      viewBox="0 0 12 12"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.75"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      <path d={up ? 'M3 7.5 6 4.5l3 3' : 'M3 4.5l3 3 3-3'} />
    </svg>
  )
}

/** The row a fold keeps still: the message's own row in the list. */
const ROW_SELECTOR = '[data-zen-message], [data-thread-msg]'

export function MessageFold({
  foldKey,
  fontSize,
  lines,
  reveal,
  children,
}: {
  /** The message's key in its list (its id). */
  foldKey: string
  /** The body's font-size, when the fold's box doesn't already have it
   *  (the clip's `em` before the first measurement). */
  fontSize?: string
  /** Lines a folded message shows: this, else the host's, else
   *  THREAD_FOLD_LINES. */
  lines?: number
  /** Landed on (a jump to this message): open it. */
  reveal?: boolean
  children: ReactNode
}): JSX.Element {
  const host = useContext(FoldContext)
  const shown = lines ?? host?.lines ?? THREAD_FOLD_LINES
  const rootRef = useRef<HTMLDivElement>(null)
  const innerRef = useRef<HTMLDivElement>(null)
  const clipId = useId()
  const [open, setOpen] = useState(() => host?.open.has(foldKey) ?? false)
  const [linePx, setLinePx] = useState(knownLinePx)
  const [long, setLong] = useState(false)
  const openRef = useRef(open)
  openRef.current = open
  const longRef = useRef(long)
  longRef.current = long
  const shownRef = useRef(shown)
  shownRef.current = shown

  // One observer per fold, on the unclipped words: their full height comes
  // through the clip. Only a crossing (or a new line-height) re-renders;
  // reads happen after layout, in the observer's callback (no thrash).
  useEffect(() => {
    const inner = innerRef.current
    if (!inner || typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver((entries?: ResizeObserverEntry[]) => {
      const e = entries?.[entries.length - 1]
      const h = e?.borderBoxSize?.[0]?.blockSize ?? e?.contentRect?.height ?? inner.offsetHeight
      const body = inner.querySelector('.chat-markdown') ?? inner
      const lh = readLinePx(body) || knownLinePx
      if (lh > 0) {
        knownLinePx = lh
        setLinePx((cur) => (Math.abs(cur - lh) > 0.25 ? lh : cur))
      }
      setLong(foldable(h, lh, shownRef.current))
    })
    ro.observe(inner)
    return () => ro.disconnect()
  }, [])

  /** Open or fold, the bubble's top kept where it was (folding: kept in
   *  view, or brought back under the header if the reader had scrolled
   *  past it). Measured now; corrected in the commit's layout effect,
   *  before paint. */
  const anchor = useRef<{ row: HTMLElement; sc: HTMLElement; want: number } | null>(null)
  const toggle = (next: boolean): void => {
    const root = rootRef.current
    if (!root || next === openRef.current) return
    const row = root.closest<HTMLElement>(ROW_SELECTOR) ?? root
    const sc = host?.scroller() ?? scrollParent(root)
    if (sc) {
      const before = row.getBoundingClientRect().top
      let want = before
      if (!next) {
        const view = sc.getBoundingClientRect().top + (parseFloat(getComputedStyle(sc).scrollPaddingTop) || 0)
        if (before < view) want = view + VIEW_GAP_PX
      }
      anchor.current = { row, sc, want }
    }
    // Opening grows the list below the bubble: sticking to the newest
    // would carry its top away. Folding: our own correction decides.
    host?.unfollow()
    if (next) host?.open.add(foldKey)
    else host?.open.delete(foldKey)
    setOpen(next)
  }
  useLayoutEffect(() => {
    const a = anchor.current
    anchor.current = null
    if (a) {
      const delta = a.row.getBoundingClientRect().top - a.want
      if (Math.abs(delta) >= 0.5) {
        a.sc.scrollTop += delta
        host?.ownScroll?.(a.sc)
      }
    }
    if (a && !open) host?.refollow?.()
  }, [open, host])
  const toggleRef = useRef(toggle)
  toggleRef.current = toggle

  // Landed on (a jump to this message): open, once per landing; "See less"
  // while still landed stays folded.
  const revealed = useRef(false)
  useLayoutEffect(() => {
    if (!reveal) {
      revealed.current = false
      return
    }
    if (!long || revealed.current) return
    revealed.current = true
    if (!openRef.current) toggleRef.current(true)
  }, [reveal, long])
  // A search hit in the hidden part: open now (flushSync), so the search
  // scrolls to the words where they are once open.
  useEffect(() => {
    const root = rootRef.current
    if (!root) return
    const onReveal = (e: Event): void => {
      e.stopPropagation()
      if (!openRef.current && longRef.current) flushSync(() => toggleRef.current(true))
    }
    root.addEventListener(FOLD_REVEAL_EVENT, onReveal)
    return () => root.removeEventListener(FOLD_REVEAL_EVENT, onReveal)
  }, [])

  const folded = !open
  const clipStyle: CSSProperties | undefined = folded
    ? { maxHeight: linePx > 0 ? `${foldClipPx(linePx, shown)}px` : `${foldClipLines(shown) * CSS_LINE_EM}em` }
    : undefined
  return (
    <div
      ref={rootRef}
      className="thread-fold"
      data-fold={long || open ? (open ? 'open' : 'folded') : undefined}
      // `em` here (the clip before measuring, the fade, the pill) is the
      // words' own; the words set theirs anyway.
      style={{ ...(fontSize ? { fontSize } : {}), ['--thread-fold-line' as string]: linePx > 0 ? `${linePx}px` : `${CSS_LINE_EM}em` }}
    >
      <div id={clipId} className="thread-fold-clip" style={clipStyle} data-testid="fold-clip">
        <div ref={innerRef} data-testid="fold-inner">
          {children}
        </div>
      </div>
      {folded && long ? <div className="thread-fold-fade" aria-hidden="true" data-testid="fold-fade" /> : null}
      {long || open ? (
        <button
          type="button"
          className="thread-fold-toggle"
          aria-expanded={open}
          aria-controls={clipId}
          data-testid="fold-toggle"
          data-chamfer=""
          onClick={(e) => {
            e.stopPropagation()
            toggle(!open)
          }}
        >
          {open ? 'See less' : 'See more'}
          <FoldChevron up={open} />
        </button>
      ) : null}
    </div>
  )
}
