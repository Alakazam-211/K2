// The Board's Beta core (Rosson 2026-11-01): which built-in needs a core,
// and the placeholder's words.
import { describe, expect, it } from 'vitest'
import { zenCoreOffText, zenWidgetCore } from './zen-widget-core'

describe('zenWidgetCore', () => {
  it('names the Board core for k2:board at any version, and nothing else', () => {
    expect(zenWidgetCore('k2:board@1')).toBe('board')
    expect(zenWidgetCore('k2:board@12')).toBe('board')
    expect(zenWidgetCore('k2:board')).toBe('board')
    for (const other of ['k2:kanban@5', 'board', 'k2:boards@1', 'k2:board@0', 'my-board', '', null, undefined]) {
      expect(zenWidgetCore(other), String(other)).toBeNull()
    }
  })

  it('says where to turn it on and that the boards are kept', () => {
    expect(zenCoreOffText('board')).toBe(
      "Board is a Beta that's off on this computer. Turn it on here or in Settings → Cores → Multiplayer; your boards are kept while it's off.",
    )
  })
})
