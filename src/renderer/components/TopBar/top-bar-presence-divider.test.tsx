// @vitest-environment jsdom
//
// Rosson 2026-10-17: with presence on, a divider sits between the people
// chips and the usage chip. Without presence there is no extra divider,
// and two dividers never touch.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'main' }) }))
// Everything but the real PresenceRoster is a stub.
vi.mock('@/components/Timer/TimerButton', () => ({ default: () => <span data-stub="timer" /> }))
vi.mock('@/components/Timer/UsageButton', () => ({ default: () => <span data-stub="usage" /> }))
vi.mock('@/components/Timer/KeepAwakeButton', () => ({ default: () => <span data-stub="keep-awake" /> }))
vi.mock('@/components/Help/HelpButton', () => ({ default: () => <span data-stub="help" /> }))
vi.mock('@/components/Presence/ModeToggle', () => ({ default: () => <span data-stub="mode" /> }))
vi.mock('./AdminProfileButton', () => ({ default: () => <span data-stub="admin-profile" /> }))

import { cleanup, render } from '@testing-library/react'
import TopBarUtilities from './TopBarUtilities'
import { usePresenceStore, type RosterUser } from '@/stores/presence'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

function user(name: string, role: RosterUser['role']): RosterUser {
  return { user: name, role, windowCount: 1, workspaces: [], connectedAt: 1 }
}

/** The cluster's items, in order, by what they are. */
function clusterItems(): string[] {
  const cluster = document.querySelector('.no-drag.flex.items-center.gap-1')
  if (!cluster) throw new Error('no top-bar cluster')
  return Array.from(cluster.children).map((c) => {
    if (c.hasAttribute('data-top-bar-pipe')) return 'pipe'
    if (c.getAttribute('data-testid') === 'presence-roster') return 'presence'
    if (c.hasAttribute('data-stub')) return c.getAttribute('data-stub') ?? '?'
    if (c.hasAttribute('data-drawer')) return 'drawer'
    if (c.hasAttribute('data-zen-enter')) return 'zen'
    return `other:${c.tagName}`
  })
}

function expectNoAdjacentPipes(items: string[]): void {
  const touching = items.filter((it, i) => it === 'pipe' && items[i + 1] === 'pipe')
  expect(touching).toEqual([])
}

const TWO_PEOPLE = [user('owner', 'owner'), user('appa', 'member')]

beforeEach(() => {
  Object.defineProperty(window.navigator, 'platform', { value: 'MacIntel', configurable: true })
})

afterEach(() => {
  cleanup()
  usePresenceStore.setState({ roster: [], supported: true })
})

describe('the presence divider in the top bar', () => {
  it('with presence on: a pipe sits between the people and the usage chip', () => {
    usePresenceStore.setState({ roster: TWO_PEOPLE, supported: true })
    render(<TopBarUtilities />)
    const items = clusterItems()
    expect(items.slice(0, 3)).toEqual(['presence', 'pipe', 'usage'])
    expectNoAdjacentPipes(items)
  })

  it('the presence pipe is the same element as the other top-bar dividers', () => {
    usePresenceStore.setState({ roster: TWO_PEOPLE, supported: true })
    render(<TopBarUtilities />)
    const pipe = document.querySelector('[data-testid="presence-roster"]')?.nextElementSibling
    if (!pipe) throw new Error('nothing after the roster')
    expect(pipe.hasAttribute('data-top-bar-pipe')).toBe(true)
    expect(pipe.className).toBe('block w-px h-4 bg-[var(--color-border)] mx-1')
  })

  it('just you: no roster and no extra pipe before the usage chip', () => {
    usePresenceStore.setState({ roster: [user('owner', 'owner')], supported: true })
    render(<TopBarUtilities />)
    const items = clusterItems()
    expect(items).not.toContain('presence')
    expect(items[0]).toBe('usage')
    expectNoAdjacentPipes(items)
  })

  it('an older daemon without presence: no roster and no extra pipe', () => {
    usePresenceStore.setState({ roster: TWO_PEOPLE, supported: false })
    render(
      <TopBarUtilities>
        <button data-drawer="left" />
      </TopBarUtilities>,
    )
    const items = clusterItems()
    expect(items).not.toContain('presence')
    expect(items[0]).toBe('usage')
    expectNoAdjacentPipes(items)
  })

  it('with presence and drawer toggles, no two pipes touch anywhere', () => {
    usePresenceStore.setState({ roster: TWO_PEOPLE, supported: true })
    render(
      <TopBarUtilities>
        <button data-drawer="left" />
        <button data-drawer="right" />
      </TopBarUtilities>,
    )
    const items = clusterItems()
    expect(items.filter((i) => i === 'pipe')).toHaveLength(5)
    expectNoAdjacentPipes(items)
  })
})
