// One message in a Home chat (prd-home-people-channels-v1 §8–§11): the
// Thread's bubble (`ThreadChatBubble`), the reply's quote strip inside it
// (click: jump to the original), reaction chips under it, and the action
// bar (React, Reply, ⋯). A long press on touch, or a right-click, opens the
// same three as a menu (`useThreadMessageGestures`, the Thread's). No
// @mentions: `@name` is plain text in the body (D18).
//
// The action bar floats at the bubble's top edge, beside it on the open side
// (right of theirs, left of yours), so it never covers the message's words;
// a bubble too wide to leave room puts it just above its outer corner
// instead (`actionBarPlace`). It shows on hover and while focus is inside
// the message, in the look's button style (the Thread's chamfered control
// in Terminal, a pill in Glass).
//
// A message just jumped to gets a soft accent wash that fades over
// JUMP_WASH_MS (none of it moves under reduced motion: the wash just goes).
//
// The row's own element carries `data-zen-message` so the render window
// (`useRenderWindow`) counts and measures it like a Thread row.

import { memo, useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type JSX } from 'react'
import { EllipsisIcon, SmilePlusIcon } from 'lucide-react'
import { ThreadChatBubble } from '@/components/Thread/ThreadChat'
import { ReplyGlyph, ThreadReplyQuote, useThreadMessageGestures } from '@/components/Thread/ThreadMessageMenu'
import { CHAMFER_CUT, chamferVars } from '@/components/Thread/chamfer'
import { K2_ICON_STROKE } from '@/components/icons/K2Icons'
import { useContextMenuStore } from '@/stores/context-menu'
import { copyToClipboard } from '@/components/common/chat-copy'
import { useToastStore } from '@/stores/toast'
import type { HomeChatMessage } from '@/lib/home-chat/api'
import type { ReplyPreview } from '@/lib/home-chat/chat'
import { HomeChatReactionChips, HomeChatReactionPicker } from './HomeChatReactions'

export interface HomeChatRowActions {
  react: (msgId: string, emoji: string) => void
  reply: (m: HomeChatMessage) => void
  jump: (msgId: string) => void
  who: (msgId: string, emoji: string) => Promise<string[]>
}

/** How long a jumped-to message's wash takes to fade (ms). */
export const JUMP_WASH_MS = 1200

/** The action bar's buttons (px, square) and its inset around them. */
const ACTION_PX = 26
const BAR_PAD = 3
export const ACTION_BAR_PX = ACTION_PX + BAR_PAD * 2
/** How far beside the bubble the bar sits, and how far above its top edge. */
const BAR_GAP = 6
const BAR_RISE = 8

export interface ActionBarPlace {
  /** From the row's top (px). */
  top: number
  /** From the row's left edge (theirs) or right edge (yours), px. */
  inset: number
  /** Beside the bubble (true), or over its outer top corner (no room). */
  beside: boolean
}

/** Where the bar goes for a bubble (rects relative to the row): beside it on
 *  the open side, level with its top edge; when the open side has no room
 *  for `barWidth`, just above its outer corner (exported for tests). */
export function actionBarPlace(row: { width: number }, bubble: { left: number; right: number; top: number }, barWidth: number, mine: boolean): ActionBarPlace {
  const room = mine ? bubble.left : row.width - bubble.right
  if (room >= barWidth + BAR_GAP) {
    return {
      top: Math.round(bubble.top - BAR_RISE),
      inset: Math.round(mine ? row.width - bubble.left + BAR_GAP : bubble.right + BAR_GAP),
      beside: true,
    }
  }
  const outer = mine ? Math.max(0, bubble.left) : Math.max(0, row.width - bubble.right)
  return {
    top: Math.round(bubble.top - ACTION_BAR_PX + 4),
    inset: Math.round(outer),
    beside: false,
  }
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

const DELETED_STYLE = {
  fontSize: '0.8em',
  fontStyle: 'italic',
  color: 'var(--zen-text-muted)',
  padding: '4px 8px',
} as const

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
  const [place, setPlace] = useState<ActionBarPlace | null>(null)
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

  // The bar's place, measured when it is about to show (hover or focus in).
  const measure = useCallback(() => {
    const row = ref.current
    const bubble = row?.querySelector<HTMLElement>('[data-zen-bubble]')
    if (!row || !bubble) return
    const r = row.getBoundingClientRect()
    const b = bubble.getBoundingClientRect()
    if (r.width === 0) return
    const w = barRef.current?.offsetWidth ?? ACTION_BAR_PX * 3
    setPlace(actionBarPlace({ width: r.width }, { left: b.left - r.left, right: b.right - r.left, top: b.top - r.top }, w, mine))
  }, [mine])

  // The jump's wash: shown, then faded out over JUMP_WASH_MS.
  useEffect(() => {
    const el = washRef.current
    if (!flash || !el || prefersReducedMotion() || typeof el.animate !== 'function') return
    const anim = el.animate([{ opacity: 1 }, { opacity: 1, offset: 0.15 }, { opacity: 0 }], { duration: JUMP_WASH_MS, easing: 'ease-out', fill: 'forwards' })
    return () => anim.cancel()
  }, [flash])

  const side = mine ? 'right' : 'left'
  const barPos: CSSProperties = place ? { top: place.top, [side]: place.inset } : { top: -BAR_RISE, [mine ? 'left' : 'right']: 0 }

  return (
    <div
      ref={ref}
      data-zen-message={m.id}
      data-home-chat-message=""
      data-mine={mine ? '' : undefined}
      data-flash={flash ? '' : undefined}
      className="group relative flex flex-col rounded-[calc(var(--radius-box)/2)]"
      style={{ alignItems: mine ? 'flex-end' : 'flex-start', minWidth: 0 }}
      onPointerEnter={m.deleted ? undefined : measure}
      onFocusCapture={m.deleted ? undefined : measure}
      {...(m.deleted ? {} : gestures)}
    >
      {flash && (
        <span
          ref={washRef}
          aria-hidden="true"
          data-home-chat-wash=""
          className="pointer-events-none absolute inset-0 rounded-[calc(var(--radius-box)/2)]"
          style={{
            background: 'color-mix(in srgb, var(--zen-accent) 18%, transparent)',
          }}
        />
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
      {!m.deleted && (
        // The outer box places it (the Thread's chamfer rule makes a cut box
        // position: relative, so the cut is on the inner toolbar).
        <div
          ref={barRef}
          data-home-chat-actions=""
          data-beside={place?.beside === false ? undefined : ''}
          className="pointer-events-none absolute z-[3] opacity-0 transition-opacity duration-100 group-hover:pointer-events-auto group-hover:opacity-100 focus-within:pointer-events-auto focus-within:opacity-100"
          style={barPos}
        >
          <div
            role="toolbar"
            aria-label="Message actions"
            data-chamfer=""
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
      {picker && (
        <HomeChatReactionPicker anchor={reactRef.current ?? ref.current} align={mine ? 'end' : 'start'} onPick={toggle} onClose={() => setPicker(false)} />
      )}
    </div>
  )
})
