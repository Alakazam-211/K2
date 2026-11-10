// @vitest-environment jsdom
// The link-opening prompt (`k2.open`, K2's card the first time a widget opens
// a site from a Garden) cancels cleanly: the ×, Esc (with focus anywhere) and
// a press outside the card each answer "Don't open". Nothing opens and the
// site is not kept for the Garden ("Always open" is the permission).
// Fixtures are made up (g-test0001, example.test).

import { afterEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { createZenLinks, type ZenLinkAnswer } from '@/lib/zen/zen-links'
import { ZenLinkSheet } from './ZenLinkSheet'

afterEach(cleanup)

/** The real card flow with K2's real card, as ZenCustomFrame mounts it. */
function flow() {
  const openUrl = vi.fn(async (_url: string) => undefined)
  const allow = vi.fn(async (_garden: string, _site: string) => undefined)
  const answers: ZenLinkAnswer[] = []
  let unmount: (() => void) | null = null
  const links = createZenLinks({
    gardenId: () => 'g-test0001',
    sites: async () => [],
    allow,
    ask: (ask) =>
      new Promise<ZenLinkAnswer>((resolve) => {
        const r = render(
          <div data-testid="garden-page">
            <button type="button" data-testid="elsewhere">
              Another widget
            </button>
            <ZenLinkSheet
              ask={ask}
              onDone={(answer) => {
                answers.push(answer)
                resolve(answer)
                unmount?.()
              }}
            />
          </div>,
        )
        unmount = r.unmount
      }),
    describe: () => ({ widget: 'Board', garden: 'Fleet Ops' }),
    openUrl,
  })
  return { links, openUrl, allow, answers }
}

/** Opens the link; the card is up. (Wrapped: an async function returning a
 *  promise would wait for the answer.) */
async function opened(f: ReturnType<typeof flow>): Promise<{ pending: Promise<boolean> }> {
  let pending!: Promise<boolean>
  await act(async () => {
    pending = f.links.open('https://example.test/x', 'https://example.test')
  })
  screen.getByTestId('zen-link-sheet')
  return { pending }
}

function expectCancelled(f: ReturnType<typeof flow>, result: boolean): void {
  expect(result).toBe(false)
  expect(f.answers).toEqual(['deny'])
  expect(f.openUrl).not.toHaveBeenCalled()
  expect(f.allow).not.toHaveBeenCalled()
  expect(screen.queryByTestId('zen-link-sheet')).toBeNull()
}

describe('ZenLinkSheet cancels without opening or keeping anything', () => {
  it('the × (Close (Esc)) answers Don’t open', async () => {
    const f = flow()
    const { pending } = await opened(f)
    const close = screen.getByRole('button', { name: 'Close (Esc)' })
    expect(close.getAttribute('title')).toBe('Close (Esc)')
    fireEvent.click(close)
    // Answered at once (fails here, not on a timeout, when nothing cancels).
    expect(f.answers).toEqual(['deny'])
    expectCancelled(f, await pending)
  })

  it('Esc answers Don’t open with focus outside the card', async () => {
    const f = flow()
    const { pending } = await opened(f)
    ;(document.activeElement as HTMLElement | null)?.blur()
    expect(screen.getByTestId('zen-link-sheet').contains(document.activeElement)).toBe(false)
    fireEvent.keyDown(document.body, { key: 'Escape' })
    // Answered at once (fails here, not on a timeout, when nothing cancels).
    expect(f.answers).toEqual(['deny'])
    expectCancelled(f, await pending)
  })

  it('a press on the dimmed card around the prompt answers Don’t open', async () => {
    const f = flow()
    const { pending } = await opened(f)
    fireEvent.pointerDown(screen.getByTestId('zen-link-sheet'))
    // Answered at once (fails here, not on a timeout, when nothing cancels).
    expect(f.answers).toEqual(['deny'])
    expectCancelled(f, await pending)
  })

  it('a press elsewhere on the page answers Don’t open', async () => {
    const f = flow()
    const { pending } = await opened(f)
    fireEvent.pointerDown(screen.getByTestId('elsewhere'))
    // Answered at once (fails here, not on a timeout, when nothing cancels).
    expect(f.answers).toEqual(['deny'])
    expectCancelled(f, await pending)
  })

  it('a press inside the prompt does not cancel it', async () => {
    const f = flow()
    const { pending } = await opened(f)
    fireEvent.pointerDown(screen.getByTestId('zen-link-url'))
    fireEvent.pointerDown(screen.getByTestId('zen-link-once'))
    expect(f.answers).toEqual([])
    screen.getByTestId('zen-link-sheet')
    fireEvent.click(screen.getByTestId('zen-link-deny'))
    // Answered at once (fails here, not on a timeout, when nothing cancels).
    expect(f.answers).toEqual(['deny'])
    expectCancelled(f, await pending)
  })
})
