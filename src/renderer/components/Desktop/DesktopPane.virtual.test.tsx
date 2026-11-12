// @vitest-environment jsdom
// The Desktop tab on a Linux server's virtual desktops
// (prd-linux-virtual-desktop-v1 §6, L6; LD-6, LD-30, LD-35–37), against a
// fake daemon speaking contract-virtual-desktop-wire-v1, fail loud:
//  - each virtual state renders its sentence and its one action for the
//    right role (`you.mayStart`); a Member sees no Start (and the Member
//    sentences);
//  - Start posts the pane's size; Set up is the Owner's and shows the
//    setup card and the command to run by hand; a refused start shows the
//    daemon's words on the card;
//  - an Owner's shown tab on an asleep desktop asks to wake it; a Member's
//    and a hidden tab show the card;
//  - a closed desktop shows its reason; `unavailable` shows the daemon's
//    message with Retry; `waiting` its place in line; `gone` keeps the
//    picker; an unknown state falls back to the generic card;
//  - the picker lists what `desktop/list` returns, hidden with one entry;
//    picking stamps the tab;
//  - a new tab asks the list for its workspace (`?workspace=<project id>`)
//    and opens its `default`;
//  - an agent's desktop shows the daemon's label and, while an agent
//    session holds it, names it and its end time;
//  - Close desktop asks first, then posts stop;
//  - without `virtual-desktop-v1` no Start or Set up shows;
//  - on a macOS server (one desktop) nothing changes.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent } from '@testing-library/react'
import { useState } from 'react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))

type Params = Record<string, unknown> | undefined
const daemon = vi.hoisted(() => ({
  status: {} as Record<string, unknown>,
  list: null as unknown,
  gets: [] as Array<{ route: string; params: Params }>,
  posts: [] as Array<{ route: string; body: unknown }>,
  refuse: null as null | { route: string; err: Error },
}))
vi.mock('@/stores/session-events', async () => {
  const actual = await vi.importActual<typeof import('@/stores/session-events')>('@/stores/session-events')
  return { ...actual, onDesktopState: () => () => {} }
})
vi.mock('@/lib/daemon-cli', async () => {
  const actual = await vi.importActual<typeof import('@/lib/daemon-cli')>('@/lib/daemon-cli')
  return {
    ...actual,
    daemonCliGet: vi.fn(async (_scope: unknown, route: string, params?: Params) => {
      if (route === 'themes/list') return []
      daemon.gets.push({ route, params })
      if (route === 'desktop/status') {
        const id = (params?.desktop as string | undefined) ?? 'console'
        const s = daemon.status[id]
        if (!s) throw new Error(`no status for ${id}`)
        return s
      }
      if (route === 'desktop/list') {
        if (daemon.list === null) throw new Error('no list here')
        return daemon.list
      }
      if (route === 'desktop/displays') return { displays: [] }
      throw new Error(`unexpected GET ${route}`)
    }),
    daemonCliPost: vi.fn(async (_scope: unknown, route: string, body: unknown) => {
      if (route === 'desktop/ticket') throw new Error('no stream in this suite')
      daemon.posts.push({ route, body })
      if (daemon.refuse && daemon.refuse.route === route) throw daemon.refuse.err
      if (route === 'desktop/start' || route === 'desktop/stop' || route === 'desktop/setup') return { ok: true }
      throw new Error(`unexpected POST ${route}`)
    }),
  }
})

import { DesktopPane } from './DesktopPane'
import { CLOSE_DESKTOP_CONFIRM, SEPARATE_NOT_PRIVATE } from './DesktopToolbar'
import { AGENT_SWITCH_OFF_LINE, NO_APPS_LINE, NO_SLEEP_LINE, STATE_CARDS, type DesktopStatus, type VirtualState } from './desktop-state'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { fakeScope } from '@/test-utils/fake-scope'
import { TabVisibilityContext } from '@/contexts/TabVisibilityContext'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import type { ServerScope } from '@/kessel/server-scope'
import type { DesktopItemData } from '@/stores/tabs'

const setDesktopItemState = vi.fn()
const tabs = { getState: () => ({ setDesktopItemState }) }

type You = DesktopStatus['you'] & { mayStart?: boolean }
type Over = Omit<Partial<DesktopStatus>, 'you'> & { you?: Partial<You> }
const OWNER = { role: 'owner', mayView: true, mayControl: true, mayManage: true, mayStart: true }
const ADMIN = { role: 'admin', mayView: true, mayControl: true, mayManage: false, mayStart: true }
const MEMBER = { role: 'member', mayView: true, mayControl: false, mayManage: false, mayStart: false }

function vstatus(desktop: string, over: Over = {}): DesktopStatus {
  const { you, ...rest } = over
  const agent = desktop.startsWith('virtual:')
  return {
    desktop, machine: 'test-node-07', enabled: true, backend: 'linux', state: 'closed', message: 'The desktop isn’t running.', lane: 'main',
    kind: agent ? 'agent' : 'server',
    name: agent ? 'scout' : 'This server',
    reason: null,
    ...(agent ? { workspaceId: desktop.slice('virtual:'.length), agentsAllowed: true } : {}),
    you: { ...OWNER, sameMachine: false, ...you } as DesktopStatus['you'],
    virtual: {
      size: null, display: null, memoryBytes: null, present: [], hold: null, nextSleepAt: null, nextCloseAt: null, canSleep: true,
      screen: 'off', waiting: null, label: agent ? 'Separate, not private' : null, missing: [], manual: null, setupCard: null, suggestRecycle: false,
    },
    ...rest,
  }
}

const AGENT = 'virtual:w9'
const LIST3 = {
  default: 'virtual',
  desktops: [
    { id: 'virtual', kind: 'server', name: 'This server', state: 'ready' },
    { id: AGENT, kind: 'agent', name: 'scout', state: 'asleep', workspaceId: 'w9', label: 'Separate, not private' },
    { id: 'virtual:w2', kind: 'agent', name: 'ada', state: 'closed', workspaceId: 'w2', label: 'Separate, not private' },
  ],
}

function linuxScope(features: string[] = ['remote-desktop-v1', 'virtual-desktop-v1']): ServerScope {
  return { ...fakeScope('other.k2.dev'), serverSupports: (f: string) => features.includes(f) }
}

let setShown: (v: boolean) => void = () => {}
function Harness(props: { data: DesktopItemData; shown?: boolean }): React.JSX.Element {
  const [shown, set] = useState(props.shown ?? true)
  setShown = set
  return (
    <TabVisibilityContext.Provider value={shown}>
      <DesktopPane tabId="t1" paneGroupId="pg1" itemId="i1" data={props.data} />
    </TabVisibilityContext.Provider>
  )
}

async function render(data: DesktopItemData, opts: { scope?: ServerScope; shown?: boolean; activeProjectId?: string } = {}): Promise<void> {
  await act(async () => {
    renderInRoom(
      testRoom({ tabs, scope: opts.scope ?? linuxScope(), isPrimary: false, activeProjectId: opts.activeProjectId ?? 'w1' }),
      <Harness data={data} shown={opts.shown} />,
    )
  })
  for (let i = 0; i < 10; i++) await act(async () => {})
}

function card(): HTMLElement {
  const el = document.querySelector<HTMLElement>('[data-desktop-card]')
  if (!el) throw new Error(`no card; pane: ${document.body.innerHTML.slice(0, 600)}`)
  return el
}
const line = (): string => card().querySelector('[data-desktop-card-line]')?.textContent ?? ''
const notes = (): string[] => Array.from(card().querySelectorAll('[data-desktop-card-note]')).map((n) => n.textContent ?? '')
const actions = (): string[] =>
  Array.from(card().querySelectorAll<HTMLElement>('[data-desktop-card-action]')).map((a) => a.dataset.desktopCardAction ?? '')

beforeEach(() => {
  daemon.status = { virtual: vstatus('virtual') }
  daemon.list = { default: 'virtual', desktops: [{ id: 'virtual', kind: 'server', name: 'This server', state: 'closed' }] }
  daemon.gets = []
  daemon.posts = []
  daemon.refuse = null
  setDesktopItemState.mockReset()
})
afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

describe('virtual states and their actions', () => {
  // [state, role, the action that role sees]
  const CASES: Array<[VirtualState | 'unavailable', typeof OWNER, string[]]> = [
    ['not_set_up', OWNER, ['set-up']],
    ['not_set_up', ADMIN, []],
    ['setting_up', OWNER, []],
    ['closed', OWNER, ['start']],
    ['closed', ADMIN, ['start']],
    ['closed', MEMBER, []],
    ['starting', OWNER, []],
    ['waking', OWNER, []],
    ['waiting', OWNER, []],
    ['gone', OWNER, []],
    ['unavailable', OWNER, ['start']],
    ['unavailable', MEMBER, ['retry']],
  ]
  it.each(CASES)('%s as %o: its sentence and its one action', async (state, you, want) => {
    daemon.status = { virtual: vstatus('virtual', { state, you }) }
    await render({ desktopId: 'virtual' })
    expect(card().dataset.desktopCard).toBe(state)
    expect(actions()).toEqual(want)
  })

  it('the §6 sentences, word for word', async () => {
    daemon.status = { virtual: vstatus('virtual', { state: 'not_set_up' }) }
    await render({ desktopId: 'virtual' })
    expect(line()).toBe('This server has no screen. K2 can start a desktop on it once three small packages are installed.')
    cleanup()
    daemon.status = { virtual: vstatus('virtual', { state: 'closed' }) }
    await render({ desktopId: 'virtual' })
    expect(line()).toBe('This server has no screen. K2 can start a desktop on it.')
    cleanup()
    daemon.status = { virtual: vstatus('virtual', { state: 'closed', you: MEMBER }) }
    await render({ desktopId: 'virtual' })
    expect(line()).toBe("The desktop isn't running. An Owner or Admin can start it.")
    cleanup()
    daemon.status = { virtual: vstatus('virtual', { state: 'gone' }) }
    await render({ desktopId: 'virtual' })
    expect(line()).toBe('This desktop no longer exists.')
  })

  it('waiting: how many run at once, and its place in line', async () => {
    const base = vstatus('virtual', { state: 'waiting' })
    daemon.status = { virtual: { ...base, virtual: { ...base.virtual, waiting: { position: 2, awakeLimit: 2 } } } }
    await render({ desktopId: 'virtual' })
    expect(line()).toBe('Waiting for another desktop to go to sleep. This server runs 2 at a time.')
    expect(notes()).toEqual(['It is number 2 in line.'])
  })

  it('unavailable shows the daemon’s message; Retry starts it again', async () => {
    daemon.status = { virtual: vstatus('virtual', { state: 'unavailable', message: 'K2 can’t start a desktop here: /tmp/.X11-unix is missing.' }) }
    await render({ desktopId: 'virtual' })
    expect(line()).toBe('K2 can’t start a desktop here: /tmp/.X11-unix is missing.')
    await act(async () => {
      fireEvent.click(card().querySelector<HTMLElement>('[data-desktop-card-action="start"]')!)
    })
    expect(daemon.posts.map((p) => p.route)).toEqual(['desktop/start'])
  })

  it('Set up: the setup card, and the command to run by hand', async () => {
    const base = vstatus('virtual', { state: 'not_set_up' })
    daemon.status = {
      virtual: {
        ...base,
        virtual: { ...base.virtual, missing: ['xvfb', 'icewm'], manual: 'sudo apt-get install xvfb icewm fonts-dejavu-core', setupCard: 'Installs Xvfb, IceWM and a font from your distro.' },
      },
    }
    await render({ desktopId: 'virtual' })
    expect(notes()).toEqual(['Installs Xvfb, IceWM and a font from your distro.', 'To install them by hand: sudo apt-get install xvfb icewm fonts-dejavu-core'])
  })

  it('a closed desktop shows its reason; a server that can’t pause says so', async () => {
    daemon.status = {
      virtual: vstatus('virtual', { state: 'closed', message: 'Your desktop was closed after a day asleep.', reason: { code: 'asleep_too_long', at: 1731300000 } }),
    }
    await render({ desktopId: 'virtual' })
    expect(line()).toBe('Your desktop was closed after a day asleep.')
    expect(actions()).toEqual(['start'])
    cleanup()
    const base = vstatus('virtual', { state: 'closed' })
    daemon.status = { virtual: { ...base, virtual: { ...base.virtual, canSleep: false } } }
    await render({ desktopId: 'virtual' })
    expect(notes()).toContain(NO_SLEEP_LINE)
  })

  it('the Start card says it opens with no apps when the server has no browser (LD-20)', async () => {
    const base = vstatus('virtual', { state: 'closed' })
    daemon.status = { virtual: { ...base, virtual: { ...base.virtual, hasBrowser: false } } }
    await render({ desktopId: 'virtual' })
    expect(notes()).toContain(NO_APPS_LINE)
    cleanup()
    daemon.status = { virtual: { ...base, virtual: { ...base.virtual, hasBrowser: true } } }
    await render({ desktopId: 'virtual' })
    expect(notes()).not.toContain(NO_APPS_LINE)
    cleanup()
    // A Member can't start it: no line about what it opens with.
    const member = vstatus('virtual', { state: 'closed', you: MEMBER })
    daemon.status = { virtual: { ...member, virtual: { ...member.virtual, hasBrowser: false } } }
    await render({ desktopId: 'virtual' })
    expect(notes()).not.toContain(NO_APPS_LINE)
  })

  it('an agent’s desktop whose switch is off has no Start, and says why', async () => {
    daemon.list = LIST3
    daemon.status = { [AGENT]: vstatus(AGENT, { state: 'closed', agentsAllowed: false }) }
    await render({ desktopId: AGENT, name: 'scout' })
    expect(actions()).toEqual([])
    expect(notes()).toContain(AGENT_SWITCH_OFF_LINE)
  })

  it('Start posts this pane’s size; Set up posts setup', async () => {
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue({
      x: 0, y: 0, left: 0, top: 0, width: 1500, height: 801, right: 1500, bottom: 801, toJSON: () => ({}),
    } as DOMRect)
    daemon.status = { virtual: vstatus('virtual', { state: 'closed' }) }
    await render({ desktopId: 'virtual' })
    await act(async () => {
      fireEvent.click(card().querySelector<HTMLElement>('[data-desktop-card-action="start"]')!)
    })
    expect(daemon.posts).toEqual([{ route: 'desktop/start', body: { desktop: 'virtual', width: 1500, height: 801 } }])
    vi.restoreAllMocks()
    cleanup()
    daemon.posts = []
    daemon.status = { virtual: vstatus('virtual', { state: 'not_set_up' }) }
    await render({ desktopId: 'virtual' })
    await act(async () => {
      fireEvent.click(card().querySelector<HTMLElement>('[data-desktop-card-action="set-up"]')!)
    })
    expect(daemon.posts).toEqual([{ route: 'desktop/setup', body: {} }])
  })

  it('a refused start shows the daemon’s words and what the refusal carries', async () => {
    daemon.status = { virtual: vstatus('virtual', { state: 'closed' }) }
    daemon.refuse = {
      route: 'desktop/start',
      err: new DaemonHttpError('This server already has 4 desktops, its most.', 409, 'too_many_desktops', null, {
        ok: false, error: { code: 'too_many_desktops', hint: 'This server already has 4 desktops, its most.' }, closable: ['virtual:w2'],
      }),
    }
    await render({ desktopId: 'virtual' })
    await act(async () => {
      fireEvent.click(card().querySelector<HTMLElement>('[data-desktop-card-action="start"]')!)
    })
    for (let i = 0; i < 5; i++) await act(async () => {})
    expect(notes()).toContain('This server already has 4 desktops, its most. Close one first: virtual:w2.')
  })

  it('a setup K2 can’t do shows the command to run by hand', async () => {
    daemon.status = { virtual: vstatus('virtual', { state: 'not_set_up' }) }
    daemon.refuse = {
      route: 'desktop/setup',
      err: new DaemonHttpError('', 409, 'setup_manual', null, { ok: false, error: { code: 'setup_manual', hint: '' }, manual: 'sudo pacman -S xorg-server-xvfb icewm ttf-dejavu' }),
    }
    await render({ desktopId: 'virtual' })
    await act(async () => {
      fireEvent.click(card().querySelector<HTMLElement>('[data-desktop-card-action="set-up"]')!)
    })
    for (let i = 0; i < 5; i++) await act(async () => {})
    expect(notes()).toContain('K2 can’t install the desktop packages on this server. To install them by hand: sudo pacman -S xorg-server-xvfb icewm ttf-dejavu')
  })

  it('the status asked for is this tab’s desktop', async () => {
    daemon.status = { [AGENT]: vstatus(AGENT, { state: 'closed' }) }
    daemon.list = LIST3
    await render({ desktopId: AGENT, name: 'scout' })
    expect(daemon.gets.filter((g) => g.route === 'desktop/status').map((g) => g.params)).toContainEqual({ desktop: AGENT })
  })
})

describe('asleep', () => {
  it('an Owner’s shown tab asks to wake it', async () => {
    daemon.status = { virtual: vstatus('virtual', { state: 'asleep' }) }
    await render({ desktopId: 'virtual' })
    expect(daemon.posts.map((p) => p.route)).toEqual(['desktop/start'])
    expect((daemon.posts[0].body as { desktop: string }).desktop).toBe('virtual')
  })

  it('a hidden tab asks nothing; shown, it asks to wake', async () => {
    daemon.status = { virtual: vstatus('virtual', { state: 'asleep' }) }
    await render({ desktopId: 'virtual' }, { shown: false })
    expect(daemon.posts).toEqual([])
    await act(async () => setShown(true))
    for (let i = 0; i < 10; i++) await act(async () => {})
    expect(daemon.posts.map((p) => p.route)).toEqual(['desktop/start'])
  })

  it('a Member’s tab shows the Member card and asks nothing', async () => {
    daemon.status = { virtual: vstatus('virtual', { state: 'asleep', you: MEMBER }) }
    await render({ desktopId: 'virtual' })
    expect(card().dataset.desktopCard).toBe('asleep')
    expect(line()).toBe('The desktop is asleep. An Owner or Admin can wake it.')
    expect(daemon.posts).toEqual([])
  })

  it('an older daemon that doesn’t say mayStart: no wake, no Start', async () => {
    const s = vstatus('virtual', { state: 'asleep' })
    delete (s.you as { mayStart?: boolean }).mayStart
    daemon.status = { virtual: s }
    await render({ desktopId: 'virtual' })
    expect(daemon.posts).toEqual([])
  })
})

describe('unknown and older states', () => {
  it('an unknown state falls back to the generic card', async () => {
    daemon.status = { virtual: vstatus('virtual', { state: 'hibernating', message: '' }) }
    await render({ desktopId: 'virtual' })
    expect(card().dataset.desktopCard).toBe('unavailable')
    expect(line()).toBe(STATE_CARDS.unavailable.message)
  })

  it('without virtual-desktop-v1 no Start or Set up shows', async () => {
    for (const state of ['closed', 'not_set_up']) {
      daemon.status = { virtual: vstatus('virtual', { state }) }
      await render({ desktopId: 'virtual' }, { scope: linuxScope(['remote-desktop-v1']) })
      expect(card().dataset.desktopCard).toBe('unavailable')
      expect(actions()).toEqual([])
      expect(daemon.gets.some((g) => g.route === 'desktop/list')).toBe(false)
      cleanup()
    }
  })
})

describe('the picker and the first desktop', () => {
  it('lists what desktop/list returns; picking stamps the tab', async () => {
    daemon.list = LIST3
    daemon.status = { virtual: vstatus('virtual', { state: 'closed' }) }
    await render({ desktopId: 'virtual' })
    const picker = document.querySelector<HTMLSelectElement>('[data-desktop-picker]')!
    expect(picker).not.toBeNull()
    expect(Array.from(picker.options).map((o) => [o.value, o.textContent])).toEqual([
      ['virtual', 'This server (awake)'],
      [AGENT, 'scout (asleep)'],
      ['virtual:w2', 'ada (not started)'],
    ])
    fireEvent.change(picker, { target: { value: 'virtual:w2' } })
    expect(setDesktopItemState).toHaveBeenCalledWith('t1', 'pg1', 'i1', { desktopId: 'virtual:w2', name: 'ada', displayId: undefined })
  })

  it('is absent with one entry', async () => {
    daemon.status = { virtual: vstatus('virtual', { state: 'closed' }) }
    await render({ desktopId: 'virtual' })
    expect(daemon.gets.some((g) => g.route === 'desktop/list')).toBe(true)
    expect(document.querySelector('[data-desktop-picker]')).toBeNull()
  })

  it('a gone desktop keeps the picker to choose another', async () => {
    daemon.list = LIST3
    daemon.status = { 'virtual:w7': vstatus('virtual:w7', { state: 'gone' }) }
    await render({ desktopId: 'virtual:w7', name: 'old' })
    expect(card().dataset.desktopCard).toBe('gone')
    expect(document.querySelector('[data-desktop-picker]')).not.toBeNull()
  })

  it('a new tab asks the list for its workspace and opens its default', async () => {
    daemon.list = { ...LIST3, default: AGENT }
    daemon.status = { virtual: vstatus('virtual'), [AGENT]: vstatus(AGENT) }
    await render({}, { activeProjectId: 'w9' })
    expect(daemon.gets.find((g) => g.route === 'desktop/list')?.params).toEqual({ workspace: 'w9' })
    expect(setDesktopItemState).toHaveBeenCalledWith('t1', 'pg1', 'i1', { desktopId: AGENT, name: 'scout' })
  })

  it('a new tab whose default is This server opens This server', async () => {
    daemon.list = LIST3
    await render({}, { activeProjectId: 'w5' })
    expect(setDesktopItemState).toHaveBeenCalledWith('t1', 'pg1', 'i1', { desktopId: 'virtual' })
  })

  it('a restored tab doesn’t ask for a workspace', async () => {
    daemon.list = LIST3
    await render({ desktopId: 'virtual' })
    expect(daemon.gets.find((g) => g.route === 'desktop/list')?.params).toBeUndefined()
  })
})

describe('an agent’s desktop', () => {
  it('shows the daemon’s label and names the session holding it, until when', async () => {
    daemon.list = LIST3
    const until = Date.UTC(2026, 10, 11, 23, 5) / 1000
    const base = vstatus(AGENT, { state: 'asleep', you: MEMBER })
    daemon.status = { [AGENT]: { ...base, virtual: { ...base.virtual, hold: { by: 'k2/scout', name: 'scout (reviewer)', until } } } }
    await render({ desktopId: AGENT, name: 'scout' })
    expect(document.querySelector('[data-desktop-separate]')?.textContent).toBe(SEPARATE_NOT_PRIVATE)
    const el = document.querySelector<HTMLElement>('[data-desktop-hold]')!
    const time = new Date(until * 1000).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })
    // The name in the line; the session's address on hover.
    expect(el.textContent).toContain(`scout (reviewer) is working here until ${time}.`)
    expect(el.textContent).toContain('You and scout (reviewer) can both move the pointer.')
    expect(el.textContent).not.toContain('k2/scout')
    expect(el.title).toBe('k2/scout')
  })

  it('a hold with no name (an older daemon) shows the address', async () => {
    daemon.list = LIST3
    const base = vstatus(AGENT, { state: 'asleep', you: MEMBER })
    daemon.status = { [AGENT]: { ...base, virtual: { ...base.virtual, hold: { by: 'k2/scout', until: 1731301800 } } } }
    await render({ desktopId: AGENT, name: 'scout' })
    expect(document.querySelector('[data-desktop-hold]')?.textContent).toContain('k2/scout is working here until')
  })

  it('This server has no label and no hold line', async () => {
    daemon.list = LIST3
    daemon.status = { virtual: vstatus('virtual', { state: 'closed' }) }
    await render({ desktopId: 'virtual' })
    expect(document.querySelector('[data-desktop-separate]')).toBeNull()
    expect(document.querySelector('[data-desktop-hold]')).toBeNull()
  })
})

describe('Close desktop', () => {
  it('asks first (open windows are lost), then posts stop; a Member has none', async () => {
    daemon.status = { virtual: vstatus('virtual', { state: 'ready' }) }
    await render({ desktopId: 'virtual' })
    fireEvent.click(document.querySelector<HTMLElement>('[data-desktop-close]')!)
    expect(daemon.posts).toEqual([])
    expect(document.querySelector('[data-desktop-close-confirm]')?.textContent).toContain(CLOSE_DESKTOP_CONFIRM)
    await act(async () => {
      fireEvent.click(document.querySelector<HTMLElement>('[data-desktop-close-yes]')!)
    })
    expect(daemon.posts).toEqual([{ route: 'desktop/stop', body: { desktop: 'virtual' } }])
    cleanup()
    daemon.status = { virtual: vstatus('virtual', { state: 'ready', you: MEMBER }) }
    await render({ desktopId: 'virtual' })
    expect(document.querySelector('[data-desktop-close]')).toBeNull()
  })
})

describe('a macOS server', () => {
  const mac = (): ServerScope => ({ ...fakeScope('your-server.k2.dev'), serverSupports: (f: string) => f === 'remote-desktop-v1' })
  it('nothing changes: no picker, no label, no Close desktop, no list asked', async () => {
    daemon.status = {
      console: { desktop: 'console', machine: 'office-mini', enabled: true, backend: 'macos', state: 'ready', message: '', lane: 'main', you: { ...OWNER, mayStart: false, sameMachine: false } },
    }
    await render({ desktopId: 'console' }, { scope: mac() })
    expect(document.querySelector('[data-desktop-picker]')).toBeNull()
    expect(document.querySelector('[data-desktop-separate]')).toBeNull()
    expect(document.querySelector('[data-desktop-close]')).toBeNull()
    expect(daemon.gets.some((g) => g.route === 'desktop/list')).toBe(false)
    expect(daemon.gets.filter((g) => g.route === 'desktop/status').map((g) => g.params)).toEqual([undefined])
  })

  it('a new tab opens This server without asking for a list', async () => {
    daemon.status = {
      console: { desktop: 'console', machine: 'office-mini', enabled: true, backend: 'macos', state: 'ready', message: '', lane: 'main', you: { ...OWNER, mayStart: false, sameMachine: false } },
    }
    await render({}, { scope: mac() })
    expect(daemon.gets.some((g) => g.route === 'desktop/list')).toBe(false)
    expect(document.querySelector('[data-desktop-pane]')).not.toBeNull()
  })
})
