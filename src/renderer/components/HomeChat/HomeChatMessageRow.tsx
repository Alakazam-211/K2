// One message in a Home chat (prd-home-people-channels-v1 §8–§11): the
// Thread's bubble (`ThreadChatBubble`), the reply's quote strip inside it
// (click: jump to the original), reaction chips under it, and the action
// bar (React, Reply, ⋯). A long press on touch, or a right-click, opens the
// same three as a menu (`useThreadMessageGestures`, the Thread's). No
// @mentions: `@name` is plain text in the body (D18).
//
// The action bar (the hover row) sits on the bubble's top corner away from
// the sender, as chat apps put it: top right on theirs, top left on yours.
// It reaches outward from the corner and overlaps the bubble's top edge by
// less than the bubble's top padding, so it never covers the words; it is
// fixed to the top edge, so it sits in the same place on every bubble, short
// or long, and doesn't move when chips or a quote change the bubble's
// height. Where the column's edge leaves no room outward it flips inward,
// over the top edge inside the corner (`actionBarPlace`). It shows on hover
// and while keyboard focus is inside the message (`useHoverReveal`), in the
// look's button style (the Thread's chamfered control in Terminal, a pill in
// Glass).
//
// By keyboard: a message is a Tab stop (the shared k2-focus ring); focus on
// it shows its row, and Tab goes on to React, Reply, More before the
// bubble's own links and the chips. Arrows move along the row; Esc goes
// back to the message.
//
// A message just jumped to gets a soft accent wash that fades over
// JUMP_WASH_MS (none of it moves under reduced motion: the wash just goes).
// The wash sits behind the row's words (the row is its own layer while
// washed), and every word on it reads at 4.5:1: the name and time line takes
// the secondary text for the while (the muted text isn't 4.5:1 on a light
// wash).
//
// The row's own element carries `data-zen-message` so the render window
// (`useRenderWindow`) counts and measures it like a Thread row.

import { memo, useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type JSX, type KeyboardEvent } from 'react'
import { EllipsisIcon, SmilePlusIcon } from 'lucide-react'
import { ThreadChatBubble } from '@/components/Thread/ThreadChat'
import { ReplyGlyph, ThreadReplyQuote, useThreadMessageGestures } from '@/components/Thread/ThreadMessageMenu'
import { CHAMFER_CUT, chamferVars } from '@/components/Thread/chamfer'
import { K2_ICON_STROKE } from '@/components/icons/K2Icons'
import { useContextMenuStore } from '@/stores/context-menu'
import { copyToClipboard } from '@/components/common/chat-copy'
import { useToastStore } from '@/stores/toast'
import { clientRectToCss } from '@/lib/app-zoom'
import type { HomeChatMessage } from '@/lib/home-chat/api'
import type { ReplyPreview } from '@/lib/home-chat/chat'
import { HomeChatReactionChips, HomeChatReactionPicker } from './HomeChatReactions'
import { useHoverReveal } from './useHoverReveal'
import { useReducedMotion } from '@/hooks/useReducedMotion'

export interface HomeChatRowActions {
  react: (msgId: string, emoji: string) => void
  reply: (m: HomeChatMessage) => void
  jump: (msgId: string) => void
  who: (msgId: string, emoji: string) => Promise<string[]>
}

/** How long a jumped-to message's wash takes to fade (ms). */
export const JUMP_WASH_MS = 1200
/** How much of the accent the wash is, over the page (%). */
export const JUMP_WASH_PCT = 12

/** The row while washed: its own layer, its muted words the secondary text. */
const WASHED_ROW = { isolation: 'isolate', '--zen-text-muted': 'var(--color-text-secondary)' } as CSSProperties

/** The action bar's buttons (px, square) and its inset around them. */
const ACTION_PX = 26
const BAR_PAD = 3
export const ACTION_BAR_PX = ACTION_PX + BAR_PAD * 2
/** The whole bar (three buttons, its inset and its 1px edge), used until
 *  it has been measured. */
const BAR_SIZE_FALLBACK = { width: ACTION_PX * 3 + BAR_PAD * 2 + 2, height: ACTION_BAR_PX + 2 }
/** How far the bar reaches over the bubble's outer corner (px), and how far
 *  down over its top edge: less than the bubble's 9px top padding, so it
 *  never covers the first line. */
export const BAR_OVER_X = 16
export const BAR_OVER_Y = 8
/** Turned inward, the bar's end stays this far inside the bubble's corner;
 *  turned outward, this far inside the column's edge. */
const BAR_IN_X = 6
const BAR_COLUMN_PAD = 4

export interface ActionBarPlace {
  /** From the row's top (px). */
  top: number
  /** From the sender's side of the row (px): its left edge on theirs, its
   *  right edge on yours. */
  inset: number
  /** Turned inward over the bubble (no room outward at the column's edge). */
  inward: boolean
}

/** Where the bar goes for a bubble (rects relative to the row, px): on its
 *  outer top corner, reaching outward, BAR_OVER_Y down over its top edge;
 *  inward over the top edge when the column has no room outward; never
 *  outside the row. Only the bubble's top edge sets the height, so a tall
 *  bubble gets it where a short one does (exported for tests). */
export function actionBarPlace(
  row: { width: number },
  bubble: { left: number; right: number; top: number; bottom?: number },
  bar: { width: number; height: number },
  mine: boolean,
): ActionBarPlace {
  const top = Math.round(bubble.top - bar.height + BAR_OVER_Y)
  // The bubble's outer corner, from the sender's side of the row.
  const corner = mine ? row.width - bubble.left : bubble.right
  const out = corner - BAR_OVER_X
  if (out + bar.width <= row.width - BAR_COLUMN_PAD) return { top, inset: Math.round(out), inward: false }
  const inside = Math.min(corner - bar.width - BAR_IN_X, row.width - bar.width)
  return { top, inset: Math.round(Math.max(0, inside)), inward: true }
}

const ACTION_BUTTON_CLASS =
  'k2-focus no-drag inline-flex flex-shrink-0 cursor-pointer items-center justify-center rounded-[var(--radius-pill)] text-[var(--zen-text-muted)] hover:bg-[var(--color-wash-1)] hover:text-[var(--zen-text)] focus-visible:text-[var(--zen-text)]'
const ACTION_BUTTON_STYLE: CSSProperties = {
  width: ACTION_PX,
  height: ACTION_PX,
  background: undefined,
  border: 'none',
}

const BAR_FILL = 'var(--zen-surface-raised)'
const BAR_EDGE = 'var(--zen-control-edge)'
const BAR_STYLE: CSSProperties = {
  gap: 0,
  padding: BAR_PAD,
  border: `1px solid ${BAR_EDGE}`,
  background: BAR_FILL,
  ...chamferVars({
    cut: CHAMFER_CUT.button,
    fill: BAR_FILL,
    edge: BAR_EDGE,
    border: 1,
  }),
}

/** How far the bar rises as it fades in (px). */
const BAR_LIFT = 3
const FADE_IN = 'var(--motion-duration-fast) var(--motion-ease-standard)'
const FADE_OUT = 'calc(var(--motion-duration-fast) * 0.75) var(--motion-ease-standard)'

/** Shown, or hidden (and out of the tab order) until placed and wanted. It
 *  fades and rises in on the fast motion token and fades out a little
 *  faster, its visibility waiting for the fade; none of it moves under
 *  reduced motion. */
function barReveal(shown: boolean, reduced: boolean): CSSProperties {
  return {
    visibility: shown ? 'visible' : 'hidden',
    opacity: shown ? 1 : 0,
    pointerEvents: shown ? 'auto' : 'none',
    transform: shown || reduced ? 'none' : `translateY(${BAR_LIFT}px)`,
    transition: reduced
      ? 'none'
      : shown
        ? `opacity ${FADE_IN}, transform ${FADE_IN}`
        : `opacity ${FADE_OUT}, transform ${FADE_OUT}, visibility 0s linear calc(var(--motion-duration-fast) * 0.75)`,
  }
}

const DELETED_STYLE = {
  fontSize: '0.8em',
  fontStyle: 'italic',
  color: 'var(--zen-text-muted)',
  padding: '4px 8px',
} as const

/** Where a key moves focus along the bar's `n` buttons from `at`. */
function barKeyTarget(key: string, at: number, n: number): number | null {
  if (key === 'ArrowRight') return (at + 1) % n
  if (key === 'ArrowLeft') return (at - 1 + n) % n
  if (key === 'Home') return 0
  if (key === 'End') return n - 1
  return null
}

function quoteText(p: ReplyPreview): { from: string; line: string } {
  if (p.state === 'loaded') return { from: p.author, line: p.line || '(no text)' }
  if (p.state === 'deleted') return { from: 'Reply', line: 'Original message deleted' }
  return { from: 'Reply', line: 'Original not loaded: click to find it' }
}

function prefersReducedMotion(): boolean {
  try {
    return typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches
  } catch {
    return false
  }
}

export const HomeChatMessageRow = memo(function HomeChatMessageRow({
  m,
  mine,
  author,
  timeLabel,
  still,
  runStart,
  tail,
  reply,
  flash,
  readOnly,
  actions,
}: {
  m: HomeChatMessage
  mine: boolean
  author: string
  timeLabel: string
  still: boolean
  runStart: boolean
  tail: boolean
  /** The original this message replies to, when it is a reply. */
  reply: ReplyPreview | null
  /** Just jumped to: a soft wash that fades. */
  flash: boolean
  /** No react or reply (an archived channel, a DM with a disabled login). */
  readOnly: boolean
  actions: HomeChatRowActions
}): JSX.Element {
  const [picker, setPicker] = useState(false)
  const ref = useRef<HTMLDivElement | null>(null)
  const barRef = useRef<HTMLDivElement | null>(null)
  const reactRef = useRef<HTMLButtonElement | null>(null)
  const washRef = useRef<HTMLSpanElement | null>(null)
  const replyTo = m.replyTo
  const replyState = reply?.state
  const replyFrom = reply ? quoteText(reply).from : ''
  const replyLine = reply ? quoteText(reply).line : ''
  const quote = useMemo(
    () =>
      replyTo && replyState ? (
        <ThreadReplyQuote
          reply={{
            part: false,
            seq: 0,
            from: replyFrom,
            id: replyTo,
            quote: replyLine,
            rest: '',
          }}
          onJump={() => actions.jump(replyTo)}
        />
      ) : undefined,
    [replyTo, replyState, replyFrom, replyLine, actions],
  )
  const openMenu = useCallback(
    (x: number, y: number) => {
      const items = [
        ...(readOnly
          ? []
          : [
              { id: 'react', label: 'React…' },
              { id: 'reply', label: 'Reply', icon: <ReplyGlyph size={12} /> },
            ]),
        { id: 'copy', label: 'Copy text', enabled: m.body.length > 0 },
      ]
      void useContextMenuStore
        .getState()
        .show(x, y, items)
        .then(async (picked) => {
          if (picked === 'react') setPicker(true)
          else if (picked === 'reply') actions.reply(m)
          else if (picked === 'copy') {
            try {
              await copyToClipboard(m.body)
            } catch (err) {
              useToastStore.getState().addToast(`Couldn't copy: ${err instanceof Error ? err.message : String(err)}`, 'error')
            }
          }
        })
    },
    [m, readOnly, actions],
  )
  const gestures = useThreadMessageGestures(ref, m.deleted ? null : openMenu)
  const toggle = useCallback((emoji: string) => actions.react(m.id, emoji), [actions, m.id])
  const who = useCallback((emoji: string) => actions.who(m.id, emoji), [actions, m.id])

  // The bar's place, measured once as it appears (useHoverReveal).
  const measure = useCallback((): ActionBarPlace | null => {
    const row = ref.current
    const bubble = row?.querySelector<HTMLElement>('[data-zen-bubble]')
    if (!row || !bubble) return null
    const r = clientRectToCss(row.getBoundingClientRect())
    const b = clientRectToCss(bubble.getBoundingClientRect())
    if (r.width === 0) return null
    const bar = barRef.current?.firstElementChild as HTMLElement | null | undefined
    const size = { width: bar?.offsetWidth || BAR_SIZE_FALLBACK.width, height: bar?.offsetHeight || BAR_SIZE_FALLBACK.height }
    return actionBarPlace({ width: r.width }, { left: b.left - r.left, right: b.right - r.left, top: b.top - r.top }, size, mine)
  }, [mine])
  const { shown, place, handlers: reveal } = useHoverReveal(measure, picker, !m.deleted)
  const reduced = useReducedMotion()

  // The jump's wash: shown, then faded out over JUMP_WASH_MS.
  useEffect(() => {
    const el = washRef.current
    if (!flash || !el || prefersReducedMotion() || typeof el.animate !== 'function') return
    const anim = el.animate([{ opacity: 1 }, { opacity: 1, offset: 0.15 }, { opacity: 0 }], { duration: JUMP_WASH_MS, easing: 'ease-out', fill: 'forwards' })
    return () => anim.cancel()
  }, [flash])

  // Along the row: arrows (wrapping), Home and End; Esc back to the message.
  const onBarKey = (e: KeyboardEvent<HTMLDivElement>): void => {
    if (e.key === 'Escape') {
      e.preventDefault()
      e.stopPropagation()
      ref.current?.focus({ preventScroll: true })
      return
    }
    const buttons = Array.from(e.currentTarget.querySelectorAll<HTMLButtonElement>('button'))
    const at = buttons.indexOf(document.activeElement as HTMLButtonElement)
    const to = barKeyTarget(e.key, at, buttons.length)
    if (to === null || at < 0) return
    e.preventDefault()
    buttons[to].focus()
  }

  const side = mine ? 'right' : 'left'
  const barPos: CSSProperties = place ? { top: place.top, [side]: place.inset } : { top: BAR_OVER_Y - BAR_SIZE_FALLBACK.height, [side]: 0 }

  return (
    <div
      ref={ref}
      data-zen-message={m.id}
      data-home-chat-message=""
      data-mine={mine ? '' : undefined}
      data-flash={flash ? '' : undefined}
      role={m.deleted ? undefined : 'article'}
      aria-label={m.deleted ? undefined : mine ? 'Your message' : `Message from ${author}`}
      tabIndex={m.deleted ? undefined : 0}
      className="k2-focus relative flex flex-col rounded-[calc(var(--radius-box)/2)]"
      style={{ alignItems: mine ? 'flex-end' : 'flex-start', minWidth: 0, ...(flash ? WASHED_ROW : null) }}
      {...reveal}
      {...(m.deleted ? {} : gestures)}
    >
      {flash && (
        <span
          ref={washRef}
          aria-hidden="true"
          data-home-chat-wash=""
          className="pointer-events-none absolute inset-0 rounded-[calc(var(--radius-box)/2)]"
          style={{
            zIndex: -1,
            background: `color-mix(in srgb, var(--zen-accent) ${JUMP_WASH_PCT}%, transparent)`,
          }}
        />
      )}
      {!m.deleted && (
        // The outer box places it (the Thread's chamfer rule makes a cut box
        // position: relative, so the cut is on the inner toolbar).
        <div
          ref={barRef}
          data-home-chat-actions=""
          data-inward={place?.inward ? '' : undefined}
          data-shown={shown ? '' : undefined}
          className="absolute z-[3]"
          style={{ ...barPos, ...barReveal(shown, reduced) }}
        >
          <div
            role="toolbar"
            aria-label="Message actions"
            data-chamfer=""
            onKeyDown={onBarKey}
            className="flex rounded-[var(--radius-pill)] [html[data-style=glass]_&]:[box-shadow:var(--shadow-2)]"
            style={BAR_STYLE}
          >
            {!readOnly && (
              <button
                ref={reactRef}
                type="button"
                aria-label="React"
                title="React"
                data-testid="home-chat-react"
                className={ACTION_BUTTON_CLASS}
                style={ACTION_BUTTON_STYLE}
                onClick={() => setPicker((v) => !v)}
              >
                <SmilePlusIcon size={15} strokeWidth={K2_ICON_STROKE} aria-hidden="true" />
              </button>
            )}
            {!readOnly && (
              <button
                type="button"
                aria-label="Reply"
                title="Reply"
                data-testid="home-chat-reply"
                className={ACTION_BUTTON_CLASS}
                style={ACTION_BUTTON_STYLE}
                onClick={() => actions.reply(m)}
              >
                <ReplyGlyph size={15} />
              </button>
            )}
            <button
              type="button"
              aria-label="More"
              title="More"
              data-testid="home-chat-more"
              className={ACTION_BUTTON_CLASS}
              style={ACTION_BUTTON_STYLE}
              onClick={(e) => {
                const r = e.currentTarget.getBoundingClientRect()
                openMenu(r.left, r.bottom)
              }}
            >
              <EllipsisIcon size={15} strokeWidth={K2_ICON_STROKE} aria-hidden="true" />
            </button>
          </div>
        </div>
      )}
      {m.deleted ? (
        <span data-home-chat-deleted="" role="note" style={DELETED_STYLE}>
          Message deleted
        </span>
      ) : (
        <ThreadChatBubble
          id={m.id}
          seq={m.seq}
          author={author}
          mine={mine}
          body={m.body}
          createdAt={m.createdAt}
          timeLabel={timeLabel}
          still={still}
          runStart={runStart}
          tail={tail}
          quote={quote}
        />
      )}
      <HomeChatReactionChips reactions={m.reactions} mine={mine} onToggle={toggle} whoFor={who} disabled={readOnly} />
      {picker && (
        <HomeChatReactionPicker anchor={reactRef.current ?? ref.current} align={mine ? 'end' : 'start'} onPick={toggle} onClose={() => setPicker(false)} />
      )}
    </div>
  )
})
