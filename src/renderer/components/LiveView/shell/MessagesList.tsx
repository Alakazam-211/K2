// Messages: the conversation from "Say something", its own section (newest
// first): "You → Atlas: …" and the reply, when, and the chip naming what a
// message pointed at.

import { agoText, personById, type LivePerson } from '@/lib/live-view/shared/people'
import type { LiveMessage } from '@/lib/live-view/shared/source'
import { AtGlyph } from './glyphs'
import { PanelSection } from './PanelSection'

export function MessagesList({ messages, people, now, title = 'Messages' }: { messages: readonly LiveMessage[]; people: readonly LivePerson[]; now: number; title?: string }): React.JSX.Element | null {
  if (messages.length === 0) return null
  const name = (id: string): string => {
    const p = personById(people, id)
    return p?.self ? 'You' : (p?.name ?? id)
  }
  const colour = (id: string): string => {
    const p = personById(people, id)
    return p?.self ? 'var(--color-accent-fill)' : (p?.colour ?? 'var(--color-text-muted)')
  }
  const rows = [...messages].sort((a, b) => b.at_ms - a.at_ms)
  return (
    <PanelSection id="messages" title={title} count={rows.length} countTestId="lv-messages-count">
      <ul className="m-0 list-none p-0">
        {rows.map((m) => (
          <li key={m.id} data-testid="lv-message" className="flex items-start gap-2 px-3.5 py-1.5">
            <span aria-hidden="true" className="lv-change-dot mt-[6px] flex-none" style={{ ['--lv-c' as string]: colour(m.from) }} />
            <span className="flex min-w-0 flex-1 flex-col gap-0.5">
              <span className="type-row text-[var(--color-text-primary)]">
                <b className="font-semibold">
                  {name(m.from)} → {name(m.to)}:
                </b>{' '}
                {m.text}
              </span>
              <span className="type-hint flex items-center gap-2 text-[var(--color-text-secondary)]">
                <span>{agoText(m.at_ms, Math.max(now, m.at_ms))}</span>
                {m.address ? (
                  <span data-k2-chamfer="chip" title={m.address.address} className="inline-flex items-center gap-1 rounded-[var(--radius-pill)] border border-[var(--color-control-edge)] px-1.5 text-[var(--color-text-primary)]">
                    <AtGlyph size={11} />
                    {m.address.label}
                  </span>
                ) : null}
              </span>
            </span>
          </li>
        ))}
      </ul>
    </PanelSection>
  )
}
