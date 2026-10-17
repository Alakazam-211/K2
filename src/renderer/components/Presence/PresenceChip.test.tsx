// @vitest-environment jsdom
//
// plan-agent-status-unify-v1 §3.5 (Q8, chamfered square): one person chip
// everywhere; role is a corner mark, never a colour; no amber; rows sit on
// the status cluster's column grid with no overlap and never show you.

import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render } from '@testing-library/react'
import { CHAMFER_CLIP, CHIP_INK_DARK, CHIP_INK_LIGHT, PERSON_COLORS, PresenceChip, PresenceMoreChip, chipInitials, chipInk, personColor, resolvePersonColor } from './PresenceChip'
import { PresenceAvatarCluster } from './PresenceWorkspaceAvatars'

afterEach(cleanup)

function hexHue(hex: string): number {
  const n = parseInt(hex.slice(1), 16)
  const r = ((n >> 16) & 255) / 255
  const g = ((n >> 8) & 255) / 255
  const b = (n & 255) / 255
  const max = Math.max(r, g, b)
  const min = Math.min(r, g, b)
  const d = max - min
  if (d === 0) return -1 // grey
  let h = max === r ? ((g - b) / d) % 6 : max === g ? (b - r) / d + 2 : (r - g) / d + 4
  h *= 60
  return h < 0 ? h + 360 : h
}

describe('person colours', () => {
  it('avoid the status hues (amber/orange 20–70°, green 90–160°, red <15° or >345°)', () => {
    for (const c of PERSON_COLORS) {
      const h = hexHue(c)
      if (h < 0) continue
      expect([c, h >= 20 && h <= 70]).toEqual([c, false])
      expect([c, h >= 90 && h <= 160]).toEqual([c, false])
      expect([c, h < 15 || h > 345]).toEqual([c, false])
    }
  })

  it('are stable per username', () => {
    expect(personColor('anna')).toBe(personColor('anna'))
    expect(PERSON_COLORS).toContain(personColor('rosson'))
  })
})

describe('PresenceChip', () => {
  it('is a chamfered square in the person colour with white initials', () => {
    const { container } = render(<PresenceChip name="anna" role="member" size={14} />)
    const fill = container.querySelector('[data-presence-chip] > span[aria-hidden]') as HTMLElement
    expect(fill.style.clipPath).toBe(CHAMFER_CLIP)
    expect(fill.style.color).toBe('rgb(255, 255, 255)')
    expect(fill.textContent).toBe('A')
  })

  it('one letter below 20px, two at 20px and up', () => {
    expect(chipInitials('Rosson Long', 14)).toBe('R')
    expect(chipInitials('Rosson Long', 20)).toBe('RL')
    expect(chipInitials('owner', 20)).toBe('O')
  })

  it('role is a corner mark: filled owner, hollow admin, none for a member', () => {
    const owner = render(<PresenceChip name="o" role="owner" size={20} />).container.querySelector<HTMLElement>('[data-role-mark]')!
    expect(owner.getAttribute('data-role-mark')).toBe('owner')
    expect(owner.style.background).toBe('var(--color-text-primary)')
    cleanup()
    const admin = render(<PresenceChip name="a" role="admin" size={20} />).container.querySelector<HTMLElement>('[data-role-mark]')!
    expect(admin.style.background).toBe('var(--color-bg-surface)')
    expect(admin.style.border).toContain('var(--color-text-primary)')
    cleanup()
    expect(render(<PresenceChip name="m" role="member" size={20} />).container.querySelector('[data-role-mark]')).toBeNull()
  })

  it('never paints amber (the old owner ring is gone)', () => {
    const { container } = render(<PresenceChip name="owner" role="owner" size={20} you />)
    expect(container.innerHTML.toLowerCase()).not.toContain('#e2a92d')
    expect(container.innerHTML).not.toContain('warn-amber')
    expect(container.innerHTML).not.toContain('needs-you')
  })

  it('you: a ring under the fill that leaves the mark whole; away: greyscale at 45%', () => {
    const you = render(<PresenceChip name="owner" role="owner" size={20} you />).container
    expect(you.querySelector('[data-you-ring]')).not.toBeNull()
    expect(you.querySelector('[data-role-mark]')).not.toBeNull()
    cleanup()
    const away = render(<PresenceChip name="anna" role="member" away />).container
    const fill = away.querySelector('[data-presence-chip] > span[aria-hidden]') as HTMLElement
    expect(fill.style.filter).toBe('grayscale(1)')
    expect(fill.style.opacity).toBe('0.45')
  })

  it('+N is the same shape, neutral', () => {
    const { container } = render(<PresenceMoreChip count={2} size={14} />)
    const fill = container.querySelector('[data-presence-more] > span') as HTMLElement
    expect(fill.style.clipPath).toBe(CHAMFER_CLIP)
    expect(fill.textContent).toBe('+2')
  })
})

describe('row presence grid', () => {
  it('14px chips, the cluster gap, 2px before the last (over the number column), no overlap', () => {
    const users = [
      { user: 'anna', role: 'member' as const },
      { user: 'baden', role: 'admin' as const },
    ]
    const { container } = render(<PresenceAvatarCluster users={users} />)
    const grid = container.querySelector<HTMLElement>('[data-presence-grid]')!
    expect(grid.style.gap).toBe('var(--clu-gap)')
    const cells = [...grid.children] as HTMLElement[]
    expect(cells).toHaveLength(2)
    expect(cells[0].style.marginLeft).toBe('')
    expect(cells[1].style.marginLeft).toBe('calc(var(--num-gap) - var(--clu-gap))')
    for (const c of cells) expect((c.firstElementChild as HTMLElement).style.width).toBe('14px')
  })

  it('more than four people: three chips then +N in the last column', () => {
    const users = ['a', 'b', 'c', 'd', 'e'].map((u) => ({ user: u, role: 'member' as const }))
    const { container } = render(<PresenceAvatarCluster users={users} />)
    expect(container.querySelectorAll('[data-presence-chip]')).toHaveLength(3)
    expect(container.querySelector('[data-presence-more]')?.textContent).toBe('+2')
  })
})

describe('rows never show you', () => {
  it('PresenceWorkspaceAvatars drops the window user from the roster', async () => {
    vi.resetModules()
    vi.doMock('@/stores/presence', async () => {
      const real = await vi.importActual<typeof import('@/stores/presence')>('@/stores/presence')
      const state = {
        supported: true,
        roster: [
          { user: 'owner', role: 'owner', windowCount: 1, workspaces: ['/w'], connectedAt: 1 },
          { user: 'anna', role: 'member', windowCount: 1, workspaces: ['/w'], connectedAt: 1 },
        ],
      }
      return { ...real, usePresenceStore: (sel: (s: typeof state) => unknown) => sel(state) }
    })
    // The window is on this computer (the default): you are `owner`.
    const { default: PresenceWorkspaceAvatars } = await import('./PresenceWorkspaceAvatars')
    const { container } = render(<PresenceWorkspaceAvatars path="/w" />)
    const chips = [...container.querySelectorAll('[data-presence-chip]')].map((c) => c.getAttribute('title'))
    expect(chips).toEqual(['anna · member'])
    vi.doUnmock('@/stores/presence')
  })
})

// 0.46.0 (Rosson 2026-10-17): the daemon serves each person's effective
// colour (chosen in their profile, else the same hash). The chip paints it
// when present and falls back to its own hash for an older daemon.
describe('server-provided person colour', () => {
  function fillColor(container: HTMLElement): string {
    const chip = container.querySelector('[data-presence-chip]') as HTMLElement
    const spans = [...chip.querySelectorAll(':scope > span')] as HTMLElement[]
    return spans[spans.length - 1].style.backgroundColor
  }

  it('uses the server colour when present', () => {
    const { container } = render(<PresenceChip name="owner" role="owner" color="#0d9488" />)
    expect(container.querySelector('[data-presence-chip]')?.getAttribute('data-person-color')).toBe('#0d9488')
    expect(fillColor(container)).toBe('rgb(13, 148, 136)')
  })

  it('an older daemon (no colour) still renders with the hash; owner is violet', () => {
    const { container } = render(<PresenceChip name="owner" role="owner" />)
    expect(container.querySelector('[data-presence-chip]')?.getAttribute('data-person-color')).toBe(personColor('owner'))
    expect(personColor('owner')).toBe('#8c62c4')
    expect(fillColor(container)).toBe('rgb(140, 98, 196)')
  })

  it('the Automatic palette is the refreshed eight, matching the daemon order', () => {
    expect([...PERSON_COLORS]).toEqual(['#3a9e96', '#3a87a8', '#6a6fc8', '#8c62c4', '#b25cb0', '#c45a86', '#9b7a95', '#64778f'])
    // The daemon's hash pins: "" → 0, "a" → 1, "owner" → 3.
    expect(personColor('')).toBe('#3a9e96')
    expect(personColor('a')).toBe('#3a87a8')
  })

  it('accepts any #rrggbb (lowercased); anything else falls back to the hash', () => {
    expect(resolvePersonColor('anna', '#FF0000')).toBe('#ff0000')
    expect(resolvePersonColor('anna', ' #12ab9f ')).toBe('#12ab9f')
    for (const bad of ['#abc', 'red', 'ff0000', '#ff00000', '#gg0000', '']) {
      expect(resolvePersonColor('anna', bad)).toBe(personColor('anna'))
    }
    expect(resolvePersonColor('anna', null)).toBe(personColor('anna'))
  })

  it('initials are white on dark fills and near-black on light ones', () => {
    expect(chipInk('#000000')).toBe(CHIP_INK_LIGHT)
    expect(chipInk('#3a87a8')).toBe(CHIP_INK_LIGHT)
    expect(chipInk('#ffffff')).toBe(CHIP_INK_DARK)
    expect(chipInk('#ffe066')).toBe(CHIP_INK_DARK)
    const { container } = render(<PresenceChip name="anna" role="member" color="#fafafa" />)
    const fill = [...container.querySelectorAll('[data-presence-chip] > span')].pop() as HTMLElement
    expect(fill.style.color).toBe('rgb(17, 20, 24)')
  })

  it('a real name drives the initials and the tooltip', () => {
    const names = { firstName: 'Rosson', lastName: 'Long' }
    expect(chipInitials('z3thon', 20, names)).toBe('RL')
    expect(chipInitials('z3thon', 14, names)).toBe('R')
    expect(chipInitials('z3thon', 20, { firstName: 'Cher' })).toBe('C')
    expect(chipInitials('z3thon', 20, null)).toBe('Z')
    const { container } = render(<PresenceChip name="z3thon" role="admin" names={names} />)
    expect(container.querySelector('[data-presence-chip]')?.getAttribute('title')).toBe('Rosson Long (z3thon) · admin')
    expect(container.querySelector('[data-presence-chip]')?.textContent).toBe('RL')
  })

  it('roster rows and the Home cluster pass the colour through', () => {
    const { container } = render(
      <PresenceAvatarCluster users={[{ user: 'anna', role: 'member', color: '#c026d3' }, { user: 'bo', role: 'member' }]} />,
    )
    const colors = [...container.querySelectorAll('[data-presence-chip]')].map((c) => c.getAttribute('data-person-color'))
    expect(colors).toEqual(['#c026d3', personColor('bo')])
  })
})
