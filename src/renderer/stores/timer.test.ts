// Plan B pilot — vitest coverage for the timer store's DB-backed time-entry
// actions after migrating them OFF the Tauri `timer_*` invoke proxy ONTO the
// host-aware `daemonCli*` HTTP layer.
//
// What this asserts:
//   - fetchEntries   → GET  `timer/entries-list`  with snake_case params
//   - saveEntry      → POST `timer/create`        + emits `sync:timer-entries`
//   - deleteEntry    → POST `timer/delete`        + emits `sync:timer-entries`
//   - exportEntries  → GET  `timer/entries-export` via the RAW-TEXT variant
//   - a failed mutation surfaces an error toast and does NOT emit sync
//
// The timer store has an import-time side effect (`initFromSettings()` →
// `settingsGet()`), so every dependency is mocked via hoisted `vi.mock`
// BEFORE the store import.

import { describe, it, expect, beforeEach, vi } from 'vitest'

// ── Mock the host-aware daemon-cli layer (the thing we migrated TO) ──────
const daemonCliGet = vi.fn()
const daemonCliGetText = vi.fn()
const daemonCliPost = vi.fn()
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (...args: unknown[]) => daemonCliGet(...args),
  daemonCliGetText: (...args: unknown[]) => daemonCliGetText(...args),
  daemonCliPost: (...args: unknown[]) => daemonCliPost(...args),
}))

// ── Mock the cross-window emit bus ───────────────────────────────────────
const emitMock = vi.fn(() => Promise.resolve())
vi.mock('@tauri-apps/api/event', () => ({
  emit: (...args: unknown[]) => emitMock(...args),
}))

// ── Mock the Tauri core invoke (broadcast_sync runtime-state path) ───────
const invokeMock = vi.fn(() => Promise.resolve())
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}))

// ── Mock daemon-settings so the import-time init resolves cleanly ────────
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(() => Promise.resolve({ timer: {} })),
  settingsUpdate: vi.fn(() => Promise.resolve({})),
}))

// ── Mock the daemon-reconnect bus (no-op listener registration) ──────────
vi.mock('@/lib/daemon-reconnect', () => ({
  onDaemonConnected: vi.fn(),
}))

// ── Toast spy (saveEntry failure path raises an error toast) ─────────────
const addToast = vi.fn()
vi.mock('@/stores/toast', () => ({
  useToastStore: { getState: () => ({ addToast }) },
}))

// ── Projects store (saveEntry/stopTimerSilently read activeProjectId) ────
vi.mock('@/stores/projects', () => ({
  useProjectsStore: { getState: () => ({ activeProjectId: 'proj-1' }) },
}))

import { useTimerStore, type TimeEntry } from './timer'

function resetStore(): void {
  useTimerStore.setState({
    status: 'idle',
    startTime: null,
    pausedElapsed: 0,
    resumeTime: null,
    targetDurationMs: null,
    showMemoDialog: false,
    stoppedElapsed: null,
    entries: [],
  })
}

describe('timer store — Plan B host-aware migration', () => {
  beforeEach(() => {
    daemonCliGet.mockReset()
    daemonCliGetText.mockReset()
    daemonCliPost.mockReset()
    emitMock.mockClear()
    invokeMock.mockClear()
    addToast.mockClear()
    resetStore()
  })

  it('fetchEntries GETs timer/entries-list with snake_case params', async () => {
    const rows: TimeEntry[] = [
      {
        id: 'e1',
        projectId: 'proj-1',
        startTime: 100,
        endTime: 160,
        durationSeconds: 60,
        memo: null,
        createdAt: 100,
      },
    ]
    daemonCliGet.mockResolvedValueOnce(rows)

    await useTimerStore.getState().fetchEntries(10, 20, 'proj-1')

    expect(daemonCliGet).toHaveBeenCalledTimes(1)
    expect(daemonCliGet).toHaveBeenCalledWith('timer/entries-list', {
      start: 10,
      end: 20,
      project_id: 'proj-1',
    })
    expect(useTimerStore.getState().entries).toEqual(rows)
  })

  it('fetchEntries passes undefined params through unchanged', async () => {
    daemonCliGet.mockResolvedValueOnce([])
    await useTimerStore.getState().fetchEntries()
    expect(daemonCliGet).toHaveBeenCalledWith('timer/entries-list', {
      start: undefined,
      end: undefined,
      project_id: undefined,
    })
  })

  it('saveEntry POSTs timer/create (camelCase body) and emits sync:timer-entries', async () => {
    daemonCliPost.mockResolvedValueOnce({ success: true })

    // Drive saveEntry off a stopped timer with a known elapsed.
    useTimerStore.setState({ startTime: 1_000_000, stoppedElapsed: 90_000 })
    await useTimerStore.getState().saveEntry('worked on it')

    expect(daemonCliPost).toHaveBeenCalledTimes(1)
    const [route, body] = daemonCliPost.mock.calls[0]
    expect(route).toBe('timer/create')
    expect(body).toMatchObject({
      projectId: 'proj-1',
      durationSeconds: 90,
      memo: 'worked on it',
    })
    expect(typeof (body as { id: string }).id).toBe('string')

    // Cross-window refresh fired exactly once for the mutation.
    expect(emitMock).toHaveBeenCalledWith('sync:timer-entries')

    // Timer reset after save.
    expect(useTimerStore.getState().status).toBe('idle')
  })

  it('saveEntry failure raises an error toast and does NOT emit sync', async () => {
    daemonCliPost.mockRejectedValueOnce(new Error('daemon down'))

    useTimerStore.setState({ startTime: 1_000_000, stoppedElapsed: 5_000 })
    await useTimerStore.getState().saveEntry()

    expect(addToast).toHaveBeenCalledWith('Failed to save timer entry', 'error')
    // No sync emit on a failed mutation.
    expect(emitMock).not.toHaveBeenCalledWith('sync:timer-entries')
    // Timer still resets (user shouldn't be stuck).
    expect(useTimerStore.getState().status).toBe('idle')
  })

  it('deleteEntry POSTs timer/delete, prunes locally, and emits sync', async () => {
    daemonCliPost.mockResolvedValueOnce({ success: true })
    const e: TimeEntry = {
      id: 'gone',
      projectId: null,
      startTime: 1,
      endTime: 2,
      durationSeconds: 1,
      memo: null,
      createdAt: 1,
    }
    useTimerStore.setState({ entries: [e] })

    await useTimerStore.getState().deleteEntry('gone')

    expect(daemonCliPost).toHaveBeenCalledWith('timer/delete', { id: 'gone' })
    expect(useTimerStore.getState().entries).toEqual([])
    expect(emitMock).toHaveBeenCalledWith('sync:timer-entries')
  })

  it('deleteEntry failure keeps the entry and does NOT emit sync', async () => {
    daemonCliPost.mockRejectedValueOnce(new Error('nope'))
    const e: TimeEntry = {
      id: 'keep',
      projectId: null,
      startTime: 1,
      endTime: 2,
      durationSeconds: 1,
      memo: null,
      createdAt: 1,
    }
    useTimerStore.setState({ entries: [e] })

    await useTimerStore.getState().deleteEntry('keep')

    expect(useTimerStore.getState().entries).toEqual([e])
    expect(emitMock).not.toHaveBeenCalledWith('sync:timer-entries')
  })

  it('exportEntries uses the RAW-TEXT GET (no JSON parse) for both formats', async () => {
    daemonCliGetText.mockResolvedValueOnce('id,duration\nx,1')
    const csv = await useTimerStore.getState().exportEntries('csv', 1, 2, 'proj-1')
    expect(daemonCliGetText).toHaveBeenCalledWith('timer/entries-export', {
      format: 'csv',
      start: 1,
      end: 2,
      project_id: 'proj-1',
    })
    expect(csv).toBe('id,duration\nx,1')

    // JSON format must come back as the verbatim STRING, not a parsed array,
    // so the caller can blob/download it unchanged.
    const jsonText = '[{"id":"x"}]'
    daemonCliGetText.mockResolvedValueOnce(jsonText)
    const json = await useTimerStore.getState().exportEntries('json')
    expect(json).toBe(jsonText)
    expect(typeof json).toBe('string')
    // The JSON-parsing GET helper must NOT be used for export.
    expect(daemonCliGet).not.toHaveBeenCalled()
  })

  it('exportEntries returns empty string on failure (no throw to caller)', async () => {
    daemonCliGetText.mockRejectedValueOnce(new Error('boom'))
    const out = await useTimerStore.getState().exportEntries('csv')
    expect(out).toBe('')
  })
})
