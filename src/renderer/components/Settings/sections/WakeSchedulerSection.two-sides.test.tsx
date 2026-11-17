// @vitest-environment jsdom
// Settings → Logs → Heartbeats, two sides like Settings → Tunnel (the shared
// halves, controls/halves): Your heartbeats on the left, and How heartbeats
// run (the scheduler's status, the wake and battery switches) then Recent
// runs on the right, each side scrolling on its own, so a long list never
// pushes the wake switches or the runs far down. Under
// HALVES_MIN_WIDTH CSS px of body the sides stack, the left one first, in
// one scroll. Every control the page had stays, with its words.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render, screen, waitFor, within } from '@testing-library/react'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'

const h = vi.hoisted(() => ({
  invokes: {} as Record<string, unknown>,
  routes: {} as Record<string, unknown>,
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => h.invokes[cmd] ?? null),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_scope: unknown, route: string) => (route in h.routes ? h.routes[route] : null)),
  daemonCliPost: vi.fn(async () => ({ success: true })),
  daemonCliPostQuery: vi.fn(async () => ({})),
}))
vi.mock('@/kessel/server-scope', () => ({
  primaryScope: () => ({ serverSupports: () => true }),
}))
vi.mock('@/stores/connect-host', () => ({
  useConnectHostStore: { getState: () => ({ activeHost: 'local' }) },
}))
vi.mock('@/stores/tabs', () => ({ useTabsStore: {} }))
vi.mock('./HeartbeatsSection', () => ({ WakeupEditor: () => null }))
vi.mock('@/components/common/HeartbeatSessionPicker', () => ({
  HeartbeatSessionPicker: ({ ariaLabel }: { ariaLabel?: string }) => <button type="button" aria-label={ariaLabel}>Own session</button>,
  openHeartbeatTarget: vi.fn(),
}))
vi.mock('@/components/common/HeartbeatStatusLine', () => ({ HeartbeatStatusLine: () => null }))
vi.mock('@/hooks/useHeartbeatDueRefetch', () => ({ useHeartbeatDueRefetch: () => undefined }))
vi.mock('@/lib/heartbeat-delivery', () => ({
  applyDeliveryTarget: (r: unknown) => r,
  deriveDeliveryTarget: () => ({ mode: 'own' }),
  setHeartbeatSession: vi.fn(async () => undefined),
}))

import { WakeSchedulerSection } from './WakeSchedulerSection'
import { HALVES_MIN_WIDTH } from '../controls/halves'

const row = (i: number) => ({
  id: `hb${i}`,
  projectId: 'p1',
  projectName: 'atlas',
  projectPath: '/Users/alex/Projects/atlas',
  name: `check-${String(i).padStart(2, '0')}`,
  frequency: 'daily',
  specJson: '{"time":"02:00"}',
  wakeupPath: `/Users/alex/Projects/atlas/.k2/heartbeats/check-${i}/WAKEUP.md`,
  enabled: true,
  lastFired: null,
  useWorkspaceSession: false,
  lastSessionId: null,
  consecutiveFailures: 0,
  disabledReason: null,
  scheduleError: null,
})
const fire = (id: number, decision: string) => ({
  id,
  projectId: 'p1',
  projectName: 'atlas',
  agentName: null,
  scheduleName: 'check-01',
  firedAt: '2026-11-10T09:00:00.000Z',
  mode: 'heartbeat',
  decision,
  reason: null,
  durationMs: null,
})
const STATUS = {
  lastTickAt: '2026-11-10T09:00:00.000Z',
  staleSecs: 20,
  enabledCount: 14,
  transportInstalled: true,
  wakeMode: 'daemon',
  ticker: { intervalSecs: 60, lastTickAt: '2026-11-10T09:00:00.000Z', staleSecs: 20 },
  wakeForHeartbeats: true,
  wakeOnBattery: false,
  wake: { state: 'ok', reason: '', nextWakeAt: null, nextFireAt: null, platform: 'macos', batteryFloorPercent: 20 },
}

let bodyWidth = 1000
class FakeResizeObserver {
  observe(): void {}
  disconnect(): void {}
  unobserve(): void {}
}
const realRect = Element.prototype.getBoundingClientRect

beforeEach(() => {
  bodyWidth = 1000
  h.invokes = {
    k2_heartbeat_list_all: Array.from({ length: 14 }, (_, i) => row(i + 1)),
    k2_heartbeat_fires_list_all: [fire(2, 'error'), fire(1, 'fired')],
  }
  h.routes = { 'heartbeat/scheduler-status': STATUS }
  vi.stubGlobal('ResizeObserver', FakeResizeObserver)
  Element.prototype.getBoundingClientRect = function (this: Element): DOMRect {
    if ((this as HTMLElement).dataset?.testid === 'heartbeats-halves') {
      return { width: bodyWidth, height: 600, top: 0, left: 0, right: bodyWidth, bottom: 600, x: 0, y: 0, toJSON: () => ({}) } as DOMRect
    }
    return realRect.call(this)
  }
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
  Element.prototype.getBoundingClientRect = realRect
})

async function loaded(): Promise<void> {
  await screen.findByTestId('heartbeat-row-hb14')
  await screen.findByRole('switch', { name: 'Also wake on battery' })
  await waitFor(() => expect(document.querySelectorAll('[data-fire-row]').length).toBe(2))
}

function headingsIn(el: HTMLElement): string[] {
  return within(el).queryAllByRole('heading', { level: 3 }).map((x) => x.textContent ?? '')
}

/** Every control on the page, by test id, settings id and accessible name. */
function controls(root: HTMLElement): string[] {
  const out: string[] = []
  for (const el of Array.from(root.querySelectorAll('[data-testid]'))) {
    const id = el.getAttribute('data-testid') ?? ''
    // The layout's own wrappers are new; the page's sections and rows are not.
    if (!id.startsWith('heartbeats-halves') && id !== 'heartbeats-left' && id !== 'heartbeats-right') out.push(`testid:${id}`)
  }
  for (const el of Array.from(root.querySelectorAll('[data-settings-id]'))) out.push(`setting:${el.getAttribute('data-settings-id')}`)
  for (const el of Array.from(root.querySelectorAll('button, [role="switch"], [role="radio"]'))) {
    const role = el.getAttribute('role') ?? 'button'
    // A run row's button reads its time in the local zone; count it by run.
    const run = el.closest('[data-fire-row]')
    const name = run ? `run ${run.getAttribute('data-tone')}` : (el.getAttribute('aria-label') ?? el.textContent ?? '')
    out.push(`${role}:${name.trim()}`)
  }
  return out.sort()
}

/** The page's controls before the two sides, with 14 heartbeats and two
 *  runs: the same set must still be there. */
const TODAY: string[] = (() => {
  const out = [
    'testid:heartbeats-checking',
    'testid:heartbeats-list',
    'testid:heartbeats-runs',
    'testid:heartbeats-runs-rows',
    'testid:heartbeats-status',
    'setting:heartbeats',
    'setting:wake-scheduler.battery',
    'setting:wake-scheduler.wake',
    'switch:Also wake on battery',
    'switch:Wake this computer for heartbeats',
    'radio:All',
    'radio:Problems (1)',
  ]
  for (let i = 1; i <= 14; i++) {
    const name = `check-${String(i).padStart(2, '0')}`
    out.push(
      `testid:heartbeat-row-hb${i}`,
      `switch:Run ${name} in atlas`,
      `button:Where ${name} runs`,
      `button:Open the chat ${name} runs in`,
      `button:Edit the instructions for ${name}`,
    )
  }
  // Two run rows, each a button that opens it.
  out.push('button:run problem', 'button:run ok')
  return out
})()

describe('Heartbeats: two sides', () => {
  it('wide: Your heartbeats on the left, How heartbeats run then Recent runs on the right; each side scrolls on its own', async () => {
    render(<WakeSchedulerSection />)
    await loaded()
    const halves = screen.getByTestId('heartbeats-halves')
    expect(halves.getAttribute('data-layout')).toBe('split')
    expect(halves.className).toContain('grid-cols-2')
    const left = screen.getByTestId('heartbeats-left')
    const right = screen.getByTestId('heartbeats-right')
    expect(headingsIn(left)).toEqual(['Your heartbeats'])
    expect(headingsIn(right)).toEqual(['How heartbeats run', 'Recent runs'])
    expect(left.contains(screen.getByTestId('heartbeat-row-hb14'))).toBe(true)
    expect(right.contains(screen.getByTestId('heartbeats-runs-rows'))).toBe(true)
    expect(within(right).getByRole('switch', { name: 'Wake this computer for heartbeats' })).toBeTruthy()
    expect(within(right).getByRole('switch', { name: 'Also wake on battery' })).toBeTruthy()
    expect(left.className).toContain('overflow-y-auto')
    expect(right.className).toContain('overflow-y-auto')
    // Each side's bottom fade ends on its bottom edge: no bottom padding on
    // the side, the space under the last row held by the fade itself.
    for (const side of [left, right]) {
      expect(side.className).not.toMatch(/(^|\s)(pb|py|p)-/)
      const last = side.lastElementChild as HTMLElement
      expect(last.getAttribute('data-scroll-edge')).toBe('bottom')
      expect(last.className).toContain('h-8')
    }
    // Reading and keyboard order: the left side first.
    expect(left.compareDocumentPosition(right) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
  })

  it(`narrow (< ${HALVES_MIN_WIDTH}px): the sides stack, Your heartbeats, then How heartbeats run, then Recent runs, in one scroll`, async () => {
    bodyWidth = HALVES_MIN_WIDTH - 1
    render(<WakeSchedulerSection />)
    await loaded()
    const halves = screen.getByTestId('heartbeats-halves')
    expect(halves.getAttribute('data-layout')).toBe('stacked')
    expect(halves.className).toContain('overflow-y-auto')
    expect(halves.className).not.toContain('grid-cols-2')
    const left = screen.getByTestId('heartbeats-left')
    const right = screen.getByTestId('heartbeats-right')
    expect(left.className).not.toContain('overflow-y-auto')
    expect(right.className).not.toContain('overflow-y-auto')
    const order = within(halves).queryAllByRole('heading', { level: 3 }).map((x) => x.textContent)
    expect(order).toEqual(['Your heartbeats', 'How heartbeats run', 'Recent runs'])
    // The scheduler's status (and its not-checking warning) sits above the log.
    const status = screen.getByTestId('heartbeats-status')
    expect(status.compareDocumentPosition(screen.getByTestId('heartbeats-runs')) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
  })

  it('the page title stays above both sides, and nothing around them scrolls', async () => {
    render(<WakeSchedulerSection />)
    await loaded()
    const halves = screen.getByTestId('heartbeats-halves')
    const title = screen.getByRole('heading', { level: 2, name: /Heartbeats/ })
    expect(halves.contains(title)).toBe(false)
    expect(title.compareDocumentPosition(halves) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    for (let el = halves.parentElement; el; el = el.parentElement) {
      expect(el.className).not.toContain('overflow-y-auto')
    }
  })

  it('Heartbeats is out of Beta: no Beta badge on the page or on a workspace’s Heartbeats', async () => {
    const { container } = render(<WakeSchedulerSection />)
    await loaded()
    expect(screen.getByRole('heading', { level: 2 }).textContent).toBe('Heartbeats')
    expect(container.querySelector('[data-beta-tag]')).toBeNull()
    expect(within(container).queryByText(/^beta$/i)).toBeNull()
    const panel = readFileSync(join(__dirname, 'HeartbeatsSection.tsx'), 'utf8')
    expect(panel).not.toContain('<BetaTag')
  })

  it('Settings gives the page its whole pane, with no padding or scroll of its own', () => {
    const src = readFileSync(join(__dirname, '..', 'Settings.tsx'), 'utf8')
    const at = src.indexOf("? 'overflow-hidden p-0'")
    const list = src.slice(src.lastIndexOf('data-material="tile"', at), at)
    expect(list).toContain("activeSection === 'wake-scheduler'")
  })

  it('keeps every control the page had, wide and narrow; the only new ones are the list’s search', async () => {
    const SEARCH = [
      'testid:heartbeats-search',
      'setting:wake-scheduler.search',
      'radio:All (14)',
      'radio:On (14)',
      'radio:Paused (0)',
      'radio:Needs attention (0)',
    ]
    const wide = render(<WakeSchedulerSection />)
    await loaded()
    expect(controls(wide.container)).toEqual([...TODAY, ...SEARCH].sort())
    cleanup()
    bodyWidth = HALVES_MIN_WIDTH - 1
    const narrow = render(<WakeSchedulerSection />)
    await loaded()
    expect(controls(narrow.container)).toEqual([...TODAY, ...SEARCH].sort())
  })
})
