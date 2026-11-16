// @vitest-environment jsdom

// The live header never runs past the tab (editors-notes.md v2, "The narrow
// file bar never wraps"): while the row would overflow it gives things up
// one at a time, in order: the file name, the avatar stack, Follow's words
// (its dot and "Stop" stay), the file's tile. jsdom has no layout, so the
// row's widths are scripted: each step frees `step` px.

import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'
import { LiveHeader } from './LiveHeader'

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

  it('stops after the file tile, whatever is left', () => {
    widths({ natural: 2000, width: 600, step: 10 })
    render(header())
    expect(screen.getByTestId('lv-shell-header').dataset.tight).toBe('4')
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
