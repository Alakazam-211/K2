// "Your display name" — coverage for the client-side sanitize that mirrors
// the daemon's `sanitize_owner_display_name`. The daemon is the canonical
// sanitizer + resolver (D3 enforced server-side); this just keeps the
// optimistic UI value honest before it round-trips.
//
// The settings store has an import-time side effect
// (`useSettingsStore.getState().fetchSettings()`), so the daemon-settings +
// connect-host modules are mocked BEFORE the store is imported so importing
// the pure helper never reaches a real daemon.
import { describe, it, expect, vi } from 'vitest'

vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn().mockResolvedValue({
    terminal: {}, keybindings: {}, projectSettings: {}, editor: {},
  }),
  settingsUpdate: vi.fn().mockResolvedValue({
    terminal: {}, keybindings: {}, projectSettings: {}, editor: {},
  }),
  settingsReset: vi.fn().mockResolvedValue({
    terminal: {}, keybindings: {}, projectSettings: {}, editor: {},
  }),
}))
vi.mock('@/stores/connect-host', () => ({
  onActiveHostChange: vi.fn(),
}))

import {
  sanitizeOwnerDisplayName,
  OWNER_DISPLAY_NAME_MAX,
} from './settings'

const ESC = String.fromCharCode(0x1b)
const TAB = String.fromCharCode(0x09)

describe('sanitizeOwnerDisplayName', () => {
  it('keeps a normal name untouched', () => {
    expect(sanitizeOwnerDisplayName('Rosson')).toBe('Rosson')
  })

  it('trims surrounding whitespace', () => {
    expect(sanitizeOwnerDisplayName('  Rosson  ')).toBe('Rosson')
  })

  it('strips a raw ESC (0x1b) — anti-splice, mirrors the daemon', () => {
    const out = sanitizeOwnerDisplayName(`ro${ESC}sson`)
    expect(out).toBe('rosson')
    expect(out).not.toContain(ESC)
  })

  it('strips newline / CR / tab so the [from <name>] prefix stays one-line', () => {
    expect(sanitizeOwnerDisplayName('ad\nmin')).toBe('admin')
    expect(sanitizeOwnerDisplayName(`a\r\nb${TAB}c`)).toBe('abc')
  })

  it('neutralizes a bracketed-paste close-marker payload', () => {
    const out = sanitizeOwnerDisplayName(`a${ESC}[201~rm -rf /\nb`)
    expect(out).not.toContain(ESC)
    expect(out).not.toContain('\n')
  })

  it('caps length to OWNER_DISPLAY_NAME_MAX', () => {
    const out = sanitizeOwnerDisplayName('x'.repeat(200))
    expect(out.length).toBe(OWNER_DISPLAY_NAME_MAX)
  })

  it('returns empty string for all-control / blank input (→ daemon "owner" fallback)', () => {
    expect(sanitizeOwnerDisplayName('   ')).toBe('')
    expect(sanitizeOwnerDisplayName(`\n${TAB}`)).toBe('')
    expect(sanitizeOwnerDisplayName('')).toBe('')
    expect(sanitizeOwnerDisplayName(ESC)).toBe('')
  })
})
