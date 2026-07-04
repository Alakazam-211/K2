// S7c — pin-size menu shared helper. Pins the pure halves both tab
// bars (PaneTabBar + workspace TabBar) rely on: item→sessionId
// resolution (the "offer the button at all" gate) and the ContextMenu
// item list (header, presets with ✓ on the active one, match, unpin
// only while pinned).

import { describe, it, expect } from 'vitest'
import {
  PIN_PRESETS,
  buildPinSizeMenuItems,
  resolvePinSessionId,
} from './pinSizeMenu'

describe('buildPinSizeMenuItems', () => {
  it('unpinned: disabled header, all presets unchecked, match, NO unpin', () => {
    const items = buildPinSizeMenuItems(null)

    expect(items[0]).toEqual({ id: 'pin-header', label: 'Pin size', enabled: false })

    const presets = items.filter((i) => i.id.startsWith('preset:'))
    expect(presets).toHaveLength(PIN_PRESETS.length)
    for (const p of presets) {
      expect(p.label).not.toContain('✓')
    }
    expect(presets.map((p) => p.id)).toEqual([
      'preset:80x24',
      'preset:100x30',
      'preset:120x36',
      'preset:160x48',
    ])

    expect(items.some((i) => i.id === 'match')).toBe(true)
    expect(items.some((i) => i.id === 'unpin')).toBe(false)
    expect(items.some((i) => i.type === 'separator')).toBe(false)
  })

  it('pinned to a preset: checkmark on exactly that preset', () => {
    const items = buildPinSizeMenuItems({ cols: 120, rows: 36 })

    const checked = items.filter((i) => i.label.includes('✓'))
    expect(checked).toHaveLength(1)
    expect(checked[0].id).toBe('preset:120x36')
    expect(checked[0].label).toContain('120×36')
  })

  it('pinned: separator + unpin appended after match', () => {
    const items = buildPinSizeMenuItems({ cols: 91, rows: 22 })

    // A non-preset pin ("match my window" sizes) checks nothing…
    expect(items.some((i) => i.label.includes('✓'))).toBe(false)
    // …but still offers Unpin behind a separator, in that order.
    const ids = items.map((i) => i.id)
    expect(ids.indexOf('pin-sep')).toBe(ids.indexOf('match') + 1)
    expect(ids.indexOf('unpin')).toBe(ids.indexOf('pin-sep') + 1)
    expect(items.find((i) => i.id === 'pin-sep')?.type).toBe('separator')
  })
})

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
