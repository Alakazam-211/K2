// Pin-to-size shared helper tests (right-click + modal rework).
// Pins the pure halves both tab bars and PinDimensionsModal rely on:
// item→sessionId resolution (the "offer the menu entry at all" gate),
// the modal's preset rows, its populate/diverge form-state logic, the
// client-side bounds validation, and applyPinSize's POST payload
// shape + store mirror.

import { describe, it, expect, vi, beforeEach } from 'vitest'
import {
  PIN_BOUNDS,
  PIN_PRESETS,
  applyPinSize,
  buildPresetRows,
  pinFormFieldEdited,
  pinFormFromPin,
  pinFormPresetClicked,
  resolvePinSessionId,
  validatePinDims,
} from './pinSizeMenu'
import { usePinnedSizeStore } from '@/stores/pinned-size'
import { daemonCliPost } from '@/lib/daemon-cli'

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliPost: vi.fn(),
}))

const daemonCliPostMock = vi.mocked(daemonCliPost)

describe('resolvePinSessionId', () => {
  const sessions = {
    'term-1': 'sess-term-1',
    'term-2-shell': 'sess-term-2-shell',
    'agent-chat:proj-9': 'sess-chat-9',
  }
  const projects = [{ id: 'proj-9', path: '/w/nine' }]

  it('terminal item resolves by terminalId', () => {
    const id = resolvePinSessionId(
      { type: 'terminal', data: { terminalId: 'term-1', cwd: '/w' } },
      sessions,
      projects,
    )
    expect(id).toBe('sess-term-1')
  })

  it('terminal item falls back to the -shell (agent-exit) pane', () => {
    const id = resolvePinSessionId(
      { type: 'terminal', data: { terminalId: 'term-2', cwd: '/w' } },
      sessions,
      projects,
    )
    expect(id).toBe('sess-term-2-shell')
  })

  it('agent chat item reconstructs agent-chat:<projectId> via the projects map', () => {
    const id = resolvePinSessionId(
      { type: 'agent', data: { agentName: 'nine', projectPath: '/w/nine', section: 'chat' } },
      sessions,
      projects,
    )
    expect(id).toBe('sess-chat-9')
  })

  it('gates out inbox agents, unknown projects, file viewers and dead terminals', () => {
    expect(
      resolvePinSessionId(
        { type: 'agent', data: { agentName: 'nine', projectPath: '/w/nine', section: 'inbox' } },
        sessions,
        projects,
      ),
    ).toBeNull()
    expect(
      resolvePinSessionId(
        { type: 'agent', data: { agentName: 'x', projectPath: '/w/unknown', section: 'chat' } },
        sessions,
        projects,
      ),
    ).toBeNull()
    expect(
      resolvePinSessionId(
        { type: 'file-viewer', data: { filePath: '/w/a.md' } },
        sessions,
        projects,
      ),
    ).toBeNull()
    expect(
      resolvePinSessionId(
        { type: 'terminal', data: { terminalId: 'never-spawned', cwd: '/w' } },
        sessions,
        projects,
      ),
    ).toBeNull()
  })
})

describe('buildPresetRows', () => {
  it('without dims: exactly the fixed presets, in order, cols×rows labels, no match row', () => {
    const rows = buildPresetRows(null)
    expect(rows).toHaveLength(PIN_PRESETS.length)
    expect(rows.map((r) => r.id)).toEqual(
      PIN_PRESETS.map((p) => `preset:${p.cols}x${p.rows}`),
    )
    // First preset stays the classic 80×24 (80 cols × 24 rows) and the
    // label matches the cols-first notation the fields are ordered by.
    expect(rows[0]).toEqual({ id: 'preset:80x24', label: '80×24', cols: 80, rows: 24 })
    expect(rows.some((r) => r.id === 'match')).toBe(false)
  })

  it('with dims: appends "Match my window now" last, carrying the live numbers', () => {
    const rows = buildPresetRows({ cols: 143, rows: 37 })
    expect(rows).toHaveLength(PIN_PRESETS.length + 1)
    const match = rows[rows.length - 1]
    expect(match.id).toBe('match')
    expect(match.cols).toBe(143)
    expect(match.rows).toBe(37)
    expect(match.label).toContain('143×37')
    expect(match.label).toContain('Match my window now')
  })

  it('every fixed preset is within the daemon bounds', () => {
    for (const p of PIN_PRESETS) {
      expect(p.cols).toBeGreaterThanOrEqual(PIN_BOUNDS.minCols)
      expect(p.cols).toBeLessThanOrEqual(PIN_BOUNDS.maxCols)
      expect(p.rows).toBeGreaterThanOrEqual(PIN_BOUNDS.minRows)
      expect(p.rows).toBeLessThanOrEqual(PIN_BOUNDS.maxRows)
    }
  })
})

describe('pin form state (populate / diverge)', () => {
  it('starts empty when unpinned, prefilled from the current pin when re-pinning', () => {
    expect(pinFormFromPin(null)).toEqual({ cols: '', rows: '', selectedPresetId: null })
    expect(pinFormFromPin({ cols: 120, rows: 36 })).toEqual({
      cols: '120',
      rows: '36',
      selectedPresetId: null,
    })
  })

  it('preset click populates both fields and highlights the row', () => {
    const row = { id: 'preset:100x40', label: '100×40', cols: 100, rows: 40 }
    expect(pinFormPresetClicked(row)).toEqual({
      cols: '100',
      rows: '40',
      selectedPresetId: 'preset:100x40',
    })
  })

  it('editing a field after a preset click clears the highlight — values diverged', () => {
    const populated = pinFormPresetClicked({ id: 'match', label: 'Match my window now (143×37)', cols: 143, rows: 37 })
    const edited = pinFormFieldEdited(populated, 'cols', '150')
    expect(edited).toEqual({ cols: '150', rows: '37', selectedPresetId: null })

    const editedRows = pinFormFieldEdited(populated, 'rows', '40')
    expect(editedRows).toEqual({ cols: '143', rows: '40', selectedPresetId: null })
  })
})

describe('validatePinDims', () => {
  it('accepts whole numbers within bounds (with surrounding whitespace)', () => {
    expect(validatePinDims('80', '24')).toEqual({ ok: true, cols: 80, rows: 24 })
    expect(validatePinDims(' 200 ', ' 60 ')).toEqual({ ok: true, cols: 200, rows: 60 })
    // Bound edges are inclusive on both sides.
    expect(validatePinDims('20', '5')).toEqual({ ok: true, cols: 20, rows: 5 })
    expect(validatePinDims('500', '200')).toEqual({ ok: true, cols: 500, rows: 200 })
  })

  it('rejects non-numeric input, including empty, decimals and negatives', () => {
    for (const bad of ['', 'abc', '80.5', '-80', '8e1']) {
      const v = validatePinDims(bad, '24')
      expect(v.ok).toBe(false)
      if (!v.ok) expect(v.error).toContain('Columns')
    }
    for (const bad of ['', 'xyz', '24.0', '-24']) {
      const v = validatePinDims('80', bad)
      expect(v.ok).toBe(false)
      if (!v.ok) expect(v.error).toContain('Rows')
    }
  })

  it('rejects out-of-bounds values with the daemon limits in the message', () => {
    const colsLow = validatePinDims('19', '24')
    expect(colsLow).toEqual({ ok: false, error: 'Columns must be between 20 and 500' })
    const colsHigh = validatePinDims('501', '24')
    expect(colsHigh.ok).toBe(false)

    const rowsLow = validatePinDims('80', '4')
    expect(rowsLow).toEqual({ ok: false, error: 'Rows must be between 5 and 200' })
    const rowsHigh = validatePinDims('80', '201')
    expect(rowsHigh.ok).toBe(false)
  })

  it('reports the columns problem first when both fields are invalid (display order)', () => {
    const v = validatePinDims('1', '999')
    expect(v.ok).toBe(false)
    if (!v.ok) expect(v.error).toContain('Columns')
  })
})

describe('applyPinSize', () => {
  beforeEach(() => {
    daemonCliPostMock.mockReset()
    usePinnedSizeStore.setState({ pins: {}, sessions: {}, dims: {} })
  })

  it('pin: POSTs {session, cols, rows} and mirrors the authoritative answer into the store', async () => {
    daemonCliPostMock.mockResolvedValue({
      success: true,
      pinned: { cols: 120, rows: 36, setBy: 'owner' },
      persisted: true,
    })

    await applyPinSize('sess-1', { cols: 120, rows: 36 })

    expect(daemonCliPostMock).toHaveBeenCalledTimes(1)
    expect(daemonCliPostMock).toHaveBeenCalledWith('terminal/pin-size', {
      session: 'sess-1',
      cols: 120,
      rows: 36,
    })
    expect(usePinnedSizeStore.getState().pins['sess-1']).toEqual({
      cols: 120,
      rows: 36,
      setBy: 'owner',
    })
  })

  it('unpin (null): POSTs {session, clear: true} and drops the store entry', async () => {
    usePinnedSizeStore.getState().setPin('sess-1', { cols: 80, rows: 24, setBy: null })
    daemonCliPostMock.mockResolvedValue({ success: true, pinned: null, persisted: true })

    await applyPinSize('sess-1', null)

    expect(daemonCliPostMock).toHaveBeenCalledTimes(1)
    expect(daemonCliPostMock).toHaveBeenCalledWith('terminal/pin-size', {
      session: 'sess-1',
      clear: true,
    })
    expect(usePinnedSizeStore.getState().pins['sess-1']).toBeUndefined()
  })

  it('propagates daemon rejection and leaves the store untouched', async () => {
    usePinnedSizeStore.getState().setPin('sess-1', { cols: 80, rows: 24, setBy: null })
    daemonCliPostMock.mockRejectedValue(new Error('cols out of range'))

    await expect(applyPinSize('sess-1', { cols: 9999, rows: 24 })).rejects.toThrow(
      'cols out of range',
    )
    // The pre-existing pin is untouched — no optimistic write happened.
    expect(usePinnedSizeStore.getState().pins['sess-1']).toEqual({
      cols: 80,
      rows: 24,
      setBy: null,
    })
  })
})
