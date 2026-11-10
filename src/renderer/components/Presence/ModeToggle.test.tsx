// @vitest-environment jsdom
//
// 0.46.11 (Rosson, thread-first): the eye/pencil shows only when it
// matters — someone else is on this session, or this window is on the eye
// (so nobody is stranded in view mode after the others leave). Alone on
// the pencil, there is no switch. The rule lives inside `ModeToggle`, so
// any bar that drops `<ModeToggle />` in follows it.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render } from '@testing-library/react'
import { createStore } from 'zustand/vanilla'

vi.mock('@/components/Presence/PresenceKickButton', () => ({
  fetchViewerRole: vi.fn(async () => null),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async () => ({ roster: [] })),
  daemonCliPost: vi.fn(async () => ({})),
}))
vi.mock('@/stores/connect-host', () => ({
  useConnectHostStore: Object.assign(
    (sel: (s: { activeHost: 'local' }) => unknown) => sel({ activeHost: 'local' }),
    { getState: () => ({ activeHost: 'local' as const, hosts: [] }) },
  ),
  activeHostKey: () => 'local',
  onActiveHostChange: () => () => {},
}))

import ModeToggle, {
  ModeToggleSessionContext,
  othersOnSessionPath,
  shouldShowModeToggle,
  type ModeToggleSession,
} from './ModeToggle'
import { usePresenceStore, type RosterUser } from '@/stores/presence'
import { useWindowModeStore, __resetWindowModeForTests } from '@/stores/window-mode'
import type { PresenceView } from '@/stores/server-view'

const WS = '/Users/me/work/demo'

function person(user: string, workspaces: string[] = [WS]): RosterUser {
  return { user, role: user === 'owner' ? 'owner' : 'member', windowCount: 1, workspaces, connectedAt: 1 }
}

const primary: ModeToggleSession = {
  path: WS,
  room: {
    isPrimary: true,
    readOnly: false,
    presence: usePresenceStore,
    scope: { label: 'This computer' } as never,
  },
}

function renderIn(session: ModeToggleSession | null, count = 1): ReturnType<typeof render> {
  const bars = Array.from({ length: count }, (_, i) => (
    <div key={i} data-bar={i}>
      <ModeToggle />
    </div>
  ))
  return render(
    session ? <ModeToggleSessionContext.Provider value={session}>{bars}</ModeToggleSessionContext.Provider> : <>{bars}</>,
  )
}

const toggles = (): Element[] => Array.from(document.querySelectorAll('[data-mode-toggle]'))

beforeEach(() => {
  __resetWindowModeForTests()
  useWindowModeStore.setState({ mode: 'claimer', resolved: true, capable: true })
  usePresenceStore.setState({ roster: [person('owner')], supported: true })
})
afterEach(() => cleanup())

describe('shouldShowModeToggle (the rule)', () => {
  it('alone on the pencil: hidden', () => {
    expect(shouldShowModeToggle({ othersOnSession: false, mode: 'claimer' })).toBe(false)
  })
  it('someone else on the session: shown, on either mode', () => {
    expect(shouldShowModeToggle({ othersOnSession: true, mode: 'claimer' })).toBe(true)
    expect(shouldShowModeToggle({ othersOnSession: true, mode: 'viewer' })).toBe(true)
  })
  it('alone on the eye: shown, so nobody is stuck in view mode', () => {
    expect(shouldShowModeToggle({ othersOnSession: false, mode: 'viewer' })).toBe(true)
  })
  it('unresolved and alone: hidden', () => {
    expect(shouldShowModeToggle({ othersOnSession: false, mode: null })).toBe(false)
  })
})

describe('othersOnSessionPath', () => {
  it('counts people on this workspace (or a worktree under it), never you', () => {
    expect(othersOnSessionPath([person('owner')], WS, 'owner', true)).toBe(false)
    expect(othersOnSessionPath([person('owner'), person('alice')], WS, 'owner', true)).toBe(true)
    expect(othersOnSessionPath([person('alice', [`${WS}/.worktrees/x`])], WS, 'owner', true)).toBe(true)
    expect(othersOnSessionPath([person('alice', ['/Users/me/work/other'])], WS, 'owner', true)).toBe(false)
  })
  it('no path or no presence: nobody', () => {
    expect(othersOnSessionPath([person('alice')], null, 'owner', true)).toBe(false)
    expect(othersOnSessionPath([person('alice')], WS, 'owner', false)).toBe(false)
  })
})

describe('<ModeToggle /> in a session bar', () => {
  it('alone on the pencil: no switch at all', () => {
    renderIn(primary)
    expect(toggles()).toHaveLength(0)
  })

  it('another person attached to this session: shown, pencil lit', () => {
    usePresenceStore.setState({ roster: [person('owner'), person('alice')], supported: true })
    renderIn(primary)
    const [t] = toggles()
    expect(t?.getAttribute('data-mode-toggle')).toBe('claimer')
    expect(t?.querySelector('button[aria-pressed="true"]')?.getAttribute('title')).toBe('Edit — type and resize')
  })

  it('someone on a different workspace does not count', () => {
    usePresenceStore.setState({ roster: [person('owner'), person('alice', ['/elsewhere'])], supported: true })
    renderIn(primary)
    expect(toggles()).toHaveLength(0)
  })

  it('alone on the eye: shown, so the way back to the pencil is there', () => {
    useWindowModeStore.setState({ mode: 'viewer', resolved: true })
    renderIn(primary)
    expect(toggles()[0]?.getAttribute('data-mode-toggle')).toBe('viewer')
  })

  it('the other person leaves while on the pencil: the switch goes', () => {
    usePresenceStore.setState({ roster: [person('owner'), person('alice')], supported: true })
    renderIn(primary)
    expect(toggles()).toHaveLength(1)
    act(() => usePresenceStore.setState({ roster: [person('owner')] }))
    expect(toggles()).toHaveLength(0)
  })

  it('the other person leaves while on the eye: it stays until you are back on the pencil', () => {
    usePresenceStore.setState({ roster: [person('owner'), person('alice')], supported: true })
    renderIn(primary)
    fireEvent.click(toggles()[0].querySelector('button[title^="View only"]') as HTMLElement)
    expect(useWindowModeStore.getState().mode).toBe('viewer')
    act(() => usePresenceStore.setState({ roster: [person('owner')] }))
    expect(toggles()).toHaveLength(1)
    fireEvent.click(toggles()[0].querySelector('button[title^="Edit"]') as HTMLElement)
    expect(useWindowModeStore.getState().mode).toBe('claimer')
    expect(toggles()).toHaveLength(0)
  })

  it('several bars on screen show the same window state, and either flips it', () => {
    usePresenceStore.setState({ roster: [person('owner'), person('alice')], supported: true })
    renderIn(primary, 2)
    expect(toggles().map((t) => t.getAttribute('data-mode-toggle'))).toEqual(['claimer', 'claimer'])
    fireEvent.click(toggles()[1].querySelector('button[title^="View only"]') as HTMLElement)
    expect(toggles().map((t) => t.getAttribute('data-mode-toggle'))).toEqual(['viewer', 'viewer'])
  })

  it('the daemon says no input rights: the pencil is disabled', () => {
    usePresenceStore.setState({ roster: [person('owner'), person('alice')], supported: true })
    useWindowModeStore.setState({ capable: false })
    renderIn(primary)
    const pencil = toggles()[0].querySelectorAll('button')[1] as HTMLButtonElement
    expect(pencil.disabled).toBe(true)
    expect(pencil.title).toBe('View-only — ask the owner for edit access')
  })

  it('with no session around it, it shows only on the eye', () => {
    usePresenceStore.setState({ roster: [person('owner'), person('alice')], supported: true })
    renderIn(null)
    expect(toggles()).toHaveLength(0)
    act(() => useWindowModeStore.setState({ mode: 'viewer' }))
    expect(toggles()).toHaveLength(1)
  })
})

describe('<ModeToggle /> in a pinned room on another server', () => {
  function remote(readOnly: boolean): ModeToggleSession {
    const presence = createStore<PresenceView>(() => ({ roster: [person('bob'), person('alice')], supported: true }))
    return {
      path: WS,
      room: { isPrimary: false, readOnly, presence, scope: { label: 'B' } as never },
    }
  }

  it('a view-only room shows its eye, disabled, set by that server', () => {
    renderIn(remote(true))
    const [t] = toggles()
    expect(t?.getAttribute('data-mode-source')).toBe('room')
    expect(t?.getAttribute('data-mode-toggle')).toBe('viewer')
    expect(t?.getAttribute('title')).toBe('Set by B')
    for (const b of Array.from(t.querySelectorAll('button'))) expect((b as HTMLButtonElement).disabled).toBe(true)
  })

  it('a usable room is on the pencil and not this window’s choice: no switch', () => {
    renderIn(remote(false))
    expect(toggles()).toHaveLength(0)
  })
})
