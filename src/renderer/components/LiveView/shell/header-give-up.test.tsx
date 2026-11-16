// @vitest-environment jsdom

// The live header never runs past the tab (editors-notes.md v2, "The narrow
// file bar never wraps"), and its sentence goes last: while the row would
// overflow, or the sentence wouldn't fit whole, it gives things up one at a
// time, in order: the file name, the avatar stack, the words of Follow and of
// the buttons after the sentence, the file's tile, the sentence's long form
// (its short form stays: "Atlas finished."), the buttons' short words. jsdom
// has no layout, so the widths are scripted.

import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'
import { GIVE_UP, LiveHeader } from './LiveHeader'

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

function widths(row: { natural: number; width: number; step: number }): void {
  vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockImplementation(function (this: HTMLElement) {
    return this.dataset.testid === 'lv-shell-header' ? row.width : 0
  })
  vi.spyOn(HTMLElement.prototype, 'scrollWidth', 'get').mockImplementation(function (this: HTMLElement) {
    return this.dataset.testid === 'lv-shell-header' ? row.natural - Number(this.dataset.tight ?? 0) * row.step : 0
  })
}

const header = (fitKey = 'a') => <LiveHeader fileName="README.md" fileGlyph={<span />} sentence={null} fitKey={fitKey} right={<span data-testid="lv-stack" />} />

describe('the header gives things up in order', () => {
  it('fits whole: gives up nothing', () => {
    widths({ natural: 500, width: 600, step: 50 })
    render(header())
    expect(screen.getByTestId('lv-shell-header').hasAttribute('data-tight')).toBe(false)
  })

  it('steps until it fits: the name, then the stack', () => {
    widths({ natural: 700, width: 600, step: 60 })
    render(header())
    expect(screen.getByTestId('lv-shell-header').dataset.tight).toBe('2')
  })

  it('stops after the last step, whatever is left', () => {
    widths({ natural: 2000, width: 600, step: 10 })
    render(header())
    expect(screen.getByTestId('lv-shell-header').dataset.tight).toBe('7')
  })

  it('keeps the sentence: the buttons give up their words first, then it takes its short form', () => {
    // The row fits; the sentence box holds 160px; the sentence is 10px a character.
    vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockImplementation(function (this: HTMLElement) {
      return this.dataset.testid === 'lv-shell-header' ? 600 : this.classList.contains('lv-sentence-box') ? 160 : 0
    })
    vi.spyOn(HTMLElement.prototype, 'scrollWidth', 'get').mockImplementation(function (this: HTMLElement) {
      if (this.dataset.testid === 'lv-shell-header') return 500
      return this.parentElement?.classList.contains('lv-sentence-box') ? (this.textContent ?? '').length * 10 : 0
    })
    const ends: number[] = []
    render(
      <LiveHeader
        fileName="README.md"
        sentence={{ who: { id: 'atlas', name: 'Atlas', kind: 'agent', colour: '#7c3aed', away: false }, text: 'finished. Stopped following.', short: 'finished.' }}
        sentenceEnd={(t) => {
          ends.push(t)
          return <span data-testid="end">{t >= GIVE_UP.words ? 'Back' : 'Back to my place'}</span>
        }}
      />,
    )
    expect(screen.getByTestId('lv-sentence').textContent).toBe('Atlas finished.')
    expect(screen.getByTestId('lv-shell-header').dataset.tight).toBe(String(GIVE_UP.sentence))
    expect(screen.getByTestId('end').textContent).toBe('Back')
    expect(ends[0]).toBe(0)
  })

  it('tries the whole row again when its controls change', () => {
    const row = { natural: 700, width: 600, step: 60 }
    widths(row)
    const r = render(header('a'))
    expect(screen.getByTestId('lv-shell-header').dataset.tight).toBe('2')
    row.natural = 500
    r.rerender(header('b'))
    expect(screen.getByTestId('lv-shell-header').hasAttribute('data-tight')).toBe(false)
  })
})
