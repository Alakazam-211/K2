// Styles as Per-Client View State (prd-styles-per-client-v1).
//
// Locks the product rule: Style selection is personal thin-client view
// state — never daemon-canonical. Host switch / fetchSettings must not
// restyle from the daemon after the one-shot migration; updateStyleSettings
// must not POST style; multi-window sync is localStorage `storage` events.
//
// vitest env is node — in-memory localStorage + mocked daemon-settings so
// the settings store's import-time fetchSettings is controllable.

import { describe, it, expect, beforeEach, vi } from 'vitest'

// ── In-memory localStorage (style + settings modules read it) ───────────
const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => (mem.has(k) ? mem.get(k)! : null),
  setItem: (k: string, v: string) => void mem.set(k, v),
  removeItem: (k: string) => void mem.delete(k),
  clear: () => mem.clear(),
  key: (i: number) => Array.from(mem.keys())[i] ?? null,
  get length() {
    return mem.size
  },
})

// document is absent in node — stampStyleAttributes no-ops (early return).
// That is fine: we assert store state + localStorage, not DOM attributes.

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

const daemon = vi.hoisted(() => {
  const settingsGet = vi.fn(async (): Promise<Record<string, unknown>> => ({
    terminal: {},
    keybindings: {},
    projectSettings: {},
    editor: {},
  }))
  const settingsUpdate = vi.fn(
    async (updates: Record<string, unknown>): Promise<Record<string, unknown>> => ({
      terminal: {},
      keybindings: {},
      projectSettings: {},
      editor: {},
      ...updates,
    }),
  )
  const settingsReset = vi.fn(async (): Promise<Record<string, unknown>> => ({
    terminal: {},
    keybindings: {},
    projectSettings: {},
    editor: {},
    style: { id: 'square', palette: 'charcoal', scheme: 'dark', gaps: '' },
  }))
  return { settingsGet, settingsUpdate, settingsReset }
})

vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: (...args: unknown[]) => daemon.settingsGet(...(args as [])),
  settingsUpdate: (...args: unknown[]) =>
    daemon.settingsUpdate(...(args as [Record<string, unknown>])),
  settingsReset: (...args: unknown[]) => daemon.settingsReset(...(args as [])),
}))

vi.mock('@/stores/connect-host', () => ({
  onActiveHostChange: vi.fn(),
}))

import {
  LS_STYLE,
  LS_PALETTE,
  LS_SCHEME,
  LS_GAPS,
  LS_STYLE_MIGRATED,
  __setPreBootMirrorCompleteForTests,
  isStyleMigrated,
  markStyleMigrated,
  migrateStyleFromDaemon,
  readMirror,
  restampFromLocalMirror,
  useStyleStore,
} from './style'
import { useSettingsStore } from './settings'

const DAEMON_GLASS = {
  id: 'glass',
  palette: 'obsidian',
  scheme: 'dark',
  gaps: '',
}

const DAEMON_BEZEL = {
  id: 'bezel',
  palette: 'graphite',
  scheme: 'dark',
  gaps: 'spacious',
}

function clearStyleKeys(): void {
  mem.delete(LS_STYLE)
  mem.delete(LS_PALETTE)
  mem.delete(LS_SCHEME)
  mem.delete(LS_GAPS)
  mem.delete('k2.palette.dark')
  mem.delete('k2.palette.light')
  mem.delete(LS_STYLE_MIGRATED)
}

function seedLocal(style: string, palette: string, scheme: string, gaps = ''): void {
  mem.set(LS_STYLE, style)
  mem.set(LS_PALETTE, palette)
  mem.set(LS_SCHEME, scheme)
  mem.set(LS_GAPS, gaps)
}

const { settingsGet, settingsUpdate, settingsReset } = daemon

beforeEach(() => {
  mem.clear()
  settingsGet.mockReset()
  settingsUpdate.mockReset()
  settingsReset.mockReset()
  settingsGet.mockResolvedValue({
    terminal: {},
    keybindings: {},
    projectSettings: {},
    editor: {},
  })
  settingsUpdate.mockImplementation(async (updates: Record<string, unknown>) => ({
    terminal: {},
    keybindings: {},
    projectSettings: {},
    editor: {},
    ...updates,
  }))
  // Reset live style to defaults for isolation.
  useStyleStore.getState().applyStyle({
    styleId: 'square',
    paletteId: 'charcoal',
    schemeMode: 'dark',
    gapsPreset: '',
  })
  // Clear migration flag after applyStyle wrote mirror keys.
  mem.delete(LS_STYLE_MIGRATED)
  __setPreBootMirrorCompleteForTests(false)
  useSettingsStore.setState({
    style: { id: 'square', palette: 'charcoal', scheme: 'dark', gaps: '' },
    loaded: false,
  })
})

describe('migrateStyleFromDaemon (one-shot)', () => {
  it('empty pre-boot mirror + daemon style seeds local + sets flag', () => {
    clearStyleKeys()
    __setPreBootMirrorCompleteForTests(false)
    // Live store still at defaults from beforeEach; migration should seed glass.
    migrateStyleFromDaemon(DAEMON_GLASS)

    expect(isStyleMigrated()).toBe(true)
    expect(localStorage.getItem(LS_STYLE_MIGRATED)).toBe('1')
    expect(useStyleStore.getState().styleId).toBe('glass')
    expect(useStyleStore.getState().paletteId).toBe('obsidian')
    expect(readMirror().styleId).toBe('glass')
  })

  it('second different daemon style no-ops after migration', () => {
    clearStyleKeys()
    __setPreBootMirrorCompleteForTests(false)
    migrateStyleFromDaemon(DAEMON_GLASS)
    expect(useStyleStore.getState().styleId).toBe('glass')

    migrateStyleFromDaemon(DAEMON_BEZEL)
    expect(useStyleStore.getState().styleId).toBe('glass')
    expect(useStyleStore.getState().paletteId).toBe('obsidian')
    expect(isStyleMigrated()).toBe(true)
  })

  it('existing pre-boot mirror + different daemon keeps mirror and sets flag', () => {
    seedLocal('bezel', 'porcelain', 'light', '')
    useStyleStore.getState().applyStyle({
      styleId: 'bezel',
      paletteId: 'porcelain',
      schemeMode: 'light',
      gapsPreset: '',
    })
    mem.delete(LS_STYLE_MIGRATED)
    __setPreBootMirrorCompleteForTests(true)

    migrateStyleFromDaemon(DAEMON_GLASS)

    expect(isStyleMigrated()).toBe(true)
    expect(useStyleStore.getState().styleId).toBe('bezel')
    expect(useStyleStore.getState().paletteId).toBe('porcelain')
    expect(useStyleStore.getState().schemeMode).toBe('light')
  })

  it('already migrated never re-applies daemon style', () => {
    markStyleMigrated()
    useStyleStore.getState().applyStyle({
      styleId: 'square',
      paletteId: 'charcoal',
      schemeMode: 'dark',
      gapsPreset: '',
    })
    __setPreBootMirrorCompleteForTests(false)

    migrateStyleFromDaemon(DAEMON_GLASS)
    expect(useStyleStore.getState().styleId).toBe('square')
    expect(useStyleStore.getState().paletteId).toBe('charcoal')
  })

  it('empty mirror + no daemon style marks migrated with defaults', () => {
    clearStyleKeys()
    useStyleStore.getState().applyStyle({
      styleId: 'square',
      paletteId: 'charcoal',
      schemeMode: 'dark',
      gapsPreset: '',
    })
    mem.delete(LS_STYLE_MIGRATED)
    __setPreBootMirrorCompleteForTests(false)

    migrateStyleFromDaemon(undefined)
    expect(isStyleMigrated()).toBe(true)
    expect(useStyleStore.getState().styleId).toBe('square')
  })
})

describe('fetchSettings — local wins when migrated', () => {
  it('daemon style different from local does not restyle after migration', async () => {
    // User is on glass/obsidian locally and already migrated.
    useStyleStore.getState().applyStyle({
      styleId: 'glass',
      paletteId: 'obsidian',
      schemeMode: 'dark',
      gapsPreset: '',
    })
    markStyleMigrated()
    __setPreBootMirrorCompleteForTests(true)

    settingsGet.mockResolvedValue({
      terminal: { fontFamily: 'MesloLGM Nerd Font', fontSize: 13 },
      keybindings: {},
      projectSettings: {},
      editor: {},
      // Daemon still has the old shared charcoal — must not win.
      style: { id: 'square', palette: 'charcoal', scheme: 'dark', gaps: '' },
    })

    await useSettingsStore.getState().fetchSettings()

    expect(useStyleStore.getState().styleId).toBe('glass')
    expect(useStyleStore.getState().paletteId).toBe('obsidian')
    expect(useSettingsStore.getState().style).toEqual({
      id: 'glass',
      palette: 'obsidian',
      scheme: 'dark',
      gaps: '',
    })
    expect(useSettingsStore.getState().loaded).toBe(true)
  })

  it('first fetch with empty pre-boot mirror seeds once from daemon', async () => {
    clearStyleKeys()
    useStyleStore.getState().applyStyle({
      styleId: 'square',
      paletteId: 'charcoal',
      schemeMode: 'dark',
      gapsPreset: '',
    })
    mem.delete(LS_STYLE_MIGRATED)
    __setPreBootMirrorCompleteForTests(false)

    settingsGet.mockResolvedValue({
      terminal: {},
      keybindings: {},
      projectSettings: {},
      editor: {},
      style: DAEMON_GLASS,
    })

    await useSettingsStore.getState().fetchSettings()
    expect(useStyleStore.getState().styleId).toBe('glass')
    expect(isStyleMigrated()).toBe(true)

    // Second fetch with a different daemon style must not restyle.
    settingsGet.mockResolvedValue({
      terminal: {},
      keybindings: {},
      projectSettings: {},
      editor: {},
      style: DAEMON_BEZEL,
    })
    await useSettingsStore.getState().fetchSettings()
    expect(useStyleStore.getState().styleId).toBe('glass')
  })
})

describe('updateStyleSettings — no daemon POST', () => {
  it('does not call settingsUpdate with style', async () => {
    settingsUpdate.mockClear()

    await useSettingsStore.getState().updateStyleSettings({
      id: 'glass',
      palette: 'obsidian',
      scheme: 'dark',
      gaps: '',
    })

    expect(settingsUpdate).not.toHaveBeenCalled()
    // Live store + settings mirror updated locally.
    expect(useStyleStore.getState().styleId).toBe('glass')
    expect(useStyleStore.getState().paletteId).toBe('obsidian')
    expect(useSettingsStore.getState().style).toEqual({
      id: 'glass',
      palette: 'obsidian',
      scheme: 'dark',
      gaps: '',
    })
    expect(isStyleMigrated()).toBe(true)
    expect(localStorage.getItem(LS_STYLE)).toBe('glass')
  })

  it('unrelated persistAndApply still does not restyle from daemon echo', async () => {
    useStyleStore.getState().applyStyle({
      styleId: 'glass',
      paletteId: 'obsidian',
      schemeMode: 'dark',
      gapsPreset: '',
    })
    markStyleMigrated()
    useSettingsStore.setState({
      style: { id: 'glass', palette: 'obsidian', scheme: 'dark', gaps: '' },
    })

    // Daemon echo includes a different style (legacy field) — must ignore.
    settingsUpdate.mockResolvedValue({
      terminal: {
        fontFamily: 'MesloLGM Nerd Font',
        fontSize: 14,
        cursorStyle: 'bar',
        scrollback: 5000,
        naturalTextEditing: true,
      },
      keybindings: {},
      projectSettings: {},
      editor: {},
      style: DAEMON_BEZEL,
      defaultAgent: 'claude',
    })

    await useSettingsStore.getState().updateTerminalSettings({ fontSize: 14 })

    expect(useStyleStore.getState().styleId).toBe('glass')
    expect(useSettingsStore.getState().style.id).toBe('glass')
  })
})

describe('multi-window restampFromLocalMirror (storage path)', () => {
  it('re-reads mirror and applies after peer-style localStorage write', () => {
    useStyleStore.getState().applyStyle({
      styleId: 'square',
      paletteId: 'charcoal',
      schemeMode: 'dark',
      gapsPreset: '',
    })

    // Simulate another window writing the mirror (storage event payload).
    mem.set(LS_STYLE, 'glass')
    mem.set(LS_PALETTE, 'obsidian')
    mem.set(LS_SCHEME, 'dark')
    mem.set(LS_GAPS, '')
    mem.set('k2.palette.dark', 'obsidian')
    mem.set('k2.palette.light', 'veil')

    restampFromLocalMirror()

    expect(useStyleStore.getState().styleId).toBe('glass')
    expect(useStyleStore.getState().paletteId).toBe('obsidian')
  })
})
