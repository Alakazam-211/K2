// @vitest-environment jsdom
// prd-popup-close-and-card-dismiss-v1 B8 (0.46.11): a person dismisses a
// waiting card with an optional one-line reason; the closed card stays in
// the history as Dismissed or Withdrawn (never hidden like a daemon close).
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { ChoiceCard } from './ChoiceCard'
import { isVoidedHitl, type OverlayDoc } from './overlayThread'

afterEach(() => cleanup())

const OPTIONS = [{ label: 'Ship' }, { label: 'Wait' }]

describe('Dismiss on a waiting choice card', () => {
  it('sends the reason, and nothing is picked', async () => {
    const onDismiss = vi.fn(async (_reason?: string) => undefined)
    const onPick = vi.fn()
    render(<ChoiceCard options={OPTIONS} allowCustom={false} status="pending" onPick={onPick} onDismiss={onDismiss} />)
    fireEvent.click(screen.getByTestId('thread-card-dismiss'))
    fireEvent.change(screen.getByTestId('thread-card-dismiss-reason'), { target: { value: 'Shipped yesterday' } })
    fireEvent.click(screen.getByTestId('thread-card-dismiss-confirm'))
    await waitFor(() => expect(onDismiss).toHaveBeenCalledTimes(1))
    expect(onDismiss).toHaveBeenCalledWith('Shipped yesterday')
    expect(onPick).not.toHaveBeenCalled()
  })

  it('Keep and Esc close the form without dismissing', () => {
    const onDismiss = vi.fn()
    render(<ChoiceCard options={OPTIONS} allowCustom={false} status="pending" onPick={() => undefined} onDismiss={onDismiss} />)
    fireEvent.click(screen.getByTestId('thread-card-dismiss'))
    fireEvent.click(screen.getByTestId('thread-card-dismiss-keep'))
    expect(screen.queryByTestId('thread-card-dismiss-form')).toBeNull()
    fireEvent.click(screen.getByTestId('thread-card-dismiss'))
    fireEvent.keyDown(screen.getByTestId('thread-card-dismiss-reason'), { key: 'Escape' })
    expect(screen.queryByTestId('thread-card-dismiss-form')).toBeNull()
    expect(onDismiss).not.toHaveBeenCalled()
  })

  it("shows the daemon's refusal on the card", async () => {
    const onDismiss = vi.fn(async () => {
      throw Object.assign(new Error('403'), { code: 'card_not_yours', hint: 'an agent withdraws only the cards it asked itself' })
    })
    render(<ChoiceCard options={OPTIONS} allowCustom={false} status="pending" onPick={() => undefined} onDismiss={onDismiss} />)
    fireEvent.click(screen.getByTestId('thread-card-dismiss'))
    fireEvent.click(screen.getByTestId('thread-card-dismiss-confirm'))
    const err = await screen.findByTestId('thread-card-dismiss-error')
    expect(err.textContent?.trim().length).toBeGreaterThan(0)
  })

  it('a closed card has no Dismiss', () => {
    render(<ChoiceCard options={OPTIONS} allowCustom={false} status="answered" answer="Ship" onPick={() => undefined} onDismiss={() => undefined} />)
    expect(screen.queryByTestId('thread-card-dismiss')).toBeNull()
  })
})

describe('a dismissed or withdrawn card stays in the history', () => {
  it('draws who and why', () => {
    render(
      <ChoiceCard
        options={OPTIONS}
        allowCustom={false}
        status="voided"
        answeredBy="You"
        closedAs="dismissed"
        closeReason="Shipped yesterday"
        from="sales"
        onPick={() => undefined}
      />,
    )
    const line = screen.getByTestId('thread-choice-closed')
    expect(line.textContent).toBe('Dismissed by You: Shipped yesterday')
    cleanup()
    render(<ChoiceCard options={OPTIONS} allowCustom={false} status="voided" closedAs="withdrawn" from="sales" onPick={() => undefined} />)
    expect(screen.getByTestId('thread-choice-closed').textContent).toBe('Withdrawn by sales')
  })

  it('only a daemon close (no closedAs) is hidden', () => {
    const card = (closedAs?: 'dismissed' | 'withdrawn'): OverlayDoc =>
      ({
        id: 'c1',
        kind: 'choice',
        from: 'sales',
        choice: { prompt: 'Ship?', options: OPTIONS, allow_custom: false, status: 'voided', ...(closedAs ? { closedAs } : {}) },
      }) as OverlayDoc
    expect(isVoidedHitl(card())).toBe(true)
    expect(isVoidedHitl(card('dismissed'))).toBe(false)
    expect(isVoidedHitl(card('withdrawn'))).toBe(false)
    const secret = { id: 's1', kind: 'secret', from: 'sales', secret: { name: 'KEY', status: 'voided', closedAs: 'dismissed' } } as OverlayDoc
    expect(isVoidedHitl(secret)).toBe(false)
  })
})
