// Sender icons in a Thread (Rosson 2026-11-04): once more than two people
// are talking, each other sender's messages get a small column on their
// left with that sender's picture, so a Thread with a second agent or
// another person reads like a group chat.
//
//   - Who counts (Rosson 2026-11-09, "in the loaded list of messages";
//     the phone will build the same rule): the distinct senders of every
//     message the Thread has loaded (all fetched pages, plus sends in
//     progress), not just the rows drawn. You never count, from any
//     device. Two or more others: the column shows. Once on it stays on
//     while that Thread is open (`useStickyOn`), so nothing jumps back
//     when older pages load or the render window moves; a different
//     Thread starts over.
//   - Every message from another sender gets the column, so they line up;
//     the picture draws once per run from one sender, on its LAST message,
//     at the bottom left of the bubble, level with its tail (Rosson
//     2026-11-18: the phone-chat place). The rest of the run leaves the
//     space empty; the name line stays over the first bubble (threadRuns).
//     A run ends where threadRuns ends it: another sender, you, a gap, a
//     deleted message or one of K2's notices. A new message from the same
//     sender takes the picture down to it (no animation: it just moves).
//     Files, a context line or chips under the bubble never push it down:
//     it sits against the bubble. Your messages never move.
//   - The picture is the one K2 already paints for that principal: an
//     agent's own image and colour (`ProjectAvatar`, the Agents page and
//     the Connections picker; another server's agent from the Home avatar
//     cache), a person's chip (`PresenceChip`, the roster's colour and
//     names, else the username hash). No fetch per message: the stores
//     already hold these, and `ProjectAvatar`'s lookup is cached per path.
//   - Motion: the column eases in (the bubble's left margin, 0 → the
//     column) and the picture fades and grows in; the reverse on the way
//     out. CSS transitions only, ~200 ms ease-out; reduced motion: none.
//   - The bubble's width is kept (a margin, not padding), so a wide column
//     re-wraps nothing. A narrow one (the bubble fills the column) does;
//     `useThreadSenderAnchor` keeps the row the reader is looking at where
//     it was while that happens.
//
// Per-client view state: the renderer is the right home (nothing for the
// daemon to own). Whether a post is the viewer's own is the caller's
// `mine` (threadAuthorLabel / isOwnThreadPost); this file never decides it.

import { createContext, memo, Suspense, useContext, useLayoutEffect, useRef, type ComponentType, type CSSProperties, type JSX } from 'react'
import { isPersonThreadPost, parseThreadAuthor } from '@/lib/thread-viewer'
import { displayThreadAddress } from '@/lib/thread-address-bus'
import type { OverlayDoc } from '@/components/SessionView/overlayThread'

/** The picture's side (px). */
export const SENDER_AVATAR_PX = 24
/** The column: the picture and the space before the bubble. */
export const SENDER_GUTTER_PX = SENDER_AVATAR_PX + 8
/** How long the column takes to come and go (ms). */
export const SENDER_MOTION_MS = 200
/** The class that turns the motion on (absent with reduced motion). */
export const SENDER_MOTION_CLASS = 'thread-sender-motion'

/** Who sent a row, as far as the column cares. */
export interface ThreadSender {
  /** Identity for "same sender" (a person by login, an agent by address). */
  key: string
  /** `name`: a name only (a message moved in from an older store): its
   *  letter, never a person's or an agent's picture. */
  kind: 'agent' | 'person' | 'name'
  /** The name on the picture's tooltip (and its letter). */
  label: string
  /** A person: the login the roster keys colours and names by. An agent:
   *  its address as shown (`handle`, `handle/sidecar`, `handle::host`). */
  id: string
}

/** One drawn row: its key, its sender, and whether it is the viewer's. */
export interface ThreadSenderRow {
  id: string
  sender: ThreadSender
  mine: boolean
}

/** What a row's bubble draws for the column (all values, so a memoized
 *  bubble compares it field by field). */
export interface ThreadSenderSlot {
  /** The column is shown (more than two people in the drawn rows). */
  on: boolean
  /** This row draws the picture (the last of its sender's run). */
  last: boolean
  /** Animate (false with reduced motion). */
  motion: boolean
  kind: ThreadSender['kind']
  label: string
  id: string
  /** The room's server when it is not the window's own (`hostKey`), so an
   *  agent's picture comes from the Home avatar cache; null: the window's
   *  server (the projects store). */
  host: string | null
}

const fold = (s: string): string => s.trim().toLowerCase()

/** A person's post: the daemon stamped a person (older docs: compose); an
 *  agent's stamp (0.46.13) is the agent's, keyed by its address below. */
function isPersonDoc(doc: OverlayDoc): boolean {
  return isPersonThreadPost(doc)
}

/** Who sent `doc`. A person is keyed by the daemon's stamp (kind + login),
 *  so the owner on two clients is one sender; an agent by its address as
 *  this Thread shows it (an old address reads as the current one). */
export function threadSender(doc: OverlayDoc, currentAddr?: string, pastAddresses?: readonly string[]): ThreadSender {
  const from = doc.from?.trim() || ''
  const named = doc.senderName?.trim()
  if (named) return { key: `name:${fold(named)}`, kind: 'name', label: named, id: '' }
  if (isPersonDoc(doc)) {
    const author = parseThreadAuthor(doc.author)
    const login = author ? (author.kind === 'owner' ? 'owner' : author.login) : from
    const key = author ? `person:${fold(author.kind)}:${fold(login)}` : `person:compose:${fold(from)}`
    return { key, kind: 'person', label: from || login || 'someone', id: login || from }
  }
  const addr = displayThreadAddress(from, currentAddr, pastAddresses) || 'unknown'
  return { key: `agent:${fold(addr)}`, kind: 'agent', label: addr, id: addr }
}

/** The distinct senders of `rows` other than the viewer. */
export function threadOtherSenders(rows: readonly Pick<ThreadSenderRow, 'sender' | 'mine'>[]): Set<string> {
  const out = new Set<string>()
  for (const r of rows) if (!r.mine) out.add(r.sender.key)
  return out
}

/** Show the column: two or more others among the drawn rows. */
export function threadSendersShown(rows: readonly Pick<ThreadSenderRow, 'sender' | 'mine'>[]): boolean {
  return threadOtherSenders(rows).size >= 2
}

/** `on` once it has been true, until `key` (the Thread) changes. */
export function useStickyOn(on: boolean, key: string): boolean {
  const ref = useRef({ key, on })
  if (ref.current.key !== key) ref.current = { key, on }
  else if (on && !ref.current.on) ref.current = { key, on: true }
  return ref.current.on
}

/** The column for each drawn row (`slots`, by row id). Rows of the viewer
 *  get none; every other row gets one, `last` on the last of a run: the
 *  Thread's runs (`ends`, threadRunEnds: the same rows the tail goes on),
 *  else a run ends at any row from someone else, the viewer included. `on`
 *  is the same for all: the column is there for the whole list or for
 *  none. */
export function threadSenderSlots(
  rows: readonly ThreadSenderRow[],
  opts: { motion: boolean; host: string | null; ends?: readonly boolean[]; on?: boolean },
): { on: boolean; slots: Map<string, ThreadSenderSlot> } {
  // `on`: the caller's answer (the loaded messages, kept on); else these rows.
  const on = opts.on ?? threadSendersShown(rows)
  const out = new Map<string, ThreadSenderSlot>()
  const keyOf = (r: ThreadSenderRow | undefined): string | null => (!r || r.mine ? null : r.sender.key)
  rows.forEach((r, i) => {
    const key = keyOf(r)
    if (key != null) {
      out.set(r.id, {
        on,
        last: opts.ends ? opts.ends[i] !== false : key !== keyOf(rows[i + 1]),
        motion: opts.motion,
        kind: r.sender.kind,
        label: r.sender.label,
        id: r.sender.id,
        host: opts.host,
      })
    }
  })
  return { on, slots: out }
}

/** Two slots draw the same. */
export function sameSenderSlot(a: ThreadSenderSlot | null | undefined, b: ThreadSenderSlot | null | undefined): boolean {
  if (a === b) return true
  if (!a || !b) return false
  return (
    a.on === b.on &&
    a.last === b.last &&
    a.motion === b.motion &&
    a.kind === b.kind &&
    a.label === b.label &&
    a.id === b.id &&
    a.host === b.host
  )
}

/** The rules: the margin and the picture ease; reduced motion, never. */
export const THREAD_SENDER_CSS = `
[data-sender-column].${SENDER_MOTION_CLASS} { transition: margin-left ${SENDER_MOTION_MS}ms ease-out, max-width ${SENDER_MOTION_MS}ms ease-out; }
[data-sender-column].${SENDER_MOTION_CLASS} [data-sender-avatar] { transition: opacity ${SENDER_MOTION_MS}ms ease-out, transform ${SENDER_MOTION_MS}ms ease-out; }
[data-sender-column="off"] [data-sender-avatar] { opacity: 0; transform: scale(0.6); pointer-events: none; }
[data-sender-column="on"] [data-sender-avatar] { opacity: 1; transform: none; }
@media (prefers-reduced-motion: reduce) {
  [data-sender-column], [data-sender-column] [data-sender-avatar] { transition: none !important; }
}
`

/** What draws the picture inside a row's frame. The Thread that turns the
 *  column on provides it (`ThreadSenderPicture`, which reads the projects,
 *  presence and Home avatar stores, loaded lazily); the shared kit and
 *  this file stay free of those stores. Absent: an empty frame. */
export const ThreadSenderAvatarContext = createContext<ComponentType<{ slot: ThreadSenderSlot }> | null>(null)

const FRAME_STYLE: CSSProperties = {
  position: 'absolute',
  left: -SENDER_GUTTER_PX,
  bottom: 0,
  width: SENDER_AVATAR_PX,
  height: SENDER_AVATAR_PX,
  overflow: 'hidden',
  borderRadius: 'calc(var(--zen-radius, 8px) / 2)',
  transformOrigin: 'center',
}

/** The picture's frame in a row's column (absolute, in the left margin of
 *  the box that holds the bubble, at its bottom: level with the tail), on
 *  the last row of a run. It stays mounted while the column is off, faded
 *  out, so going and coming back both animate. */
export const ThreadSenderFrame = memo(function ThreadSenderFrame({ slot }: { slot: ThreadSenderSlot }): JSX.Element | null {
  const Picture = useContext(ThreadSenderAvatarContext)
  if (!slot.last) return null
  return (
    <span
      data-sender-avatar={slot.kind}
      data-testid="thread-sender-avatar"
      aria-hidden={!slot.on}
      title={slot.on ? slot.label : undefined}
      style={FRAME_STYLE}
    >
      {Picture ? (
        <Suspense fallback={null}>
          <Picture slot={slot} />
        </Suspense>
      ) : null}
    </span>
  )
}, (a, b) => sameSenderSlot(a.slot, b.slot))

/** What a bubble's root takes for its slot: the marks, the margin, and
 *  (narrow column, where the bubble fills the width) a max width the
 *  column comes out of. `maxWidth` is the bubble's own. */
export function threadSenderRootProps(
  slot: ThreadSenderSlot | null | undefined,
  maxWidth: string,
  narrow: boolean,
): { attrs: Record<string, string | undefined>; className: string; style: CSSProperties } {
  if (!slot) return { attrs: {}, className: '', style: {} }
  const px = slot.on ? SENDER_GUTTER_PX : 0
  return {
    attrs: { 'data-sender-column': slot.on ? 'on' : 'off' },
    className: slot.motion ? SENDER_MOTION_CLASS : '',
    style: {
      position: 'relative',
      marginLeft: px,
      // Wide: the bubble keeps its ratio and the column sits in the room
      // beside it. Narrow: the bubble fills the column, so it gives way.
      ...(narrow ? { maxWidth: `calc(${maxWidth} - ${px}px)` } : {}),
    },
  }
}

/** Keep the row the reader is looking at in place while the column comes
 *  or goes (a narrow column re-wraps the bubbles). Not while following the
 *  newest: the list's own pin keeps the bottom. Call it after
 *  `useRenderWindow` (its window move lands first; this one then puts the
 *  row back on screen where it was). */
export function useThreadSenderAnchor(
  on: boolean,
  scroller: () => HTMLElement | null,
  following: () => boolean,
  onScrollWrite?: (top: number) => void,
): void {
  const prevOn = useRef(on)
  const anchor = useRef<{ key: string; y: number } | null>(null)
  // Before the commit (the render that flips the column): the first row
  // on screen and where it sits in the view.
  if (prevOn.current !== on) {
    prevOn.current = on
    anchor.current = following() ? null : captureRow(scroller())
  }
  const writeRef = useRef(onScrollWrite)
  writeRef.current = onScrollWrite
  useLayoutEffect(() => {
    const a = anchor.current
    anchor.current = null
    const el = scroller()
    if (!a || !el) return
    const row = findRow(el, a.key)
    if (!row) return
    const write = (dy: number): void => {
      if (Math.abs(dy) < 1) return
      el.scrollTop += dy
      writeRef.current?.(el.scrollTop)
    }
    // Now: back where the reader saw it (a window move in this commit has
    // already put it back; then this is a no-op).
    write(row.getBoundingClientRect().top - el.getBoundingClientRect().top - a.y)
    // Through the transition (a narrow column re-wraps frame by frame):
    // follow the row's place in the content only, so the reader's own
    // scrolling meanwhile is never undone.
    const contentY = (): number => row.getBoundingClientRect().top - el.getBoundingClientRect().top + el.scrollTop
    let last = contentY()
    const until = performance.now() + SENDER_MOTION_MS + 60
    let raf = 0
    const tick = (): void => {
      if (!row.isConnected) return
      const y = contentY()
      write(y - last)
      last = contentY()
      if (performance.now() < until) raf = requestAnimationFrame(tick)
    }
    raf = requestAnimationFrame(tick)
    return () => cancelAnimationFrame(raf)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [on])
}

function rowsOf(el: HTMLElement): HTMLElement[] {
  return Array.from(el.querySelectorAll<HTMLElement>('[data-zen-message]'))
}

function captureRow(el: HTMLElement | null): { key: string; y: number } | null {
  if (!el || !(el.clientHeight > 0)) return null
  const top = el.getBoundingClientRect().top
  for (const r of rowsOf(el)) {
    const box = r.getBoundingClientRect()
    if (box.bottom > top + 1) {
      const key = r.getAttribute('data-zen-message')
      return key ? { key, y: box.top - top } : null
    }
  }
  return null
}

function findRow(el: HTMLElement, key: string): HTMLElement | null {
  for (const r of rowsOf(el)) if (r.getAttribute('data-zen-message') === key) return r
  return null
}
