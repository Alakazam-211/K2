// @vitest-environment jsdom
// The Desktop tab's pane, D7a (prd-remote-desktop-tab-v1 §8; vs-live R22),
// against a fake daemon, fail loud:
//  - every state renders its line and exactly its one action (none where
//    the table has none; an owner-only action only for an Owner);
//  - Turn on shows the S14 sentence first and POSTs desktop/enable only on
//    the second click; Retry / Try again re-read status;
//  - the tab dials the ROOM's server (a tab under another server's
//    workspace asks that server, never the window's);
//  - an older daemon (no remote-desktop-v1) is never asked and says to
//    update K2 there;
//  - ready shows the toolbar with the machine's name and the picture's
//    stage; the stream buttons are disabled until the stream opens;
//  - a `desktop_state` event re-reads status.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render as rtlRender, screen } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))

const daemon = vi.hoisted(() => ({
  status: null as unknown,
  gets: [] as Array<{ scope: string; route: string }>,
  posts: [] as Array<{ scope: string; route: string; body: unknown }>,
}))
const events = vi.hoisted(() => ({ fns: new Set<() => void>() }))
vi.mock('@/stores/session-events', async () => {
  const actual = await vi.importActual<typeof import('@/stores/session-events')>('@/stores/session-events')
  return {
    ...actual,
    onDesktopState: (_scope: unknown, fn: () => void) => {
      events.fns.add(fn)
      return () => events.fns.delete(fn)
    },
  }
})
vi.mock('@/lib/daemon-cli', async () => {
  const actual = await vi.importActual<typeof import('@/lib/daemon-cli')>('@/lib/daemon-cli')
  return {
    ...actual,
    daemonCliGet: vi.fn(async (scope: { hostKey: string }, route: string) => {
      // The settings store's own boot reads (themes) are not this pane's.
      if (route === 'themes/list') return []
      daemon.gets.push({ scope: scope.hostKey, route })
      if (route === 'desktop/status') return daemon.status
      if (route === 'desktop/displays') return { displays: [{ id: '1', name: 'Built-in', widthPt: 1512, heightPt: 982, widthPx: 3024, heightPx: 1964, scale: 2, main: true }] }
      throw new Error(`unexpected GET ${route}`)
    }),
    daemonCliPost: vi.fn(async (scope: { hostKey: string }, route: string, body: unknown) => {
      // The stream's ticket: this suite has no socket (DesktopPane.stream.test.tsx has).
      if (route === 'desktop/ticket') throw new Error('no stream in this suite')
      daemon.posts.push({ scope: scope.hostKey, route, body })
      if (route === 'desktop/enable') return { enabled: true, state: 'ready', card: 'CARD FROM DAEMON' }
      throw new Error(`unexpected POST ${route}`)
    }),
  }
})

import { DesktopPane } from './DesktopPane'
import { NEEDS_STREAM_TITLE } from './DesktopToolbar'
import { DesktopStateCard } from './DesktopStateCard'
import { DESKTOP_ENABLE_CARD, STATE_CARDS, type DesktopStatus, type TabState } from './desktop-state'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { fakeScope } from '@/test-utils/fake-scope'
import type { ServerScope } from '@/kessel/server-scope'

const setDesktopItemState = vi.fn()
const tabs = { getState: () => ({ setDesktopItemState }) }

function status(over: Omit<Partial<DesktopStatus>, 'you'> & { you?: Partial<DesktopStatus['you']> } = {}): DesktopStatus {
  const { you, ...rest } = over
  return {
    desktop: 'console', machine: 'office-mini', enabled: true, backend: 'macos', state: 'ready', message: '', lane: 'main',
    you: { role: 'owner', mayView: true, mayControl: true, mayManage: true, sameMachine: false, ...you },
    ...rest,
  }
}

async function render(scope: ServerScope = fakeScope('b.k2.dev')): Promise<void> {
  await act(async () => {
    renderInRoom(
      testRoom({ tabs, scope, isPrimary: false }),
      <DesktopPane tabId="t1" paneGroupId="pg1" itemId="i1" data={{ desktopId: 'console' }} />,
    )
  })
  await act(async () => {})
}

beforeEach(() => {
  daemon.status = status()
  daemon.gets = []
  daemon.posts = []
  events.fns.clear()
  setDesktopItemState.mockReset()
})
afterEach(() => cleanup())

function card(): HTMLElement {
  const el = document.querySelector<HTMLElement>('[data-desktop-card]')
  if (!el) throw new Error(`no card; pane: ${document.body.innerHTML.slice(0, 400)}`)
  return el
}

// The status that lands on each card, as the owner (actions shown).
const CASES: Array<[Exclude<TabState, 'ready' | 'update_k2' | 'waiting_consent' | 'denied'>, Omit<Partial<DesktopStatus>, 'you'> & { you?: Partial<DesktopStatus['you']> }]> = [
  ['off', { enabled: false }],
  ['not_allowed', { you: { mayView: false, mayManage: false, role: 'member' } }],
  ['same_machine', { you: { sameMachine: true } }],
  ['needs_permission', { state: 'needs_permission' }],
  ['locked', { state: 'locked' }],
  ['no_session', { state: 'no_session' }],
  ['no_display', { state: 'no_display' }],
  ['portal_allow', { state: 'portal_allow' }],
  ['needs_update', { state: 'needs_update' }],
  ['unavailable', { state: 'unavailable' }],
  ['blocked_in_login_items', { state: 'blocked_in_login_items' }],
]

describe('DesktopPane states', () => {
  it.each(CASES)('%s renders its line and exactly its one action', async (state, over) => {
    daemon.status = status(over)
    await render()
    expect(card().dataset.desktopCard).toBe(state)
    expect(card().querySelector('[data-desktop-card-line]')?.textContent).toBe(STATE_CARDS[state].message)
    const actions = card().querySelectorAll('[data-desktop-card-action]')
    const want = STATE_CARDS[state].action
    const shown = want && (!want.ownerOnly || status(over).you.mayManage) ? [want.id] : []
    expect(Array.from(actions).map((a) => (a as HTMLElement).dataset.desktopCardAction)).toEqual(shown)
  })

  it('an owner-only action is not offered to an Admin', async () => {
    daemon.status = status({ enabled: false, you: { role: 'admin', mayManage: false } })
    await render()
    expect(card().dataset.desktopCard).toBe('off')
    expect(card().querySelectorAll('[data-desktop-card-action]')).toHaveLength(0)
  })

  it('Turn on shows the S14 sentence first, then enables on the room’s server', async () => {
    daemon.status = status({ enabled: false })
    await render()
    const button = (): HTMLElement => card().querySelector<HTMLElement>('[data-desktop-card-action="turn-on"]')!
    fireEvent.click(button())
    expect(card().querySelector('[data-desktop-enable-card]')?.textContent).toBe(DESKTOP_ENABLE_CARD)
    expect(daemon.posts).toEqual([])
    daemon.status = status()
    await act(async () => {
      fireEvent.click(button())
    })
    await act(async () => {})
    expect(daemon.posts).toEqual([{ scope: 'b.k2.dev', route: 'desktop/enable', body: {} }])
    expect(document.querySelector('[data-desktop-picture]')).not.toBeNull()
  })

  it('Retry re-reads status; a desktop_state event does too', async () => {
    daemon.status = status({ state: 'needs_update' })
    await render()
    const before = daemon.gets.filter((g) => g.route === 'desktop/status').length
    await act(async () => {
      fireEvent.click(card().querySelector<HTMLElement>('[data-desktop-card-action="retry"]')!)
    })
    await act(async () => {})
    const afterRetry = daemon.gets.filter((g) => g.route === 'desktop/status').length
    expect(afterRetry).toBe(before + 1)
    daemon.status = status({ state: 'locked' })
    await act(async () => {
      for (const fn of events.fns) fn()
    })
    await act(async () => {})
    expect(card().dataset.desktopCard).toBe('locked')
  })
})

describe('DesktopPane server and version', () => {
  it('dials the server that owns the room, never the window’s (R22)', async () => {
    await render(fakeScope('other-box.k2.dev'))
    expect(daemon.gets.length).toBeGreaterThan(0)
    expect(new Set(daemon.gets.map((g) => g.scope))).toEqual(new Set(['other-box.k2.dev']))
  })

  it('an older daemon is never asked and says to update K2 there', async () => {
    await render({ ...fakeScope('old-box.k2.dev'), serverSupports: () => false })
    expect(card().dataset.desktopCard).toBe('update_k2')
    expect(card().textContent).toContain('Update K2 on this computer to use Remote desktop')
    expect(daemon.gets).toEqual([])
  })

  it('ready: the toolbar names the machine, the stream buttons wait for the live picture', async () => {
    await render()
    expect(document.querySelector('[data-desktop-machine]')?.textContent).toBe('office-mini')
    expect(document.querySelector('[data-desktop-picture]')).not.toBeNull()
    for (const sel of ['data-desktop-keys-menu', 'data-desktop-clip-send', 'data-desktop-clip-copy', 'data-desktop-take-control']) {
      const b = document.querySelector<HTMLButtonElement>(`[${sel}]`)!
      expect(b.disabled, sel).toBe(true)
      expect(b.title, sel).toBe(NEEDS_STREAM_TITLE)
    }
    // A watcher (no control) can't leave view-only or capture keys.
    cleanup()
    daemon.status = status({ you: { role: 'member', mayControl: false, mayManage: false } })
    await render()
    expect(document.querySelector<HTMLButtonElement>('[data-desktop-view-only]')!.disabled).toBe(true)
    expect(document.querySelector<HTMLButtonElement>('[data-desktop-view-only]')!.dataset.desktopViewOnly).toBe('true')
    expect(document.querySelector<HTMLButtonElement>('[data-desktop-capture]')!.disabled).toBe(true)
  })

  it('fit and display picks are stamped on the item', async () => {
    await render()
    fireEvent.click(document.querySelector<HTMLElement>('[data-desktop-fit]')!)
    expect(setDesktopItemState).toHaveBeenCalledWith('t1', 'pg1', 'i1', { fit: '1:1' })
  })
})

describe('DesktopStateCard (the cards no status reaches yet)', () => {
  it.each([
    ['waiting_consent', 'cancel'],
    ['denied', 'try-again'],
    ['update_k2', null],
  ] as const)('%s: its line and one action', (state, action) => {
    const onAction = vi.fn()
    rtlRender(<DesktopStateCard state={state} mayManage={false} onAction={onAction} />)
    expect(card().querySelector('[data-desktop-card-line]')?.textContent).toBe(STATE_CARDS[state].message)
    const shown = Array.from(card().querySelectorAll<HTMLElement>('[data-desktop-card-action]')).map((a) => a.dataset.desktopCardAction)
    expect(shown).toEqual(action ? [action] : [])
    if (action) {
      fireEvent.click(screen.getByText(STATE_CARDS[state].action!.label))
      expect(onAction).toHaveBeenCalledWith(action)
    }
  })
})
