// @vitest-environment jsdom
// PageTabs crash hardening (0.46.7): the settings body comes from the
// daemon, any version. A body without `keybindings` (or with a non-object
// one) must leave the store holding `{}`, and every chord lookup must fall
// back to the default instead of throwing in a render.

import { describe, expect, it, vi } from 'vitest'

const io = vi.hoisted(() => ({ body: {} as Record<string, unknown> }))
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(async () => io.body),
  settingsUpdate: vi.fn(async () => io.body),
  settingsReset: vi.fn(async () => io.body),
}))

import { effectiveKeybinding, getDefaultKeybindings } from '@shared/hotkeys'
import { getEffectiveKeybinding, normalizeKeybindings, useSettingsStore } from './settings'

describe('keybindings from the daemon', () => {
  it('normalizeKeybindings keeps string combos only; anything else is {}', () => {
    expect(normalizeKeybindings(undefined)).toEqual({})
    expect(normalizeKeybindings(null)).toEqual({})
    expect(normalizeKeybindings('Meta+K')).toEqual({})
    expect(normalizeKeybindings(['Meta+K'])).toEqual({})
    expect(normalizeKeybindings({ openWiki: 'Meta+Shift+W', bad: 3, worse: null })).toEqual({ openWiki: 'Meta+Shift+W' })
  })

  it('effectiveKeybinding falls back to the default for a missing or malformed map', () => {
    const def = getDefaultKeybindings(true).openMobile
    expect(def).toBe('Meta+Shift+M')
    for (const bad of [undefined, null, [] as unknown, 'x' as unknown, { openMobile: 7 } as unknown]) {
      expect(effectiveKeybinding(bad as Record<string, string>, 'openMobile', true)).toBe(def)
      expect(getEffectiveKeybinding(bad as Record<string, string>, 'openMobile', true)).toBe(def)
    }
    expect(effectiveKeybinding({ openMobile: 'Meta+Alt+M' }, 'openMobile', true)).toBe('Meta+Alt+M')
    expect(effectiveKeybinding(undefined, 'no-such-id', true)).toBe('')
  })

  it('a settings body without keybindings leaves the store with {}', async () => {
    useSettingsStore.setState({ keybindings: { openMobile: 'Meta+Alt+M' } })
    io.body = { terminal: {}, projectSettings: {} }
    await useSettingsStore.getState().resetAllSettings()
    expect(useSettingsStore.getState().keybindings).toEqual({})
  })

  it('a non-object keybindings body is stored as {}', async () => {
    io.body = { terminal: {}, keybindings: 'oops' }
    await useSettingsStore.getState().resetAllSettings()
    expect(useSettingsStore.getState().keybindings).toEqual({})
  })
})
