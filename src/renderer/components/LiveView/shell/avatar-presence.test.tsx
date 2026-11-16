// @vitest-environment jsdom
// Avatars in the view's presence colours: when the view gives a person a
// presence slot (the scene's marks on the picture, the editors' carets),
// their avatar is that token (`--color-presence-1..4`) with the page's
// background as its ink, so one person reads as one colour in the avatar
// stack and on the file. A view that colours its marks from the data (the
// timeline) keeps the data colour. You stay the accent. Each token reads
// as ink on the background at 4.5:1 or more in every shipped look.

import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { LiveAvatar, PresenceColours } from './LiveAvatar'

afterEach(cleanup)
const avatar = () => screen.getByTestId('lv-avatar')

describe('the avatar’s colour', () => {
  it('is the person’s presence token when the view gives one, with the background as ink', () => {
    render(<LiveAvatar person={{ name: 'Brook', kind: 'agent', colour: '#3fa37a', away: false, presence: 3 }} />)
    expect(avatar().style.backgroundColor).toBe('var(--color-presence-3)')
    expect(avatar().style.color).toBe('var(--color-bg)')
  })

  it('is the token a view maps the person’s colour to (marks drawn elsewhere on the tab)', () => {
    render(
      <PresenceColours.Provider value={new Map([['#3fa37a', 3]])}>
        <LiveAvatar person={{ name: 'Brook', kind: 'agent', colour: '#3FA37A', away: false }} />
      </PresenceColours.Provider>,
    )
    expect(avatar().style.backgroundColor).toBe('var(--color-presence-3)')
  })

  it('stays the data colour where the view gives no slot (the timeline)', () => {
    render(<LiveAvatar person={{ name: 'Brook', kind: 'agent', colour: '#3fa37a', away: false }} />)
    expect(avatar().style.backgroundColor).toBe('rgb(63, 163, 122)')
  })

  it('you stay the accent', () => {
    render(<LiveAvatar person={{ name: 'You', kind: 'person', colour: '#e8872a', away: false, self: true, presence: 4 }} />)
    expect(avatar().style.backgroundColor).toBe('var(--color-accent-fill)')
  })
})

describe('the presence tokens as avatar ink', () => {
  const lum = (h: string): number => {
    const c = [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16) / 255).map((v) => (v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4))
    return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
  }
  const ratio = (a: string, b: string): number => (Math.max(lum(a), lum(b)) + 0.05) / (Math.min(lum(a), lum(b)) + 0.05)
  for (const [style, palette] of [['glass', 'obsidian'], ['glass', 'veil'], ['glass', 'yellow'], ['terminal', 'charcoal'], ['terminal', 'paper'], ['terminal', 'yellow']])
    it(`${style} ${palette}: the background reads on every presence token`, () => {
      const c = JSON.parse(readFileSync(join(__dirname, '..', '..', '..', '..', '..', 'styles', style, 'palettes', `${palette}.json`), 'utf8')).color
      for (const k of [1, 2, 3, 4]) expect(ratio(c[`presence-${k}`].$value, c.bg.$value), `presence-${k}`).toBeGreaterThanOrEqual(4.5)
    })
})
