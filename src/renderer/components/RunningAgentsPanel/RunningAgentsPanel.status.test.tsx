// @vitest-environment jsdom
//
// plan-agent-status-unify-v1 part 2: Running agents draws each session in
// the shared status model (working slot, then status-or-live), not
// ActivityMark squares, the braille spinner or status words.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  running: [] as unknown[],
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))
vi.mock('@/lib/daemon-cli', async (orig) => {
  const real = await orig<typeof import('@/lib/daemon-cli')>()
  return {
    ...real,
    daemonCliGet: vi.fn(async (_scope: unknown, route: string) => {
      if (route === 'agents/running') return h.running
      throw new Error(`unexpected route ${route}`)
    }),
  }
})
vi.mock('@/lib/terminal-daemon', async (orig) => {
  const real = await orig<typeof import('@/lib/terminal-daemon')>()
  return { ...real, terminalListRunning: vi.fn(async () => []) }
})

import { act, cleanup, render, screen } from '@testing-library/react'
import RunningAgentsPanel from './RunningAgentsPanel'
import { useRunningAgentsStore } from '@/stores/running-agents'
import { primaryScope } from '@/kessel/server-scope'
import { __resetActivityForTests, __seedActivityForTests, type ActivityRow } from '@/stores/activity'
import type { ActivityDisplay } from '@/stores/session-events'

function row(sessionId: string, agentName: string, display: ActivityDisplay, extra: Partial<ActivityRow> = {}): ActivityRow {
  return {
    sessionId,
    agentName,
    projectId: 'p1',
    workspacePath: '/ws',
    harness: 'claude',
    display,
    lead: { state: display === 'working' ? 'working' : display === 'waiting' ? 'waiting' : 'idle', outcome: 'none', since: 0, promptId: null },
    children: { subagents: 0, shells: 0, monitors: 0, crons: 0, unknown: 0, owed: 0, waiting: 0 },
    turnStartedAt: null,
    evidenceAt: null,
    evidenceSource: null,
    reason: 'idle',
    staleSince: null,
    confirmed: true,
    rev: 1,
    ...extra,
  }
}

beforeEach(() => {
  __resetActivityForTests()
  h.running = []
})

afterEach(() => {
  cleanup()
  useRunningAgentsStore.getState().close()
  __resetActivityForTests()
})

async function openPanel(): Promise<void> {
  useRunningAgentsStore.getState().open()
  render(<RunningAgentsPanel />)
  await act(async () => {
    await Promise.resolve()
    await Promise.resolve()
  })
}

function statusOf(name: string): { status: string | null; working: boolean } {
  const label = screen.getByText(name)
  const rowEl = label.closest('[data-index]')
  if (!(rowEl instanceof HTMLElement)) throw new Error(`no row for ${name}`)
  const st = rowEl.querySelector('[data-running-agent-status]')
  if (!(st instanceof HTMLElement)) throw new Error(`no status for ${name}`)
  return { status: st.getAttribute('data-running-agent-status'), working: !!st.querySelector('[data-agent-working]') }
}

describe('Running agents: the shared status slots', () => {
  it('working + needs you, done, a row-less listed session (live), and no words or old marks', async () => {
    h.running = [
      { terminalId: 's-a', cwd: '/ws/a', command: 'claude', agentName: 'tab-a', workspaceName: 'alpha' },
      { terminalId: 's-b', cwd: '/ws/b', command: 'codex', agentName: 'tab-b', workspaceName: 'bravo' },
      { terminalId: 's-c', cwd: '/ws/c', command: 'zsh', agentName: 'tab-c', workspaceName: 'charlie' },
    ]
    __seedActivityForTests(primaryScope(), {
      supported: true,
      rows: [
        row('s-a', 'tab-a', 'waiting', { lead: { state: 'working', outcome: 'none', since: 0, promptId: null } }),
        row('s-b', 'tab-b', 'idle'),
      ],
      unseenDone: new Map([['s-b', { at: 1, agentName: 'tab-b', projectId: 'p1', workspacePath: '/ws', outcome: 'success' }]]),
    })
    await openPanel()
    await screen.findByText('alpha')
    expect(statusOf('alpha')).toEqual({ status: 'needs-you', working: true })
    expect(statusOf('bravo')).toEqual({ status: 'done', working: false })
    expect(statusOf('charlie')).toEqual({ status: 'live', working: false })
    expect(document.querySelector('[data-activity]')).toBeNull()
    expect(document.querySelector('.braille-spinner')).toBeNull()
    expect(screen.queryByText('Idle')).toBeNull()
    expect(screen.queryByText('Needs you')).toBeNull()
  })
})
