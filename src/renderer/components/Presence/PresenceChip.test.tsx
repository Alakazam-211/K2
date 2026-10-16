// @vitest-environment jsdom
//
// plan-agent-status-unify-v1 §3.5 (Q8, chamfered square): one person chip
// everywhere; role is a corner mark, never a colour; no amber; rows sit on
// the status cluster's column grid with no overlap and never show you.

import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render } from '@testing-library/react'
import { CHAMFER_CLIP, PERSON_COLORS, PresenceChip, PresenceMoreChip, chipInitials, personColor } from './PresenceChip'
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
