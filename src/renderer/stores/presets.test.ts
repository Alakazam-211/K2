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
const emitMock = vi.fn((..._args: unknown[]) => Promise.resolve())
vi.mock('@tauri-apps/api/event', () => ({
  emit: (...args: unknown[]) => emitMock(...args),
}))

// ── Mock ./tabs (registerPresetsStore import-time side effect) ───────────
vi.mock('./tabs', () => ({
  useTabsStore: { getState: () => ({}) },
  registerPresetsStore: vi.fn(),
}))

import {
  usePresetsStore,
  cloneDefaultInjectFlow,
  cloneDefaultInjectFlowForCommand,
  isDefaultInjectFlow,
  isDefaultInjectFlowForCommand,
  parseInjectFlowOrDefault,
  programIsGrok,
  readPresetInjectFlowJson,
  type AgentPreset,
} from './presets'

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

  it('updatePreset POSTs camelCase injectFlow when provided', async () => {
    daemonCliPost.mockResolvedValueOnce({})
    daemonCliGet.mockResolvedValueOnce([])
    const flow = '[{"key":"paste","waitMs":10},{"key":"return","waitMs":10}]'

    await usePresetsStore.getState().updatePreset({ id: 'p1', injectFlow: flow })

    expect(daemonCliPost).toHaveBeenCalledWith('presets/update', {
      id: 'p1',
      injectFlow: flow,
    })
  })
})

describe('inject flow helpers', () => {
  it('prefills D5 when the column is NULL and accepts both spellings', () => {
    const d5 = cloneDefaultInjectFlow()
    expect(parseInjectFlowOrDefault(null)).toEqual(d5)
    expect(isDefaultInjectFlow(parseInjectFlowOrDefault(undefined))).toBe(true)
    expect(readPresetInjectFlowJson({ inject_flow: null })).toBeNull()
    expect(
      readPresetInjectFlowJson({ injectFlow: '[{"key":"paste","waitMs":1},{"key":"return","waitMs":1}]' }),
    ).toBe('[{"key":"paste","waitMs":1},{"key":"return","waitMs":1}]')
    expect(
      readPresetInjectFlowJson({
        inject_flow: '[{"key":"esc","waitMs":0},{"key":"paste","waitMs":1}]',
      }),
    ).toBe('[{"key":"esc","waitMs":0},{"key":"paste","waitMs":1}]')
  })

  it('parses a stored grok experiment and falls back on garbage', () => {
    const grok =
      '[{"key":"esc","waitMs":0},{"key":"space","waitMs":50},{"key":"paste","waitMs":150},{"key":"return","waitMs":250},{"key":"return","waitMs":120}]'
    const parsed = parseInjectFlowOrDefault(grok)
    expect(parsed[0]).toEqual({ key: 'esc', waitMs: 0 })
    expect(parsed[1]).toEqual({ key: 'space', waitMs: 50 })
    expect(parsed[2]?.key).toBe('paste')
    expect(isDefaultInjectFlow(parsed)).toBe(false)
    expect(parseInjectFlowOrDefault('not-json')).toEqual(cloneDefaultInjectFlow())
  })

  it('uses paste plus one Return as the Grok default', () => {
    expect(programIsGrok('grok --always-approve')).toBe(true)
    expect(programIsGrok('/opt/homebrew/bin/grok')).toBe(true)
    expect(programIsGrok('claude --dangerously-skip-permissions')).toBe(false)
    const grokDefault = cloneDefaultInjectFlowForCommand('grok --always-approve')
    expect(grokDefault).toEqual([
      { key: 'paste', waitMs: 150 },
      { key: 'return', waitMs: 250 },
    ])
    expect(isDefaultInjectFlowForCommand(grokDefault, 'grok')).toBe(true)
    expect(isDefaultInjectFlow(grokDefault)).toBe(false)
    expect(parseInjectFlowOrDefault(null, 'grok --always-approve')).toEqual(grokDefault)
    expect(parseInjectFlowOrDefault(null, 'claude')).toEqual(cloneDefaultInjectFlow())
  })
})
