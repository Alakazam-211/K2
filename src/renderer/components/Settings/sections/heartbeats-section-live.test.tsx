// @vitest-environment jsdom
// A scheduled fire emits heartbeat_state_changed with a project path.
// The open workspace list refetches heartbeat/list. It does not patch
// the row from `live`. The global All Heartbeats roster stays mount-only.
// Fail loud — no skip. Does not boot a daemon.
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, waitFor } from '@testing-library/react'

const gets = vi.hoisted(() => ({
  calls: [] as Array<[string, unknown]>,
}))
const tabSubs = vi.hoisted(() => ({
  calls: [] as Array<{ path: string; handlers: Record<string, (event?: unknown) => void> }>,
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'daemon_ws_url') {
      return { state: 'unavailable', reason: 'test env', port: null, token: null }
    }
    return null
  }),
}))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly(async (route: string, params?: unknown) => {
      gets.calls.push([route, params])
      return []
    }),
    daemonCliGetText: primaryOnly(async () => '{}'),
    daemonCliPost: primaryOnly(async () => ({})),
    RecoveringError: class RecoveringError extends Error {},
  }
})
vi.mock('@/lib/heartbeat-launch', () => ({
  launchHeartbeat: vi.fn(async () => undefined),
}))
vi.mock('@/lib/server-capabilities', () => ({
  serverSupports: (cap: string) => cap === 'daemon-broadcasts',
}))
vi.mock('@/lib/daemon-reconnect', () => ({ onDaemonConnected: vi.fn() }))
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(async () => ({ settings: {} })),
  settingsUpdate: vi.fn(async () => ({ settings: {} })),
  settingsReset: vi.fn(async () => ({ settings: {} })),
}))
vi.mock('@/lib/terminal-daemon', () => ({
  terminalListRunning: vi.fn(async () => []),
  terminalKill: vi.fn(async () => undefined),
}))
vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ port: 9999, token: 'tok', host: '127.0.0.1' })),
  daemonHttpBase: vi.fn(() => 'http://127.0.0.1:9999'),
  daemonWsBase: vi.fn(() => 'ws://127.0.0.1:9999'),
  invalidateDaemonWs: vi.fn(),
}))
vi.mock('@/stores/session-events', async () => {
  // Home M1: every subscription takes the server scope first; these
  // mocks fail loudly unless it is the primary scope.
  const { expectPrimaryScope } = await import('@/test-utils/scope')
  return {
    subscribeToWorkspaceTabEvents: (scope: unknown, path: string, handlers: Record<string, (event?: unknown) => void>) => {
      expectPrimaryScope(scope)
      tabSubs.calls.push({ path, handlers })
      return () => undefined
    },
    subscribeToWorkspaceSessionEvents: vi.fn(() => () => undefined),
    onSessionAddedApp: vi.fn(() => () => undefined),
    onSessionRemovedApp: vi.fn(() => () => undefined),
    onAppHello: vi.fn(() => () => undefined),
    onOpenUrl: vi.fn(() => () => undefined),
    onChatHistoryChanged: vi.fn(() => () => undefined),
    onceRecovered: vi.fn(),
  }
})
vi.mock('@/stores/connect-host', () => ({
  useConnectHostStore: {
    getState: () => ({ activeHost: 'local', recovery: { kind: 'connected' } }),
    subscribe: vi.fn(() => () => undefined),
  },
  onActiveHostChange: vi.fn(() => () => undefined),
  activeHostKey: vi.fn(() => 'local'),
}))
vi.mock('@/stores/presets', () => ({
  usePresetsStore: { getState: () => ({ presets: [] }) },
}))
vi.mock('@/lib/workspace-agent', () => ({
  agentDisplayName: vi.fn(async () => 'claude'),
}))
vi.mock('@/components/AIFileEditor/AIFileEditor', () => ({ AIFileEditor: () => null }))
vi.mock('@/components/FileViewerPane/FileViewerPane', () => ({ FileViewerPane: () => null }))
vi.mock('@/hooks/useResolvedAgentCommand', () => ({
  useResolvedAgentCommand: () => ({ command: 'claude', args: [] }),
}))

import { HeartbeatsPanel, scheduleFormError, type HeartbeatRow } from './HeartbeatsSection'
import { heartbeatStatusText } from '@/lib/heartbeat-wait'

const root = join(dirname(fileURLToPath(import.meta.url)), '../../../../../')

function listCalls(): Array<[string, unknown]> {
  return gets.calls.filter((c) => c[0] === 'heartbeat/list')
}

afterEach(() => {
  cleanup()
  gets.calls = []
  tabSubs.calls = []
})

describe('HeartbeatsSection refetches on heartbeat_state_changed', () => {
  it('subscribes for this project and refetches heartbeat/list; it does not patch from live', async () => {
    render(<HeartbeatsPanel projectPath="/ws/proj" agentName="claude" />)
    await waitFor(() => {
      expect(tabSubs.calls.length).toBeGreaterThan(0)
      expect(listCalls().length).toBeGreaterThan(0)
    })
    expect(tabSubs.calls[0].path).toBe('/ws/proj')
    const handler = tabSubs.calls[0].handlers.onHeartbeatStateChanged
    expect(typeof handler).toBe('function')
    const before = listCalls().length
    await act(async () => {
      handler({
        kind: 'heartbeat_state_changed',
        workspacePath: '/ws/proj',
        project: 'proj-id',
        agent: 'daily',
        live: true,
      })
    })
    await waitFor(() => {
      expect(listCalls().length).toBe(before + 1)
    })
    expect(listCalls().at(-1)?.[1]).toEqual({ project: '/ws/proj' })
  })
})

describe('lock 8 boundaries', () => {
  it('the global All Heartbeats roster stays a mount-only load', () => {
    const src = readFileSync(join(root, 'src/renderer/components/Settings/sections/WakeSchedulerSection.tsx'), 'utf8')
    expect(src).not.toContain('onHeartbeatStateChanged')
    expect(src).not.toContain('heartbeat_state_changed')
    expect(src).not.toContain('subscribeToWorkspaceTabEvents')
  })

  it('the first fire and a later inject emit heartbeat_state_changed with the project path', () => {
    const wake = readFileSync(join(root, 'crates/k2-daemon/src/wake_headless.rs'), 'utf8')
    const launch = readFileSync(join(root, 'crates/k2-daemon/src/heartbeat_launch.rs'), 'utf8')
    expect(wake).toContain('emit_heartbeat_live(project_path, pid, hb_name, true)')
    expect(wake).not.toContain('emit_heartbeat_live("")')
    expect(launch).toContain('emit_heartbeat_live(project_path, project_id, &hb.name, true)')
    expect(launch).not.toContain('emit_heartbeat_live("")')
  })
})

// ── S5 (prd-heartbeat-firing-v1 D6, HB32, HB34) ──────────────────────

function addCalls(): Array<[string, unknown]> {
  return gets.calls.filter((c) => c[0] === 'heartbeat/add')
}

describe('S5: Settings → Add requires instructions', () => {
  it('cannot submit an empty Add, and sends the instructions to the daemon', async () => {
    const view = render(<HeartbeatsPanel projectPath="/ws/proj" agentName="claude" />)
    await waitFor(() => {
      expect(listCalls().length).toBeGreaterThan(0)
    })
    fireEvent.click(view.getByTitle('Add heartbeat'))
    fireEvent.change(view.getByPlaceholderText('daily-brief'), { target: { value: 'morning-brief' } })

    const add = view.getByRole('button', { name: 'Add' }) as HTMLButtonElement
    expect(add.disabled).toBe(true)
    fireEvent.click(add)
    expect(addCalls()).toEqual([])

    const box = view.getByLabelText('Instructions')
    fireEvent.change(box, { target: { value: '   \n  ' } })
    expect(add.disabled).toBe(true)

    fireEvent.change(box, { target: { value: 'check inbox' } })
    expect(add.disabled).toBe(false)
    await act(async () => {
      fireEvent.click(add)
    })
    await waitFor(() => {
      expect(addCalls().length).toBe(1)
    })
    expect(addCalls()[0][1]).toEqual({
      project: '/ws/proj',
      name: 'morning-brief',
      frequency: 'daily',
      spec: JSON.stringify({ frequency: 'daily', time: '07:00' }),
      instructions: 'check inbox',
    })
  })
})

describe('S5: schedule form rules', () => {
  it('refuses unknown frequencies and sub-minute or non-whole intervals', () => {
    expect(scheduleFormError({ frequency: 'list' }, 'x', true)).toBe("Choose a frequency ('list' is not one)")
    expect(scheduleFormError({ frequency: 'hourly', every_seconds: 30 }, 'x', false)).toMatch(/at least 1/)
    expect(scheduleFormError({ frequency: 'hourly', every_seconds: Number.NaN }, 'x', false)).toMatch(/whole number/)
    expect(scheduleFormError({ frequency: 'hourly', every_seconds: 60 }, 'x', false)).toBeNull()
    expect(scheduleFormError({ frequency: 'hourly' }, 'x', false)).toBeNull()
  })

  it('requires instructions on Add only', () => {
    expect(scheduleFormError({ frequency: 'daily' }, '', false)).toMatch(/instructions/)
    expect(scheduleFormError({ frequency: 'daily' }, '', true)).toBeNull()
  })
})

describe('S4: Settings rows use the drawer formatter (HB31)', () => {
  const base: HeartbeatRow = {
    id: 'id',
    projectId: 'p',
    name: 'hb',
    frequency: 'daily',
    specJson: '{}',
    wakeupPath: '.k2/heartbeats/hb/WAKEUP.md',
    enabled: true,
    lastFired: null,
    createdAt: 0,
    useWorkspaceSession: true,
    lastSessionId: null,
    consecutiveFailures: 0,
    nextRetryAt: null,
    disabledReason: null,
    scheduleError: null,
  }
  const now = Date.parse('2026-10-07T20:00:00Z')

  it('a Settings row names an empty WAKEUP.md, an invalid schedule and the next fire', () => {
    const row: HeartbeatRow = { ...base, frequency: 'list' }
    expect(heartbeatStatusText(row, { now, nextFire: false })).toBeNull()
    expect(heartbeatStatusText({ ...base, waitReason: 'wakeup_empty' }, { now, nextFire: true })).toBe(
      'waiting: WAKEUP.md is empty',
    )
    expect(
      heartbeatStatusText(
        {
          ...base,
          scheduleError: "unknown frequency 'list'",
          waitReason: 'schedule_error',
          waitDetail: "unknown frequency 'list'",
        },
        { now, nextFire: true },
      ),
    ).toBe("invalid schedule: unknown frequency 'list'")
    expect(
      heartbeatStatusText(
        { ...base, waitReason: 'scheduled', nextFireAt: '2026-10-07T20:12:04Z' },
        { now, nextFire: true },
      ),
    ).toBe('in 12m 04s')
    expect(heartbeatStatusText(base, { now, nextFire: false })).toBeNull()
  })

  it('Settings → Heartbeats and the Wake Scheduler list render the shared line', () => {
    for (const f of ['HeartbeatsSection.tsx', 'WakeSchedulerSection.tsx']) {
      const src = readFileSync(join(root, 'src/renderer/components/Settings/sections', f), 'utf8')
      expect(src, f).toContain('<HeartbeatStatusLine')
      expect(src, f).toContain("serverSupports('heartbeat-next-fire')")
      expect(src, f).not.toContain('describeHeartbeatWait')
    }
  })
})
