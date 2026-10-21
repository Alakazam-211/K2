// @vitest-environment jsdom
// 0.46.3: a settings (action) card the person answered shows what K2 did,
// from the doc's `choice.action.result.message`, as a small muted line under
// the chosen option. Every other card looks exactly as before: the chips
// stay the card's direct children and no result line exists.
import { afterEach, describe, expect, it } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'
import { ChoiceCard } from './ChoiceCard'
import { applyOverlayFrame } from './overlayThread'

afterEach(() => cleanup())

const OPTIONS = [{ label: 'Keep as mine' }, { label: 'Leave it as builder’s' }]
const KEPT = 'Kept as yours. Undo in Settings → Gardens → Local Data.'

describe('ChoiceCard: a settings card’s result line', () => {
  it('an answered card with a result shows it under the chosen option, muted', () => {
    render(<ChoiceCard options={OPTIONS} allowCustom={false} status="answered" answer="Keep as mine" resultMessage={KEPT} onPick={() => undefined} />)
    const line = screen.getByTestId('thread-choice-result')
    expect(line.textContent).toBe(KEPT)
    expect(line.className).toContain('--thread-card-muted')
    // Right under the chosen chip, in the same wrapper.
    const chosen = line.previousElementSibling as HTMLElement
    expect(chosen.getAttribute('data-label')).toBe('Keep as mine')
    expect(chosen.getAttribute('data-selected')).toBe('true')
  })

  it('a card without a result (every card that has no action) looks as before', () => {
    for (const status of ['pending', 'answered', 'voided', 'expired']) {
      cleanup()
      render(
        <ChoiceCard options={OPTIONS} allowCustom={false} status={status} answer={status === 'answered' ? 'Keep as mine' : null} onPick={() => undefined} />,
      )
      expect(screen.queryByTestId('thread-choice-result'), status).toBeNull()
      const card = screen.getByTestId('thread-choice-card')
      const chips = screen.getAllByTestId('thread-choice-chip')
      expect(chips.every((c) => c.parentElement === card), status).toBe(true)
    }
  })

  it('a result never shows on a pending card or under the option not chosen', () => {
    render(<ChoiceCard options={OPTIONS} allowCustom={false} status="pending" resultMessage={KEPT} onPick={() => undefined} />)
    expect(screen.queryByTestId('thread-choice-result')).toBeNull()
  })
})

describe('overlay docs: action.result.message → choice.resultMessage', () => {
  const frame = (choice: Record<string, unknown>) =>
    applyOverlayFrame([], { collection: 'thread', seq: 1, id: 'c1', doc: { id: 'c1', kind: 'choice', from: 'k2', choice: choice as never } }, 1)

  it('a settings card carries the message; a card without an action has no field at all', () => {
    const base = { prompt: 'Keep it?', options: OPTIONS, allow_custom: false, status: 'answered', answer: 'Keep as mine' }
    const kept = frame({ ...base, action: { id: 'zen.widget.keep', params: { widget: 'board' }, result: { kept: true, message: KEPT } } })
    expect(kept[0].doc.choice?.resultMessage).toBe(KEPT)
    const plain = frame(base)
    expect(plain[0].doc.choice).toBeTruthy()
    expect('resultMessage' in (plain[0].doc.choice as object)).toBe(false)
    const pending = frame({ ...base, status: 'pending', action: { id: 'zen.widget.keep', params: {} } })
    expect('resultMessage' in (pending[0].doc.choice as object)).toBe(false)
  })
})
