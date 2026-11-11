// @vitest-environment jsdom
// The tile bar's layout: an agent tile's view picker sits in the centre
// column of three (name | picker | controls); a pane without one keeps the
// plain row.
import { afterEach, describe, expect, it } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'
import { PaneChrome } from './PaneChrome'

afterEach(() => cleanup())

const bar = (): HTMLElement => document.querySelector('[data-tile-bar]') as HTMLElement

describe('PaneChrome', () => {
  it('centres an agent tile’s picker: three columns, name | picker | controls', () => {
    render(
      <PaneChrome
        title="sales"
        shortcutNum={2}
        readOnly={false}
        focused={false}
        onClose={() => {}}
        lead={<span data-testid="lead" />}
        picker={<div data-testid="picker" />}
        tileView="thread"
      >
        <div />
      </PaneChrome>,
    )
    const b = bar()
    expect(b.hasAttribute('data-tile-centred')).toBe(true)
    // Equal side columns round an auto centre; the controls' column never
    // shrinks below its content, the name's gives way first.
    expect(b.className).toContain('grid-cols-[minmax(0,1fr)_auto_minmax(max-content,1fr)]')
    const cells = Array.from(b.children) as HTMLElement[]
    expect(cells.map((c) => Object.keys(c.dataset)[0])).toEqual(['tileStart', 'tilePicker', 'tileEnd'])
    expect(cells[0].contains(screen.getByTestId('lead'))).toBe(true)
    expect(cells[0].textContent).toContain('sales')
    expect(cells[0].className).toContain('min-w-0')
    expect(cells[1].contains(screen.getByTestId('picker'))).toBe(true)
    expect(cells[1].className).toContain('justify-self-center')
    expect(cells[2].contains(screen.getByRole('button', { name: 'Close pane' }))).toBe(true)
    expect(cells[2].querySelector('[data-tile-shortcut]')).not.toBeNull()
  })

  it('keeps the plain row for a pane with no picker', () => {
    render(
      <PaneChrome title="notes.md" readOnly={false} focused={false} onClose={() => {}}>
        <div />
      </PaneChrome>,
    )
    const b = bar()
    expect(b.hasAttribute('data-tile-centred')).toBe(false)
    expect(b.className).toContain('flex')
    expect(b.querySelector('[data-tile-spacer]')).not.toBeNull()
    expect(b.querySelector('[data-tile-picker]')).toBeNull()
  })
})
