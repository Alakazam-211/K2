// @vitest-environment jsdom
//
// 0.46.11 (Rosson): the eye/pencil shows in the session bar at all times,
// alone or not, once the window's mode has resolved. A pinned remote room
// shows its fixed mode, eye or pencil, disabled.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render } from '@testing-library/react'

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

import ModeToggle, { ModeToggleSessionContext, type ModeToggleSession } from './ModeToggle'
import { useWindowModeStore, __resetWindowModeForTests } from '@/stores/window-mode'

const primary: ModeToggleSession = {
  room: { isPrimary: true, readOnly: false, scope: { label: 'This computer' } as never },
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
})
afterEach(() => cleanup())

describe('<ModeToggle /> in a session bar', () => {
  it('alone on the pencil: shown, pencil lit, and usable', () => {
    renderIn(primary)
    const [t] = toggles()
    expect(t?.getAttribute('data-mode-toggle')).toBe('claimer')
    expect(t?.querySelector('button[aria-pressed="true"]')?.getAttribute('title')).toBe('Edit — type and resize')
    fireEvent.click(t.querySelector('button[title^="View only"]') as HTMLElement)
    expect(useWindowModeStore.getState().mode).toBe('viewer')
    fireEvent.click(toggles()[0].querySelector('button[title^="Edit"]') as HTMLElement)
    expect(useWindowModeStore.getState().mode).toBe('claimer')
    expect(toggles()).toHaveLength(1)
  })

  it('on the eye: shown', () => {
    useWindowModeStore.setState({ mode: 'viewer', resolved: true })
    renderIn(primary)
    expect(toggles()[0]?.getAttribute('data-mode-toggle')).toBe('viewer')
  })

  it('unresolved: hidden until the mode resolves, so it does not flash', () => {
    useWindowModeStore.setState({ resolved: false })
    renderIn(primary)
    expect(toggles()).toHaveLength(0)
    act(() => useWindowModeStore.setState({ mode: 'claimer', resolved: true }))
    expect(toggles()).toHaveLength(1)
  })

  it('several bars on screen show the same window state, and either flips it', () => {
    renderIn(primary, 2)
    expect(toggles().map((t) => t.getAttribute('data-mode-toggle'))).toEqual(['claimer', 'claimer'])
    fireEvent.click(toggles()[1].querySelector('button[title^="View only"]') as HTMLElement)
    expect(toggles().map((t) => t.getAttribute('data-mode-toggle'))).toEqual(['viewer', 'viewer'])
  })

  it('the daemon says no input rights: the pencil is disabled', () => {
    useWindowModeStore.setState({ capable: false })
    renderIn(primary)
    const pencil = toggles()[0].querySelectorAll('button')[1] as HTMLButtonElement
    expect(pencil.disabled).toBe(true)
    expect(pencil.title).toBe('View-only — ask the owner for edit access')
  })

  it('with no session around it, it shows the window mode too', () => {
    renderIn(null)
    expect(toggles()[0]?.getAttribute('data-mode-toggle')).toBe('claimer')
  })
})

describe('<ModeToggle /> in a pinned room on another server', () => {
  const remote = (readOnly: boolean): ModeToggleSession => ({
    room: { isPrimary: false, readOnly, scope: { label: 'B' } as never },
  })

  for (const [readOnly, mode] of [[true, 'viewer'], [false, 'claimer']] as const) {
    it(`a ${readOnly ? 'view-only' : 'usable'} room shows its ${mode === 'viewer' ? 'eye' : 'pencil'}, disabled, set by that server`, () => {
      renderIn(remote(readOnly))
      const [t] = toggles()
      expect(t?.getAttribute('data-mode-source')).toBe('room')
      expect(t?.getAttribute('data-mode-toggle')).toBe(mode)
      expect(t?.getAttribute('title')).toBe('Set by B')
      for (const b of Array.from(t.querySelectorAll('button'))) expect((b as HTMLButtonElement).disabled).toBe(true)
    })
  }

  it('shown even before the window mode resolves (its mode is the room’s)', () => {
    useWindowModeStore.setState({ resolved: false })
    renderIn(remote(false))
    expect(toggles()).toHaveLength(1)
  })
})
