// @vitest-environment jsdom
// S5 (prd-heartbeat-firing-v1 HB33/HB35): the drawer row shows the
// daemon's wait reason instead of a guessed "Next run", and an empty
// WAKEUP.md row stays enabled. Fail loud — no skips, no daemon.
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async () => {
    throw new Error('the row must not call the daemon while rendering')
  }),
}))
vi.mock('@/lib/heartbeat-launch', () => ({ launchHeartbeat: vi.fn(async () => undefined) }))
vi.mock('@/components/common/HeartbeatSessionPicker', () => ({ openHeartbeatTarget: vi.fn() }))
vi.mock('@/stores/heartbeat-sessions', () => ({
  useHeartbeatSessionsStore: { getState: () => ({ refresh: vi.fn() }) },
}))

import { HeartbeatEntryRow } from './HeartbeatEntry'
import type { HeartbeatEntry, HeartbeatRow } from '@/stores/heartbeat-sessions'
import { renderInRoom, testRoom } from '@/test-utils/room'

afterEach(() => cleanup())

function entry(row: Partial<HeartbeatRow>): HeartbeatEntry {
  return {
    row: {
      id: 'id',
      projectId: 'p',
      name: 'hb',
      frequency: 'hourly',
      specJson: JSON.stringify({ frequency: 'hourly', every_seconds: 60 }),
      wakeupPath: '.k2/heartbeats/hb/WAKEUP.md',
      enabled: true,
      lastFired: null,
      lastSessionId: null,
      archivedAt: null,
      createdAt: 0,
      ...row,
    },
    state: 'scheduled',
  } as HeartbeatEntry
}

function renderRow(e: HeartbeatEntry): ReturnType<typeof renderInRoom> {
  return renderInRoom(testRoom({ tabs: {} }), <HeartbeatEntryRow entry={e} projectPath="/ws/proj" />)
}

describe('drawer row wait reason', () => {
  it('an empty WAKEUP.md row stays enabled and says it is waiting, not "now"', () => {
    const view = renderRow(entry({ waitReason: 'wakeup_empty', waitDetail: 'needs instructions' }))
    expect(view.getByText('waiting: WAKEUP.md is empty')).toBeTruthy()
    expect(view.queryByText(/Next run/)).toBeNull()
    expect(view.getByRole('switch').getAttribute('aria-checked')).toBe('true')
  })

  it("Reggie's row reads invalid schedule with the daemon's detail", () => {
    const view = renderRow(
      entry({
        frequency: 'list',
        specJson: '{}',
        waitReason: 'schedule_error',
        waitDetail: "unknown frequency 'list'",
        scheduleError: "unknown frequency 'list'",
      }),
    )
    expect(view.getByText("invalid schedule: unknown frequency 'list'")).toBeTruthy()
    expect(view.queryByText(/Next run/)).toBeNull()
  })

  it('a row with nothing in the way keeps its Next run line', () => {
    const view = renderRow(entry({ lastFired: new Date().toISOString() }))
    expect(view.getByText(/^Next run:/)).toBeTruthy()
  })
})
