// Plan B (Bulk-2) — vitest coverage for the presets store's daemon-data
// actions after migrating them OFF the Tauri `presets_*` invoke proxy ONTO
// the host-aware `daemonCli*` HTTP layer.
//
// What this asserts:
//   - fetchPresets         → GET  `presets/list`
//   - createPreset         → POST `presets/create`  + emits `sync:presets`
//   - updatePreset         → POST `presets/update`  (camelCase sortOrder)  + emit
//   - deletePreset         → POST `presets/delete`  + emit + local refetch
//   - reorderPresets       → POST `presets/reorder` + emit
//   - resetPresetsToBuiltIns → POST `presets/reset` + emit
//   - a failed mutation does NOT emit `sync:presets`
//
// The presets store has an import-time side effect (`registerPresetsStore`
// against `./tabs`), so `./tabs` is mocked via hoisted `vi.mock` BEFORE the
// store import (vitest hoists `vi.mock`).

import { describe, it, expect, beforeEach, vi } from 'vitest'

// ── Mock the host-aware daemon-cli layer (the thing we migrated TO) ──────
const daemonCliGet = vi.fn()
const daemonCliPost = vi.fn()
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (...args: unknown[]) => daemonCliGet(...args),
  daemonCliPost: (...args: unknown[]) => daemonCliPost(...args),
}))

// ── Mock the cross-window emit bus ───────────────────────────────────────
const emitMock = vi.fn(() => Promise.resolve())
vi.mock('@tauri-apps/api/event', () => ({
  emit: (...args: unknown[]) => emitMock(...args),
}))

// ── Mock ./tabs (registerPresetsStore import-time side effect) ───────────
vi.mock('./tabs', () => ({
  useTabsStore: { getState: () => ({}) },
  registerPresetsStore: vi.fn(),
}))

import { usePresetsStore, type AgentPreset } from './presets'

function resetStore(): void {
  usePresetsStore.setState({ presets: [], showPresetsBar: true })
}

const PRESET: AgentPreset = {
  id: 'p1',
  label: 'Claude',
  command: 'claude',
  icon: null,
  enabled: 1,
  sortOrder: 0,
  isBuiltIn: 1,
  createdAt: 1,
}

describe('presets store — Plan B host-aware migration', () => {
  beforeEach(() => {
    daemonCliGet.mockReset()
    daemonCliPost.mockReset()
    emitMock.mockClear()
    resetStore()
  })

  it('fetchPresets GETs presets/list and stores the result', async () => {
    daemonCliGet.mockResolvedValueOnce([PRESET])
    await usePresetsStore.getState().fetchPresets()
    expect(daemonCliGet).toHaveBeenCalledTimes(1)
    expect(daemonCliGet).toHaveBeenCalledWith('presets/list')
    expect(usePresetsStore.getState().presets).toEqual([PRESET])
  })

  it('createPreset POSTs presets/create, emits sync:presets, then refetches', async () => {
    daemonCliPost.mockResolvedValueOnce({}) // create
    daemonCliGet.mockResolvedValueOnce([PRESET]) // refetch

    await usePresetsStore
      .getState()
      .createPreset({ label: 'Claude', command: 'claude', icon: 'x' })

    expect(daemonCliPost).toHaveBeenCalledWith('presets/create', {
      label: 'Claude',
      command: 'claude',
      icon: 'x',
    })
    expect(emitMock).toHaveBeenCalledWith('sync:presets')
    // Refetch ran (GET fired after the mutation).
    expect(daemonCliGet).toHaveBeenCalledWith('presets/list')
    expect(usePresetsStore.getState().presets).toEqual([PRESET])
  })

  it('updatePreset POSTs presets/update with camelCase sortOrder + emits', async () => {
    daemonCliPost.mockResolvedValueOnce({})
    daemonCliGet.mockResolvedValueOnce([])

    await usePresetsStore
      .getState()
      .updatePreset({ id: 'p1', enabled: 0, sortOrder: 3 })

    expect(daemonCliPost).toHaveBeenCalledWith('presets/update', {
      id: 'p1',
      enabled: 0,
      sortOrder: 3,
    })
    expect(emitMock).toHaveBeenCalledWith('sync:presets')
  })

  it('deletePreset POSTs presets/delete, emits sync:presets, then refetches', async () => {
    daemonCliPost.mockResolvedValueOnce({})
    daemonCliGet.mockResolvedValueOnce([])

    await usePresetsStore.getState().deletePreset('p1')

    expect(daemonCliPost).toHaveBeenCalledWith('presets/delete', { id: 'p1' })
    expect(emitMock).toHaveBeenCalledWith('sync:presets')
    expect(daemonCliGet).toHaveBeenCalledWith('presets/list')
  })

  it('reorderPresets POSTs presets/reorder with the id list + emits', async () => {
    daemonCliPost.mockResolvedValueOnce({})
    daemonCliGet.mockResolvedValueOnce([])

    await usePresetsStore.getState().reorderPresets(['a', 'b', 'c'])

    expect(daemonCliPost).toHaveBeenCalledWith('presets/reorder', {
      ids: ['a', 'b', 'c'],
    })
    expect(emitMock).toHaveBeenCalledWith('sync:presets')
  })

  it('resetPresetsToBuiltIns POSTs presets/reset (empty body) + emits', async () => {
    daemonCliPost.mockResolvedValueOnce({})
    daemonCliGet.mockResolvedValueOnce([])

    await usePresetsStore.getState().resetPresetsToBuiltIns()

    expect(daemonCliPost).toHaveBeenCalledWith('presets/reset', {})
    expect(emitMock).toHaveBeenCalledWith('sync:presets')
  })

  it('a failed mutation rejects and does NOT emit sync:presets', async () => {
    daemonCliPost.mockRejectedValueOnce(new Error('daemon down'))

    await expect(
      usePresetsStore.getState().deletePreset('p1'),
    ).rejects.toThrow('daemon down')

    expect(emitMock).not.toHaveBeenCalledWith('sync:presets')
    // No refetch fired after the failed mutation.
    expect(daemonCliGet).not.toHaveBeenCalled()
  })
})
