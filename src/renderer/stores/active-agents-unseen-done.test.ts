// F4 — unseen-done state machine (.k2/notes/orange-dot-done-sound.md).
// A pane that transitions working|permission → idle while the user ISN'T
// looking gets marked unseen-done (→ Active-bar amber dot) and chimes —
// after a 4s debounce so Claude's tool-boundary working→idle→working
// flickers never mark. Viewing the pane (markSeen) or re-entering working
// clears the mark; the first ~5s after a pane's first activity are
// suppressed (launch banners flicker).
//
// vitest env is node (no Tauri / daemon). Mock the load-time boundaries so
// importing the store is inert, then drive the store directly under fake
// timers.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async () => []),
  daemonCliPost: vi.fn(async () => ({})),
}))
vi.mock('@/lib/daemon-reconnect', () => ({
  onDaemonConnected: vi.fn(),
}))
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(async () => ({})),
  settingsUpdate: vi.fn(async () => ({})),
  settingsReset: vi.fn(async () => ({})),
}))
vi.mock('@/lib/terminal-daemon', () => ({
  terminalCreate: vi.fn(),
  terminalExists: vi.fn(async () => false),
  terminalListRunning: vi.fn(async () => []),
}))
// The chime is the store's only side output — spy on it. The sound module
// itself (setting gate + throttle) has its own test.
vi.mock('@/lib/completion-sound', () => ({
  playCompletionSound: vi.fn(),
}))

const touchInteraction = vi.fn()
let activeProjectId: string | null = null
vi.mock('./projects', () => ({
  useProjectsStore: {
    getState: () => ({
      activeProjectId,
      touchInteraction,
      projects: [],
    }),
  },
}))

import { playCompletionSound } from '@/lib/completion-sound'
import {
  useActiveAgentsStore,
  projectHasUnseenDone,
  __resetAgentStateForHostSwitch,
} from './active-agents'
import { useTabsStore } from './tabs'
import { useWindowFocusStore } from '@/stores/window-focus'

const PANE = 'term-pane-1'
const PROJECT = 'proj-a'

/** Surface PANE as the active item of the active tab in group 0. */
function surfacePaneVisible(): void {
  useTabsStore.setState({
    tabs: [
      {
        id: 'tab-1',
        title: 'Tab',
        mosaicTree: PANE,
        paneGroups: new Map([
          [
            PANE,
            {
              id: PANE,
              items: [{ id: 'item-1', type: 'terminal', data: { terminalId: PANE } }],
              activeItemIndex: 0,
            },
          ],
        ]),
      } as never,
    ],
    activeTabId: 'tab-1',
    extraGroups: [],
  })
}

/** Drive a hook-observed run long enough to clear the spawn grace, then stop. */
function workThenStop(): void {
  const store = useActiveAgentsStore.getState()
  store.handleLifecycleEvent(PANE, '', 'start')
  vi.advanceTimersByTime(6_000) // past the 5s spawn grace
  store.handleLifecycleEvent(PANE, '', 'stop')
}

describe('F4 — unseen-done state machine', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    vi.mocked(playCompletionSound).mockClear()
    touchInteraction.mockClear()
    activeProjectId = PROJECT
    // Clears the store maps AND the module-level debounce timers /
    // spawn-grace anchors between tests.
    __resetAgentStateForHostSwitch()
    useTabsStore.setState({ tabs: [], activeTabId: null, extraGroups: [] })
    useWindowFocusStore.setState({ isFocused: true })
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('0.40.39 — a client false idle while the DAEMON still says working must not chime; the daemon completion does', () => {
    const store = useActiveAgentsStore.getState()
    // Agent starts; daemon truth arrives (visibility-independent).
    store.handleLifecycleEvent(PANE, '', 'start')
    store.applyDaemonActivity({
      workspacePath: '/x', agentName: `tab-${PANE}`, paneGroupId: PANE, status: 'working',
    })
    vi.advanceTimersByTime(6_000) // past spawn grace

    // The switch-away false idle: the parked pane's idle watcher writes
    // idle client-side while the agent is STILL working per the daemon.
    store.recordTitleActivity(PANE, false)
    vi.advanceTimersByTime(10_000)
    expect(playCompletionSound).not.toHaveBeenCalled()
    expect(useActiveAgentsStore.getState().unseenDone.has(PANE)).toBe(false)

    // True completion: daemon working→idle → chime after the debounce.
    useActiveAgentsStore.getState().applyDaemonActivity({
      workspacePath: '/x', agentName: `tab-${PANE}`, paneGroupId: PANE, status: 'idle',
    })
    vi.advanceTimersByTime(4_000)
    expect(playCompletionSound).toHaveBeenCalledTimes(1)
    expect(useActiveAgentsStore.getState().unseenDone.has(PANE)).toBe(true)
  })

  it('0.40.39 — a first-ever daemon idle (session never observed working) never chimes', () => {
    useActiveAgentsStore.getState().applyDaemonActivity({
      workspacePath: '/x', agentName: `tab-${PANE}`, paneGroupId: PANE, status: 'idle',
    })
    vi.advanceTimersByTime(10_000)
    expect(playCompletionSound).not.toHaveBeenCalled()
  })

  it('a hook stop while the pane is not visible marks unseen-done after the 4s debounce and chimes', () => {
    workThenStop()

    // Not yet — the debounce must survive first.
    expect(useActiveAgentsStore.getState().unseenDone.has(PANE)).toBe(false)

    vi.advanceTimersByTime(4_000)

    const s = useActiveAgentsStore.getState()
    expect(s.unseenDone.has(PANE)).toBe(true)
    expect(projectHasUnseenDone(s.unseenDone, s.paneProjectMap, PROJECT)).toBe(true)
    expect(playCompletionSound).toHaveBeenCalledTimes(1)
  })

  it('re-entering working within the debounce cancels the mark (tool-boundary flicker)', () => {
    workThenStop()
    vi.advanceTimersByTime(2_000)
    useActiveAgentsStore.getState().handleLifecycleEvent(PANE, '', 'start')

    vi.advanceTimersByTime(10_000)

    expect(useActiveAgentsStore.getState().unseenDone.has(PANE)).toBe(false)
    expect(playCompletionSound).not.toHaveBeenCalled()
  })

  it('a visible, focused pane never marks unseen-done — no dot, no chime', () => {
    surfacePaneVisible()
    workThenStop()
    vi.advanceTimersByTime(4_000)

    expect(useActiveAgentsStore.getState().unseenDone.has(PANE)).toBe(false)
    expect(playCompletionSound).not.toHaveBeenCalled()
  })

  it('an unfocused window counts as not-looking even when the tab is active', () => {
    surfacePaneVisible()
    useWindowFocusStore.setState({ isFocused: false })
    workThenStop()
    vi.advanceTimersByTime(4_000)

    expect(useActiveAgentsStore.getState().unseenDone.has(PANE)).toBe(true)
    expect(playCompletionSound).toHaveBeenCalledTimes(1)
  })

  it('markSeen clears the mark (the pane became visible-and-focused)', () => {
    workThenStop()
    vi.advanceTimersByTime(4_000)
    expect(useActiveAgentsStore.getState().unseenDone.has(PANE)).toBe(true)

    useActiveAgentsStore.getState().markSeen(PANE)

    const s = useActiveAgentsStore.getState()
    expect(s.unseenDone.has(PANE)).toBe(false)
    expect(projectHasUnseenDone(s.unseenDone, s.paneProjectMap, PROJECT)).toBe(false)
  })

  it('spawn grace — a stop within ~5s of first activity never marks (launch-banner flicker)', () => {
    const store = useActiveAgentsStore.getState()
    store.handleLifecycleEvent(PANE, '', 'start')
    vi.advanceTimersByTime(1_000)
    store.handleLifecycleEvent(PANE, '', 'stop')

    vi.advanceTimersByTime(10_000)

    expect(useActiveAgentsStore.getState().unseenDone.has(PANE)).toBe(false)
    expect(playCompletionSound).not.toHaveBeenCalled()
  })

  it('scan-driven title idle arms too — pinned-chat completions dot without hooks', () => {
    const store = useActiveAgentsStore.getState()
    store.recordTitleActivity(PANE, true)
    vi.advanceTimersByTime(6_000)
    store.recordTitleActivity(PANE, false)
    vi.advanceTimersByTime(4_000)

    const s = useActiveAgentsStore.getState()
    expect(s.unseenDone.has(PANE)).toBe(true)
    expect(projectHasUnseenDone(s.unseenDone, s.paneProjectMap, PROJECT)).toBe(true)
    expect(playCompletionSound).toHaveBeenCalledTimes(1)
  })

  it('re-entering working clears an ALREADY-SET mark', () => {
    workThenStop()
    vi.advanceTimersByTime(4_000)
    expect(useActiveAgentsStore.getState().unseenDone.has(PANE)).toBe(true)

    useActiveAgentsStore.getState().handleLifecycleEvent(PANE, '', 'start')

    expect(useActiveAgentsStore.getState().unseenDone.has(PANE)).toBe(false)
  })
})
