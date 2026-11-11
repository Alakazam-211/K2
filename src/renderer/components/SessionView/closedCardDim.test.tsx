// @vitest-environment jsdom
// A dismissed or withdrawn card's question dims with its card in the
// desktop Thread only. Gardens theme themselves: inside a Zen root (the
// Garden's Thread widget) the shared bubble draws it as it always has.
import { afterEach, describe, expect, it } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { ThreadItemRow } from './ThreadOverlayPane'
import { ThreadBubble } from '@/components/Thread/ThreadZenKit'
import { threadItemsFromSnapshot, type OverlayThreadItem } from './overlayThread'

afterEach(() => cleanup())

const QUESTION = 'Ship the dialog polish today?'

function closedCard(): OverlayThreadItem {
  const doc = {
    id: 'tc_closed1',
    kind: 'choice',
    from: 'k2-design',
    body: QUESTION,
    choice: {
      prompt: QUESTION,
      options: [{ label: 'Ship now' }, { label: 'Wait' }],
      allow_custom: false,
      status: 'voided',
      closedAs: 'dismissed',
      closeReason: 'Already fixed on main',
    },
  }
  return threadItemsFromSnapshot({ conversation_id: 'c', items: [{ collection: 'thread', seq: 7, id: doc.id, doc }] }).items[0]
}

/** The opacity on the question's body, or its nearest styled box. */
function questionOpacity(): string {
  let el: HTMLElement | null = screen.getByText(QUESTION)
  while (el && !el.style.opacity && !el.hasAttribute('data-zen-bubble')) el = el.parentElement
  return el?.style.opacity ?? ''
}

describe('closed-card dimming stays out of Gardens', () => {
  it('the desktop Thread dims a closed card’s question', () => {
    render(<ThreadItemRow item={closedCard()} nowSec={1761062000} onAnswer={() => undefined} onVoid={() => undefined} />)
    expect(screen.getByTestId('thread-choice-closed')).toBeTruthy()
    expect(questionOpacity()).toBe('0.6')
  })

  it('inside a Zen root (the Garden’s Thread widget) the question is not dimmed', () => {
    render(
      <div data-zen-root="">
        <ThreadBubble item={closedCard()} mine={false} author="k2-design" timeLabel="now" viewer={null} nowSec={1761062000} />
      </div>,
    )
    expect(screen.getByTestId('thread-choice-closed')).toBeTruthy()
    expect(questionOpacity()).toBe('')
  })

  it('the Garden’s Thread widget never opts in', () => {
    const src = readFileSync(resolve(__dirname, '../Zen/widgets/ZenConversationWidget.tsx'), 'utf8')
    expect(src.includes('dimClosedCards')).toBe(false)
  })
})
