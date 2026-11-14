// One message in a Home chat (prd-home-people-channels-v1 §8–§11): the
// Thread's bubble (`ThreadChatBubble`), the reply's quote strip inside it
// (click: jump to the original), reaction chips under it, and the hover row
// (React, Reply, ⋯). A long press on touch, or a right-click, opens the same
// three as a menu (`useThreadMessageGestures`, the Thread's). No @mentions:
// `@name` is plain text in the body (D18).
//
// The row's own element carries `data-zen-message` so the render window
// (`useRenderWindow`) counts and measures it like a Thread row.

import { memo, useCallback, useMemo, useRef, useState, type JSX } from 'react'
import { ThreadChatBubble } from '@/components/Thread/ThreadChat'
import { ReplyGlyph, ThreadReplyQuote, useThreadMessageGestures } from '@/components/Thread/ThreadMessageMenu'
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

const ACTION_BUTTON = {
  width: 26,
  height: 24,
  display: 'inline-flex',
  alignItems: 'center',
  justifyContent: 'center',
  background: 'none',
  border: 'none',
  cursor: 'pointer',
  color: 'var(--zen-text-muted)',
  fontSize: 13,
} as const

const DELETED_STYLE = { fontSize: '0.8em', fontStyle: 'italic', color: 'var(--zen-text-muted)', padding: '4px 8px' } as const

function quoteText(p: ReplyPreview): { from: string; line: string } {
  if (p.state === 'loaded') return { from: p.author, line: p.line || '(no text)' }
  if (p.state === 'deleted') return { from: 'Reply', line: 'Original message deleted' }
  return { from: 'Reply', line: 'Original not loaded: click to find it' }
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
  /** Just jumped to: flashed for a moment. */
  flash: boolean
  /** No react or reply (an archived channel, a DM with a disabled login). */
  readOnly: boolean
  actions: HomeChatRowActions
}): JSX.Element {
  const [picker, setPicker] = useState(false)
  const ref = useRef<HTMLDivElement | null>(null)
  const replyTo = m.replyTo
  const replyState = reply?.state
  const replyFrom = reply ? quoteText(reply).from : ''
  const replyLine = reply ? quoteText(reply).line : ''
  const quote = useMemo(
    () =>
      replyTo && replyState ? (
        <ThreadReplyQuote
          reply={{ part: false, seq: 0, from: replyFrom, id: replyTo, quote: replyLine, rest: '' }}
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

  return (
    <div
      ref={ref}
      data-zen-message={m.id}
      data-home-chat-message=""
      data-mine={mine ? '' : undefined}
      data-flash={flash ? '' : undefined}
      className="group relative flex flex-col rounded-[calc(var(--radius-box)/2)]"
      style={{
        alignItems: mine ? 'flex-end' : 'flex-start',
        minWidth: 0,
        transition: 'background-color 600ms',
        background: flash ? 'color-mix(in srgb, var(--zen-accent) 14%, transparent)' : undefined,
      }}
      {...(m.deleted ? {} : gestures)}
    >
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
        <div
          data-home-chat-actions=""
          className="pointer-events-none absolute flex rounded-[calc(var(--radius-box)/2)] opacity-0 group-hover:pointer-events-auto group-hover:opacity-100 focus-within:pointer-events-auto focus-within:opacity-100"
          style={{
            top: -10,
            [mine ? 'left' : 'right']: 0,
            gap: 0,
            border: '1px solid var(--zen-border)',
            background: 'var(--zen-surface-raised, var(--color-bg-elevated))',
          }}
        >
          {!readOnly && (
            <button type="button" aria-label="React" title="React" data-testid="home-chat-react" style={ACTION_BUTTON} onClick={() => setPicker(true)}>
              ☺
            </button>
          )}
          {!readOnly && (
            <button type="button" aria-label="Reply" title="Reply" data-testid="home-chat-reply" style={ACTION_BUTTON} onClick={() => actions.reply(m)}>
              <ReplyGlyph size={13} />
            </button>
          )}
          <button
            type="button"
            aria-label="More"
            title="More"
            data-testid="home-chat-more"
            style={ACTION_BUTTON}
            onClick={(e) => {
              const r = e.currentTarget.getBoundingClientRect()
              openMenu(r.left, r.bottom)
            }}
          >
            ⋯
          </button>
        </div>
      )}
      {picker && (
        <div className="relative" style={{ alignSelf: mine ? 'flex-end' : 'flex-start' }}>
          <HomeChatReactionPicker onPick={toggle} onClose={() => setPicker(false)} />
        </div>
      )}
    </div>
  )
})
