// @vitest-environment jsdom
//
// Settings → Projects: fetch budget + switch behaviour (0.46.1,
// eval-project-settings-v1 P3/P4).
//
// A fake daemon answers every GET after a fixed latency (fake timers) and
// counts requests per route, so the suite can say exactly what a chat
// message, a project change or a switch costs:
//   - a chat message in any project fetches NOTHING for the settings page
//     (before 0.46.1: list + show + resources + the 10-50 KB icon each);
//   - a project-changed event for the open project refetches its show view
//     exactly once; one for ANOTHER project fetches no show/resources/icon;
//   - switching projects never blanks the panel to "Loading…" (the previous
//     project stays on screen until the next one's data lands), and going
//     back to a project already seen renders at once from the cache;
//   - the icon is fetched once per (project, icon) and reused; an unchanged
//     iconKey never refetches it;
//   - rename / pin / point of contact apply at once, take the daemon's
//     answer, and roll back with an error when the daemon refuses.
//
// The numbers printed under "[perf]" are what the 0.46.1 report quotes.

import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'main' }) }))

// ── Fake daemon ──────────────────────────────────────────────────────────

const { LATENCY_MS, ICON } = vi.hoisted(() => ({
  LATENCY_MS: 50,
  ICON: 'data:image/png;base64,' + 'A'.repeat(30_000),
}))

interface FakeGroup {
  id: string
  name: string
  pocWorkspaceId: string | null
  pinned: boolean
  color: string | null
  sortOrder: number
  createdAt: number
  updatedAt: number
  memberCount: number
  memberWorkspaceIds: string[]
  iconKey: string | null
}

const daemon = vi.hoisted(() => ({
  gets: [] as Array<{ route: string; params: Record<string, unknown> | undefined }>,
  posts: [] as Array<{ route: string; body: unknown }>,
  groups: [] as FakeGroup[],
  icons: {} as Record<string, string | null>,
  /** route → error message for the next POST to it (one shot). */
  failNext: {} as Record<string, string>,
}))

const { delay, findGroup } = vi.hoisted(() => ({
  delay<T>(fn: () => T): Promise<T> {
    return new Promise((resolve, reject) =>
      setTimeout(() => {
        try {
          resolve(fn())
        } catch (e) {
          reject(e)
        }
      }, 50),
    )
  },
  findGroup(id: unknown): FakeGroup {
    const g = daemon.groups.find((x) => x.id === id)
    if (!g) throw new Error(`fake daemon: no group ${String(id)}`)
    return g
  },
}))

function showOf(g: FakeGroup): unknown {
  return {
    ...g,
    memberWorkspaceIds: undefined,
    members: g.memberWorkspaceIds.map((w) => ({
      workspaceId: w,
      name: w,
      path: `/tmp/${w}`,
      agentName: w.toUpperCase(),
      createdAt: 1,
    })),
    dashboards: [
      { id: `${g.id}-d1`, groupId: g.id, name: 'Main', layoutJson: '{}', revision: 1, position: 0, createdAt: 1, updatedAt: 1 },
    ],
  }
}

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn((_scope: unknown, route: string, params?: Record<string, unknown>) => {
    daemon.gets.push({ route, params })
    return delay(() => {
      switch (route) {
        case 'project-group/list':
          return { ok: true, groups: daemon.groups.map((g) => ({ ...g })) }
        case 'project-group/show':
          return showOf(findGroup(params?.group))
        case 'project-group/icon': {
          const g = findGroup(params?.group)
          const dataUrl = daemon.icons[g.id] ?? null
          return { ok: true, found: dataUrl !== null, dataUrl, iconKey: g.iconKey }
        }
        case 'project-group/resources':
          return { ok: true, groupId: params?.group, docs: [] }
        default:
          return {}
      }
    })
  }),
  daemonCliPost: vi.fn((_scope: unknown, route: string, body?: Record<string, unknown>) => {
    daemon.posts.push({ route, body })
    return delay(() => {
      const fail = daemon.failNext[route]
      if (fail) {
        delete daemon.failNext[route]
        throw new Error(fail)
      }
      const g = body?.group ? findGroup(body.group) : null
      switch (route) {
        case 'project-group/rename':
          g!.name = String(body!.name)
          return { ...g! }
        case 'project-group/pin':
          g!.pinned = Boolean(body!.pinned)
          return { ...g! }
        case 'project-group/set-poc':
          g!.pocWorkspaceId = String(body!.workspace)
          return { ...g! }
        default:
          return { ok: true }
      }
    })
  }),
  withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))

// The session-events bus: keep the real module, capture the handlers the
// page + store register so the test can fire daemon frames through them.
const bus = vi.hoisted(() => ({
  projectGroups: [] as Array<(reason: string, groupId?: string) => void>,
}))
vi.mock('@/stores/session-events', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/stores/session-events')>()
  return {
    ...actual,
    onProjectGroupsChanged: vi.fn((_scope: unknown, fn: (reason: string, groupId?: string) => void) => {
      bus.projectGroups.push(fn)
      return () => void (bus.projectGroups = bus.projectGroups.filter((f) => f !== fn))
    }),
    onAppHello: vi.fn(() => () => undefined),
    onReadStateChanged: vi.fn(() => () => undefined),
    onWorkspaceResourcesChanged: vi.fn(() => () => undefined),
  }
})

import { act } from 'react'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import ProjectSettings from './ProjectSettings'
import { clearCachedShows, initProjectGroupEvents, useProjectGroupsStore } from '@/stores/project-groups'
import { useSettingsStore } from '@/stores/settings'
import { useToastStore } from '@/stores/toast'
import { dropCachedGroupIcon } from './group-icon-cache'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
if (typeof Element !== 'undefined' && typeof Element.prototype.scrollIntoView !== 'function') {
  Element.prototype.scrollIntoView = () => {}
}

function group(id: string, name: string, members: string[], iconKey: string | null): FakeGroup {
  return {
    id,
    name,
    pocWorkspaceId: members[0] ?? null,
    pinned: false,
    color: null,
    sortOrder: 0,
    createdAt: 1,
    updatedAt: 1,
    memberCount: members.length,
    memberWorkspaceIds: members,
    iconKey,
  }
}

function fire(reason: string, groupId?: string): void {
  act(() => {
    for (const fn of bus.projectGroups) fn(reason, groupId)
  })
}

async function advance(ms: number): Promise<void> {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms)
  })
}

function countGets(): Record<string, number> {
  const out: Record<string, number> = {}
  for (const g of daemon.gets) out[g.route] = (out[g.route] ?? 0) + 1
  return out
}

function heading(): string {
  const h = document.querySelector('h2')
  return h?.textContent ?? ''
}

function iconShown(): boolean {
  return Array.from(document.querySelectorAll('img')).some((img) => img.getAttribute('src') === ICON)
}

function loadingShown(): boolean {
  return screen.queryAllByText('Loading…').length > 0
}

async function mountSettings(): Promise<void> {
  render(<ProjectSettings />)
  await advance(1000)
  expect(heading()).toBe('Alpha')
}

beforeAll(() => {
  vi.useFakeTimers()
  initProjectGroupEvents()
})

beforeEach(async () => {
  daemon.groups = [group('g-alpha', 'Alpha', ['ws-a', 'ws-b'], 'k1'), group('g-beta', 'Beta', ['ws-c'], 'k2')]
  daemon.icons = { 'g-alpha': ICON, 'g-beta': ICON }
  daemon.failNext = {}
  dropCachedGroupIcon()
  clearCachedShows()
  useSettingsStore.setState({ initialProjectGroupId: null })
  useProjectGroupsStore.setState({ groups: null, selectedGroupId: 'g-alpha' })
  useToastStore.setState({ toasts: [] })
  void useProjectGroupsStore.getState().fetchGroups()
  await advance(1000)
  daemon.gets = []
  daemon.posts = []
})

afterEach(async () => {
  cleanup()
  await advance(2000)
})

describe('Settings → Projects fetch budget', () => {
  it('a minute of chat (one message a second, any project) fetches nothing for the page', async () => {
    await mountSettings()
    daemon.gets = []
    for (let i = 0; i < 60; i++) {
      fire('message-created', i % 2 === 0 ? 'g-alpha' : 'g-beta')
      await advance(1000)
    }
    const counts = countGets()
    console.log('[perf] GETs during 60 chat messages / 60 s with Settings → Projects open:', JSON.stringify(counts))
    expect(counts['project-group/show'] ?? 0).toBe(0)
    expect(counts['project-group/resources'] ?? 0).toBe(0)
    expect(counts['project-group/icon'] ?? 0).toBe(0)
    expect(counts['project-group/list'] ?? 0).toBe(0)
  })

  it('a change to the open project refetches its show view exactly once', async () => {
    await mountSettings()
    daemon.gets = []
    daemon.groups[0].memberWorkspaceIds.push('ws-z')
    daemon.groups[0].memberCount = 3
    fire('members-changed', 'g-alpha')
    await advance(1000)
    const counts = countGets()
    console.log('[perf] GETs for one members-changed on the open project:', JSON.stringify(counts))
    expect(counts['project-group/show']).toBe(1)
    expect(counts['project-group/icon'] ?? 0).toBe(0)
    expect(screen.getByText('WS-Z')).toBeTruthy()
  })

  it('a frame naming no project (older daemon) still refreshes the open panel, once', async () => {
    await mountSettings()
    daemon.gets = []
    fire('poc-changed')
    await advance(1000)
    const counts = countGets()
    expect(counts['project-group/show']).toBe(1)
    expect(counts['project-group/resources']).toBe(1)
    expect(counts['project-group/icon'] ?? 0).toBe(0)
  })

  it("a change to ANOTHER project fetches nothing for the open one's panel", async () => {
    await mountSettings()
    daemon.gets = []
    fire('members-changed', 'g-beta')
    await advance(1000)
    const counts = countGets()
    expect(counts['project-group/show'] ?? 0).toBe(0)
    expect(counts['project-group/resources'] ?? 0).toBe(0)
    expect(counts['project-group/icon'] ?? 0).toBe(0)
    // The left list still refreshes (member counts live there).
    expect(counts['project-group/list']).toBe(1)
  })

  it('the icon is fetched once and reused; an unchanged iconKey never refetches it', async () => {
    await mountSettings()
    expect(daemon.gets.filter((g) => g.route === 'project-group/icon').length).toBe(1)
    expect(iconShown()).toBe(true)
    // A rename emits groups-changed for this project: the icon is unchanged.
    daemon.gets = []
    daemon.groups[0].name = 'Alpha 2'
    fire('groups-changed', 'g-alpha')
    await advance(1000)
    expect(heading()).toBe('Alpha 2')
    expect(daemon.gets.filter((g) => g.route === 'project-group/icon').length).toBe(0)
    // A new upload changes the key: exactly one refetch.
    daemon.groups[0].iconKey = 'k1b'
    fire('groups-changed', 'g-alpha')
    await advance(1000)
    expect(daemon.gets.filter((g) => g.route === 'project-group/icon').length).toBe(1)
  })
})

describe('Settings → Projects switching', () => {
  it('keeps the previous project on screen until the next one lands, then renders a revisit at once', async () => {
    await mountSettings()
    const t0 = Date.now()
    fireEvent.click(must(document.querySelector('[data-settings-group-id="g-beta"]')))
    // The previous project stays on screen, marked busy and inert, so the
    // panel keeps its height and scroll and can't be edited by mistake.
    expect(heading()).toBe('Alpha')
    expect(must(document.querySelector('[aria-busy="true"]')).className).toContain('pointer-events-none')
    let sawLoading = loadingShown()
    let firstMs = -1
    let iconMs = -1
    for (let t = 0; t < 1000 && (firstMs < 0 || iconMs < 0); t += 5) {
      await advance(5)
      sawLoading = sawLoading || loadingShown()
      if (firstMs < 0 && heading() === 'Beta') firstMs = Date.now() - t0
      if (iconMs < 0 && heading() === 'Beta' && iconShown()) iconMs = Date.now() - t0
    }
    // Back to a project already seen.
    const t1 = Date.now()
    fireEvent.click(must(document.querySelector('[data-settings-group-id="g-alpha"]')))
    let revisitMs = heading() === 'Alpha' ? 0 : -1
    let sawLoadingBack = loadingShown()
    for (let t = 0; t < 1000 && revisitMs < 0; t += 5) {
      await advance(5)
      sawLoadingBack = sawLoadingBack || loadingShown()
      if (heading() === 'Alpha') revisitMs = Date.now() - t1
    }
    const iconGets = daemon.gets.filter((g) => g.route === 'project-group/icon').length
    console.log(
      `[perf] switch (daemon latency ${LATENCY_MS}ms): first visit ${firstMs}ms (picture at ${iconMs}ms), blanked=${sawLoading}; ` +
        `revisit ${revisitMs}ms, blanked=${sawLoadingBack}; icon GETs over 3 renders: ${iconGets}`,
    )
    expect(sawLoading).toBe(false)
    expect(sawLoadingBack).toBe(false)
    expect(revisitMs).toBe(0)
    expect(iconGets).toBe(2) // one per project, never again
  })
})

describe('Settings → Projects optimistic edits', () => {
  it('rename shows the new name at once and keeps it', async () => {
    await mountSettings()
    fireEvent.click(screen.getByTitle('Rename project'))
    const input = document.querySelector('input[type="text"]:not([placeholder])') as HTMLInputElement
    fireEvent.change(input, { target: { value: 'Alpha Prime' } })
    fireEvent.keyDown(input, { key: 'Enter' })
    expect(heading()).toBe('Alpha Prime')
    await advance(1000)
    expect(heading()).toBe('Alpha Prime')
    expect(daemon.posts.map((p) => p.route)).toEqual(['project-group/rename'])
  })

  it('a refused rename rolls back and says why', async () => {
    await mountSettings()
    daemon.failNext['project-group/rename'] = 'name taken'
    fireEvent.click(screen.getByTitle('Rename project'))
    const input = document.querySelector('input[type="text"]:not([placeholder])') as HTMLInputElement
    fireEvent.change(input, { target: { value: 'Beta' } })
    fireEvent.keyDown(input, { key: 'Enter' })
    expect(heading()).toBe('Beta')
    await advance(1000)
    expect(heading()).toBe('Alpha')
    expect(screen.getByText('name taken')).toBeTruthy()
  })

  it('pin flips at once; a refused pin flips back with a toast', async () => {
    await mountSettings()
    const pin = must(document.querySelector('[data-settings-id="project-pinned"]'))
    fireEvent.click(pin)
    expect(pin.getAttribute('aria-checked')).toBe('true')
    await advance(1000)
    expect(pin.getAttribute('aria-checked')).toBe('true')
    daemon.failNext['project-group/pin'] = 'no'
    fireEvent.click(pin)
    expect(pin.getAttribute('aria-checked')).toBe('false')
    await advance(1000)
    expect(pin.getAttribute('aria-checked')).toBe('true')
    expect(useToastStore.getState().toasts.some((t) => t.message.includes('Pin failed'))).toBe(true)
  })

  it('point of contact changes at once; a refused change rolls back', async () => {
    await mountSettings()
    expect(document.body.textContent).toContain('PoC WS-A')
    // SettingDropdown: open, pick WS-B.
    fireEvent.click(screen.getByLabelText('Point of Contact'))
    fireEvent.click(menuOption('WS-B'))
    expect(document.body.textContent).toContain('PoC WS-B')
    await advance(1000)
    expect(document.body.textContent).toContain('PoC WS-B')
    daemon.failNext['project-group/set-poc'] = 'not allowed'
    fireEvent.click(screen.getByLabelText('Point of Contact'))
    fireEvent.click(menuOption('WS-A'))
    expect(document.body.textContent).toContain('PoC WS-A')
    await advance(1000)
    expect(document.body.textContent).toContain('PoC WS-B')
    expect(useToastStore.getState().toasts.some((t) => t.message.includes('not allowed'))).toBe(true)
  })
})

/** The open SettingDropdown menu's row (portaled last into <body>). */
function menuOption(label: string): HTMLElement {
  const buttons = screen
    .getAllByText(label)
    .map((el) => el.closest('button'))
    .filter((b): b is HTMLButtonElement => b !== null)
  return must(buttons[buttons.length - 1] ?? null)
}

function must(el: Element | null): HTMLElement {
  if (!el) throw new Error('missing element')
  return el as HTMLElement
}
