// Reactions on a Home chat bubble (prd-home-people-channels-v1 §10, D7):
//   - chips under the bubble: emoji and count, lit when you reacted; a tap
//     toggles yours; hovering (a long press on touch) says who;
//   - the picker: six quick emoji plus search over the Unicode set.
// Look: the Thread kit's tokens (`--zen-*`); K2 Design owns the final look.

import { useEffect, useMemo, useRef, useState, type JSX } from 'react'
import type { HomeChatReaction } from '@/lib/home-chat/api'
import { QUICK_REACTIONS, searchEmoji } from '@/lib/home-chat/chat'

const CHIP_STYLE = {
  display: 'inline-flex',
  alignItems: 'center',
  gap: 4,
  padding: '1px 8px',
  borderRadius: 999,
  fontSize: '0.8em',
  lineHeight: 1.6,
  cursor: 'pointer',
} as const

/** The chips under one bubble. `whoFor` answers the names for a chip's
 *  tooltip (read on hover); `onToggle` flips the viewer's own reaction. */
export function HomeChatReactionChips({
  reactions,
  mine,
  onToggle,
  whoFor,
  disabled = false,
}: {
  reactions: readonly HomeChatReaction[]
  /** The bubble is the viewer's (chips line up on its side). */
  mine: boolean
  onToggle: (emoji: string) => void
  whoFor: (emoji: string) => Promise<string[]>
  disabled?: boolean
}): JSX.Element | null {
  const [who, setWho] = useState<Record<string, string>>({})
  if (reactions.length === 0) return null
  const hover = (emoji: string): void => {
    void whoFor(emoji).then(
      (names) => setWho((w) => ({ ...w, [emoji]: names.length > 0 ? names.join(', ') : '' })),
      () => undefined,
    )
  }
  return (
    <div
      data-testid="home-chat-reactions"
      className="flex flex-wrap"
      style={{ gap: 4, alignSelf: mine ? 'flex-end' : 'flex-start', marginTop: 2, padding: '0 4px' }}
    >
      {reactions.map((r) => (
        <button
          key={r.emoji}
          type="button"
          data-testid="home-chat-reaction"
          data-emoji={r.emoji}
          data-me={r.me ? '' : undefined}
          aria-pressed={r.me}
          aria-label={`${r.emoji} ${r.count}${r.me ? ', you reacted' : ''}`}
          title={who[r.emoji] ?? ''}
          disabled={disabled}
          onMouseEnter={() => hover(r.emoji)}
          onFocus={() => hover(r.emoji)}
          onClick={() => onToggle(r.emoji)}
          style={{
            ...CHIP_STYLE,
            border: `1px solid ${r.me ? 'var(--zen-accent)' : 'var(--zen-border)'}`,
            background: r.me ? 'color-mix(in srgb, var(--zen-accent) 18%, transparent)' : 'var(--zen-surface-raised)',
            color: 'var(--zen-text)',
          }}
        >
          <span aria-hidden="true">{r.emoji}</span>
          <span data-reaction-count="">{r.count}</span>
        </button>
      ))}
    </div>
  )
}

/** The picker: the quick row, then a search box over the Unicode set. */
export function HomeChatReactionPicker({
  onPick,
  onClose,
}: {
  onPick: (emoji: string) => void
  onClose: () => void
}): JSX.Element {
  const [q, setQ] = useState('')
  const found = useMemo(() => (q.trim() ? searchEmoji(q) : []), [q])
  const ref = useRef<HTMLDivElement | null>(null)
  useEffect(() => {
    const onDown = (e: MouseEvent): void => {
      if (ref.current && e.target instanceof Node && !ref.current.contains(e.target)) onClose()
    }
    document.addEventListener('mousedown', onDown)
    return () => document.removeEventListener('mousedown', onDown)
  }, [onClose])
  const pick = (e: string): void => {
    onPick(e)
    onClose()
  }
  const cell = { width: 28, height: 28, fontSize: 18, borderRadius: 6, cursor: 'pointer', background: 'none', border: 'none' } as const
  return (
    <div
      ref={ref}
      role="dialog"
      aria-label="Add a reaction"
      data-testid="home-chat-reaction-picker"
      onKeyDown={(e) => {
        if (e.key === 'Escape') {
          e.stopPropagation()
          onClose()
        }
      }}
      style={{
        position: 'absolute',
        zIndex: 30,
        bottom: '100%',
        marginBottom: 4,
        padding: 6,
        minWidth: 220,
        borderRadius: 10,
        border: '1px solid var(--zen-border)',
        background: 'var(--zen-surface-raised, var(--color-bg-elevated))',
        boxShadow: '0 6px 20px rgba(0,0,0,0.25)',
      }}
    >
      <div className="flex" style={{ gap: 2 }} data-testid="home-chat-quick-reactions">
        {QUICK_REACTIONS.map((e) => (
          <button key={e} type="button" aria-label={`React ${e}`} data-emoji={e} style={cell} onClick={() => pick(e)}>
            {e}
          </button>
        ))}
      </div>
      <input
        autoFocus
        value={q}
        onChange={(e) => setQ(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && found[0]) {
            e.preventDefault()
            pick(found[0])
          }
        }}
        placeholder="Search emoji"
        aria-label="Search emoji"
        data-testid="home-chat-emoji-search"
        className="w-full bg-transparent focus:outline-none"
        style={{ marginTop: 6, padding: '4px 6px', fontSize: 12, border: '1px solid var(--zen-border)', borderRadius: 6, color: 'var(--zen-text)' }}
      />
      {q.trim() !== '' && (
        <div className="flex flex-wrap" style={{ gap: 2, marginTop: 4, maxHeight: 140, overflowY: 'auto' }} data-testid="home-chat-emoji-results">
          {found.length === 0 ? (
            <span style={{ fontSize: 11, color: 'var(--zen-text-muted)', padding: 4 }}>No emoji match</span>
          ) : (
            found.map((e) => (
              <button key={e} type="button" aria-label={`React ${e}`} data-emoji={e} style={cell} onClick={() => pick(e)}>
                {e}
              </button>
            ))
          )}
        </div>
      )}
    </div>
  )
}
