// Who is typing in a Home chat (prd-home-people-channels-v1 §9), in the
// message box's drawer slot (where the Thread shows agent activity).
//
// A quiet line on the box's top edge: the names, then three dots that
// breathe (the Thread's `data-zen-typing` motion; still under reduced
// motion). It never moves the box or the list: the slot is always there and
// has no height; the line hangs above the box, out of the flow, and fades
// in and out. A small canvas-coloured backing keeps it readable over the
// messages scrolling under it. The words are one polite live region, so a
// screen reader hears "Ana is typing" once, not every dot.

import type { CSSProperties, JSX } from 'react'
import { typingLine } from '@/lib/home-chat/chat'

/** "Ana is typing", "Ana and Ben are typing", "Ana and 2 others are
 *  typing": the line without its ellipsis (the dots draw it). */
export function typingWords(names: readonly string[]): string {
  return typingLine(names).replace(/…$/, '')
}

const SLOT: CSSProperties = { position: 'relative', height: 0, zIndex: 1 }
const LINE: CSSProperties = {
  position: 'absolute',
  bottom: 4,
  left: 8,
  maxWidth: 'calc(100% - 16px)',
  display: 'inline-flex',
  alignItems: 'center',
  gap: 6,
  padding: '1px 8px',
  fontSize: 'var(--zen-type-sm, 0.8em)',
  lineHeight: '16px',
  color: 'var(--zen-text-muted)',
  background: 'var(--zen-canvas)',
  whiteSpace: 'nowrap',
  pointerEvents: 'none',
  transition: 'opacity 160ms ease',
}
const DOT: CSSProperties = { width: 4, height: 4, background: 'currentColor', display: 'inline-block' }

export function HomeChatTyping({ names }: { names: readonly string[] }): JSX.Element {
  const on = names.length > 0
  return (
    <div style={SLOT} data-home-chat-typing-slot="">
      <div
        data-testid="home-chat-typing"
        data-on={on ? '' : undefined}
        aria-live="polite"
        className="rounded-[calc(var(--radius-box)/2)]"
        style={{ ...LINE, opacity: on ? 1 : 0 }}
      >
        <span className="min-w-0 truncate" data-home-chat-typing-words="">
          {on ? typingWords(names) : ''}
        </span>
        {on && (
          <span data-zen-typing="" aria-hidden="true" className="inline-flex flex-shrink-0 items-center" style={{ gap: 3 }}>
            <span className="rounded-[var(--radius-badge)]" style={DOT} />
            <span className="rounded-[var(--radius-badge)]" style={DOT} />
            <span className="rounded-[var(--radius-badge)]" style={DOT} />
          </span>
        )}
      </div>
    </div>
  )
}
