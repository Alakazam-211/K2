// @vitest-environment jsdom
// A scheduled fire emits heartbeat_state_changed with a project path.
// The open workspace list refetches heartbeat/list. It does not patch
// the row from `live`. The global All Heartbeats roster stays mount-only.
// Fail loud — no skip. Does not boot a daemon.
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, render, waitFor } from '@testing-library/react'

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

import { HeartbeatsPanel } from './HeartbeatsSection'

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
