// @vitest-environment jsdom
//
// The accent override and Auto picks, against the real store, the real <html>
// and index.html's real pre-paint bootstrap.

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))

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

let osLight = false
Object.defineProperty(window, 'matchMedia', {
  configurable: true,
  value: (q: string) => ({
    matches: q.includes('light') ? osLight : !osLight,
    addEventListener: () => {},
  }),
})

const html = document.documentElement
const accentVar = (): string => html.style.getPropertyValue('--color-accent')

const page = readFileSync(resolve(__dirname, '../index.html'), 'utf8')
const script = page.match(/<script>(\s*\/\/ Stamp Style attributes[\s\S]*?)<\/script>/)
if (!script) throw new Error('the Style bootstrap is not in index.html')
const runBootstrap = new Function(script[1])

function clearHtml(): void {
  for (const a of ['data-style', 'data-palette', 'data-scheme', 'data-gaps', 'style']) html.removeAttribute(a)
}

async function freshStore() {
  vi.resetModules()
  return (await import('./style')).useStyleStore
}

const SWITCHED = 'k2.style.migrated-0-46-11-glass'

/** A client that already had 0.46.11's one-time switch, then saved Terminal /
 *  Charcoal / Dark. */
function seedTerminal(): void {
  mem.set(SWITCHED, '1')
  mem.set('k2.style', 'terminal')
  mem.set('k2.palette', 'charcoal')
  mem.set('k2.scheme', 'dark')
}

beforeEach(() => {
  mem.clear()
  seedTerminal()
  clearHtml()
  osLight = false
})

describe('accent override', () => {
  it('applies live and is saved', async () => {
    const store = await freshStore()
    store.getState().setAccent('#FF00AA')
    expect(accentVar()).toBe('#ff00aa')
    expect(store.getState().accent).toBe('#ff00aa')
    expect(mem.get('k2.accent.terminal.charcoal')).toBe('#ff00aa')
  })

  it('still applies live when storage refuses to save it', async () => {
    const store = await freshStore()
    const setItem = vi.spyOn(localStorage, 'setItem').mockImplementation(() => {
      throw new Error('QuotaExceededError')
    })
    try {
      store.getState().setAccent('#ff00aa')
    } finally {
      setItem.mockRestore()
    }
    expect(accentVar()).toBe('#ff00aa')
    expect(store.getState().accent).toBe('#ff00aa')
    expect(mem.has('k2.accent.terminal.charcoal')).toBe(false)
  })

  it('ignores an invalid colour', async () => {
    const store = await freshStore()
    store.getState().setAccent('not-a-colour')
    expect(accentVar()).toBe('')
    expect(store.getState().accent).toBeNull()
    expect(mem.has('k2.accent.terminal.charcoal')).toBe(false)
  })

  it('an invalid stored value falls back to the default at boot', async () => {
    mem.set('k2.accent.terminal.charcoal', 'url(evil)')
    const store = await freshStore()
    expect(store.getState().accent).toBeNull()
    expect(accentVar()).toBe('')
  })

  it('Reset removes it at once, leaving the palette in charge', async () => {
    const store = await freshStore()
    store.getState().setAccent('#ff00aa')
    store.getState().resetAccent()
    expect(accentVar()).toBe('')
    expect(html.style.getPropertyValue('--color-on-accent')).toBe('')
    expect(store.getState().accent).toBeNull()
    expect(mem.has('k2.accent.terminal.charcoal')).toBe(false)
    expect(mem.has('k2.accent.stamp.dark')).toBe(false)
  })

  it('is remembered per style', async () => {
    const store = await freshStore()
    store.getState().setAccent('#ff00aa')
    store.getState().applyStyle({ styleId: 'glass', paletteId: 'obsidian' })
    expect(accentVar()).toBe('')
    expect(store.getState().accent).toBeNull()
    store.getState().setAccent('#00ff88')
    store.getState().applyStyle({ styleId: 'terminal', paletteId: 'charcoal' })
    expect(accentVar()).toBe('#ff00aa')
    expect(store.getState().accent).toBe('#ff00aa')
  })

  it('a hover preview of another style shows that style’s accent, and leaving restores', async () => {
    const store = await freshStore()
    const { stampStyleAttributes } = await import('./style')
    store.getState().setAccent('#ff00aa')
    mem.set('k2.accent.glass.obsidian', '#00ff88')
    const { styleId, paletteId, schemeMode, gapsPreset } = store.getState()
    stampStyleAttributes({ styleId: 'glass', paletteId: 'obsidian', schemeMode, gapsPreset })
    expect(accentVar()).toBe('#00ff88')
    stampStyleAttributes({ styleId, paletteId, schemeMode, gapsPreset })
    expect(accentVar()).toBe('#ff00aa')
    expect(store.getState().accent).toBe('#ff00aa')
  })

  it('picks readable ink for a bright colour', async () => {
    const store = await freshStore()
    store.getState().setAccent('#ffee00')
    expect(html.style.getPropertyValue('--color-on-accent')).toBe('#141414')
  })

  it('a peer window’s change arrives through the stamp key', async () => {
    const { STYLE_MIRROR_STORAGE_KEYS } = await import('./style')
    expect(STYLE_MIRROR_STORAGE_KEYS).toEqual(
      expect.arrayContaining(['k2.accent.stamp.dark', 'k2.accent.stamp.light']),
    )
  })
})

describe('an accent per palette', () => {
  const glassKeys = (): Record<string, string> =>
    Object.fromEntries([...mem].filter(([k]) => /^k2\.accent\.glass\./.test(k)))

  /** Glass on a fixed dark mode, Obsidian, past every migration. */
  const seedGlass = (): void => {
    mem.set('k2.style', 'glass')
    mem.set('k2.palette', 'obsidian')
    mem.set('k2.scheme', 'dark')
  }

  it('changing Obsidian’s accent leaves Veil’s and Yellow’s alone', async () => {
    seedGlass()
    mem.set('k2.accent.glass.veil', '#3366cc')
    const store = await freshStore()
    store.getState().setAccent('#ff00aa')
    expect(glassKeys()).toEqual({ 'k2.accent.glass.obsidian': '#ff00aa', 'k2.accent.glass.veil': '#3366cc' })
    store.getState().applyStyle({ paletteId: 'yellow' })
    expect([store.getState().accent, accentVar()]).toEqual([null, ''])
    store.getState().applyStyle({ paletteId: 'veil', schemeMode: 'light' })
    expect([store.getState().accent, accentVar()]).toEqual(['#3366cc', '#3366cc'])
    store.getState().applyStyle({ paletteId: 'obsidian', schemeMode: 'dark' })
    expect([store.getState().accent, accentVar()]).toEqual(['#ff00aa', '#ff00aa'])
  })

  it('Reset returns only that palette to its own accent', async () => {
    seedGlass()
    mem.set('k2.accent.glass.obsidian', '#ff00aa')
    mem.set('k2.accent.glass.yellow', '#00ff88')
    const store = await freshStore()
    store.getState().resetAccent()
    expect(glassKeys()).toEqual({ 'k2.accent.glass.yellow': '#00ff88' })
    expect([store.getState().accent, accentVar()]).toEqual([null, ''])
    store.getState().applyStyle({ paletteId: 'yellow' })
    expect(accentVar()).toBe('#00ff88')
  })

  it('under Auto, each OS appearance shows its own palette’s accent, live and at first paint', async () => {
    mem.set('k2.style', 'glass')
    mem.set('k2.scheme', 'auto')
    mem.set('k2.palette', 'obsidian')
    mem.set('k2.palette.dark', 'obsidian')
    mem.set('k2.palette.light', 'veil')
    mem.set('k2.accent.glass.obsidian', '#ff00aa')
    mem.set('k2.accent.glass.veil', '#3366cc')
    const store = await freshStore()
    expect([store.getState().resolvedPaletteId, store.getState().accent, accentVar()]).toEqual([
      'obsidian',
      '#ff00aa',
      '#ff00aa',
    ])
    const dark = JSON.parse(mem.get('k2.accent.stamp.dark')!)
    const light = JSON.parse(mem.get('k2.accent.stamp.light')!)
    expect([dark.palette, dark.vars['--color-accent'], light.palette, light.vars['--color-accent']]).toEqual([
      'obsidian',
      '#ff00aa',
      'veil',
      '#3366cc',
    ])
    clearHtml()
    osLight = true
    runBootstrap()
    expect([html.getAttribute('data-palette'), accentVar()]).toEqual(['veil', '#3366cc'])
    clearHtml()
    osLight = false
    runBootstrap()
    expect([html.getAttribute('data-palette'), accentVar()]).toEqual(['obsidian', '#ff00aa'])
  })

  it('under Auto, the row edits the palette on screen: a light system’s change is Veil’s', async () => {
    mem.set('k2.style', 'glass')
    mem.set('k2.scheme', 'auto')
    mem.set('k2.palette.dark', 'obsidian')
    mem.set('k2.palette.light', 'veil')
    osLight = true
    const store = await freshStore()
    store.getState().setAccent('#3366cc')
    expect(glassKeys()).toEqual({ 'k2.accent.glass.veil': '#3366cc' })
    expect(mem.has('k2.accent.stamp.dark')).toBe(false)
  })

  it('a hover preview of another palette shows that palette’s accent', async () => {
    seedGlass()
    mem.set('k2.accent.glass.veil', '#3366cc')
    const store = await freshStore()
    const { stampStyleAttributes } = await import('./style')
    const { styleId, gapsPreset } = store.getState()
    stampStyleAttributes({ styleId, paletteId: 'veil', schemeMode: 'light', gapsPreset })
    expect(accentVar()).toBe('#3366cc')
    stampStyleAttributes({ styleId, paletteId: 'obsidian', schemeMode: 'dark', gapsPreset })
    expect(accentVar()).toBe('')
  })
})

describe('accent pre-paint (index.html)', () => {
  it('paints the saved accent before any module runs', async () => {
    const store = await freshStore()
    store.getState().setAccent('#ff00aa')
    clearHtml()
    runBootstrap()
    expect(accentVar()).toBe('#ff00aa')
  })

  it('paints nothing without a saved accent', async () => {
    await freshStore()
    clearHtml()
    runBootstrap()
    expect(accentVar()).toBe('')
  })

  const stamp = (over: Record<string, unknown>): void => {
    mem.set('k2.style', 'terminal')
    mem.set('k2.palette', 'charcoal')
    mem.set('k2.scheme', 'dark')
    mem.set(
      'k2.accent.stamp.dark',
      JSON.stringify({ style: 'terminal', palette: 'charcoal', vars: { '--color-accent': '#ff00aa' }, ...over }),
    )
  }

  it('skips a stamp for another style or palette', () => {
    stamp({ style: 'glass' })
    runBootstrap()
    expect(accentVar()).toBe('')
    clearHtml()
    stamp({ palette: 'paper' })
    runBootstrap()
    expect(accentVar()).toBe('')
  })

  it('skips any value that is not a plain colour', () => {
    stamp({ vars: { '--color-accent': 'url(evil)', '--color-accent-hover': '#123456', 'color': '#123456' } })
    runBootstrap()
    expect(accentVar()).toBe('')
    expect(html.style.getPropertyValue('--color-accent-hover')).toBe('#123456')
    expect(html.style.getPropertyValue('color')).toBe('')
  })

  it('survives a damaged stamp', () => {
    mem.set('k2.scheme', 'dark')
    mem.set('k2.accent.stamp.dark', '{not json')
    expect(() => runBootstrap()).not.toThrow()
    expect(html.getAttribute('data-palette')).toBe('charcoal')
  })
})

describe('Auto picks at boot', () => {
  const seedAuto = (dark: string, light: string): void => {
    mem.set('k2.style', 'terminal')
    mem.set('k2.scheme', 'auto')
    mem.set('k2.palette', 'charcoal')
    mem.set('k2.palette.dark', dark)
    mem.set('k2.palette.light', light)
    mem.set('k2.style.migrated', '1')
  }

  it('paints the pick for this appearance, with the palette’s own polarity', () => {
    seedAuto('charcoal', 'yellow')
    osLight = true
    runBootstrap()
    expect([html.getAttribute('data-palette'), html.getAttribute('data-scheme')]).toEqual(['yellow', 'dark'])
  })

  it('the store agrees with the bootstrap, and keeps both picks', async () => {
    seedAuto('charcoal', 'yellow')
    osLight = true
    const store = await freshStore()
    expect([html.getAttribute('data-palette'), html.getAttribute('data-scheme')]).toEqual(['yellow', 'dark'])
    expect(store.getState().autoPalettes).toEqual({ dark: 'charcoal', light: 'yellow' })
    expect(mem.get('k2.palette.light')).toBe('yellow')
  })

  it('a fixed mode leaves the picks for when Auto returns', async () => {
    seedAuto('yellow', 'paper')
    const store = await freshStore()
    store.getState().applyStyle({ schemeMode: 'light', paletteId: 'paper' })
    store.getState().applyStyle({ schemeMode: 'auto' })
    expect(store.getState().autoPalettes).toEqual({ dark: 'yellow', light: 'paper' })
    expect(html.getAttribute('data-palette')).toBe('yellow')
  })

  it('a fixed mode paints its own palette, not an Auto pick left in that side’s key', () => {
    mem.set('k2.style', 'terminal')
    mem.set('k2.scheme', 'light')
    mem.set('k2.palette', 'paper')
    mem.set('k2.palette.light', 'yellow')
    runBootstrap()
    expect([html.getAttribute('data-palette'), html.getAttribute('data-scheme')]).toEqual(['paper', 'light'])
  })

  it('the accent stamp follows the pick for this appearance', async () => {
    seedAuto('charcoal', 'paper')
    const store = await freshStore()
    store.getState().setAccent('#ff00aa')
    mem.set('k2.accent.terminal.paper', '#3366cc')
    store.getState().applyStyle({})
    const light = JSON.parse(mem.get('k2.accent.stamp.light')!)
    const dark = JSON.parse(mem.get('k2.accent.stamp.dark')!)
    expect([light.palette, dark.palette]).toEqual(['paper', 'charcoal'])
    clearHtml()
    osLight = true
    runBootstrap()
    expect(accentVar()).toBe('#3366cc')
    expect(html.getAttribute('data-palette')).toBe('paper')
  })
})

describe('the default look on a client with nothing saved', () => {
  beforeEach(() => {
    mem.clear()
  })

  /** Every saved accent choice (not the pre-paint stamps or flags). */
  const accentChoiceKeys = (): string[] =>
    [...mem.keys()].filter((k) => k.startsWith('k2.accent.') && !k.startsWith('k2.accent.stamp.'))

  it('boots to Glass under Auto, Obsidian and Veil, with no accent written', async () => {
    const store = await freshStore()
    const s = store.getState()
    expect([s.styleId, s.paletteId, s.schemeMode, s.autoPalettes]).toEqual([
      'glass',
      'obsidian',
      'auto',
      { dark: 'obsidian', light: 'veil' },
    ])
    expect(s.accent).toBeNull()
    expect(accentChoiceKeys()).toEqual([])
    expect([html.getAttribute('data-style'), html.getAttribute('data-palette')]).toEqual(['glass', 'obsidian'])
    expect(accentVar()).toBe('')
    expect([mem.get('k2.style'), mem.get('k2.scheme'), mem.get('k2.palette.light')]).toEqual(['glass', 'auto', 'veil'])
  })

  it('on a light system it is Veil with its own accent', async () => {
    osLight = true
    const store = await freshStore()
    expect(store.getState().resolvedPaletteId).toBe('veil')
    expect(accentVar()).toBe('')
  })

  it('the first paint is already the default, with each palette’s own accent', () => {
    runBootstrap()
    expect([html.getAttribute('data-style'), html.getAttribute('data-palette'), html.getAttribute('data-scheme')]).toEqual([
      'glass',
      'obsidian',
      'dark',
    ])
    expect(accentVar()).toBe('')
    clearHtml()
    osLight = true
    runBootstrap()
    expect([html.getAttribute('data-palette'), html.getAttribute('data-scheme')]).toEqual(['veil', 'light'])
    expect(accentVar()).toBe('')
  })

  it('the bootstrap’s default accents are exactly what DEFAULT_PALETTE_ACCENTS derives, on both sides', async () => {
    const { accentVarsFor } = await import('@/lib/style-accent')
    const { DEFAULT_PALETTE_ACCENTS } = await import('@/lib/style-resolve')
    const { getStyle } = await import('@/styles.generated')
    const glass = getStyle('glass')!
    for (const [palette, light] of [['obsidian', false], ['veil', true]] as const) {
      clearHtml()
      osLight = light
      runBootstrap()
      const choice = DEFAULT_PALETTE_ACCENTS[palette] ?? null
      const want = accentVarsFor(choice, glass.palettes.find((p) => p.id === palette)!, light ? 'light' : 'dark')
      const names = ['--color-accent', ...Object.keys(want)]
      const got = Object.fromEntries(names.map((k) => [k, html.style.getPropertyValue(k)]).filter(([, v]) => v !== ''))
      expect([palette, got]).toEqual([palette, want])
    }
  })

  it('the bootstrap and the store agree after the first launch', async () => {
    await freshStore()
    clearHtml()
    runBootstrap()
    expect(accentVar()).toBe('')
    expect(html.getAttribute('data-style')).toBe('glass')
  })

  it('an existing Terminal client keeps it, and no accent is written', async () => {
    seedTerminal()
    const store = await freshStore()
    expect([store.getState().styleId, store.getState().resolvedPaletteId, store.getState().schemeMode]).toEqual([
      'terminal',
      'charcoal',
      'dark',
    ])
    expect(store.getState().accent).toBeNull()
    expect(accentChoiceKeys()).toEqual([])
    expect(accentVar()).toBe('')
    clearHtml()
    runBootstrap()
    expect([html.getAttribute('data-style'), accentVar()]).toEqual(['terminal', ''])
  })

  it('past the one-time switch, a deliberate Glass accent from before moves to every Glass palette', async () => {
    mem.set(SWITCHED, '1')
    mem.set('k2.style', 'glass')
    mem.set('k2.palette', 'veil')
    mem.set('k2.scheme', 'light')
    mem.set('k2.accent.glass', '#00ff88')
    const store = await freshStore()
    expect(store.getState().accent).toBe('#00ff88')
    expect(mem.has('k2.accent.glass')).toBe(false)
    expect(['obsidian', 'veil', 'yellow'].map((p) => mem.get(`k2.accent.glass.${p}`))).toEqual([
      '#00ff88',
      '#00ff88',
      '#00ff88',
    ])
  })
})

describe('the daemon’s style echo on a new client', () => {
  // A client past the one-time switch whose saved style is gone: the path
  // where the older daemon migration can still run.
  beforeEach(() => {
    mem.clear()
    mem.set(SWITCHED, '1')
  })

  it('the daemon’s never-set default (square / charcoal / dark) does not replace the default look', async () => {
    const store = await freshStore()
    const { migrateStyleFromDaemon, __setPreBootMirrorCompleteForTests } = await import('./style')
    __setPreBootMirrorCompleteForTests(false)
    migrateStyleFromDaemon({ id: 'square', palette: 'charcoal', scheme: 'dark', gaps: '' })
    expect([store.getState().styleId, store.getState().accent]).toEqual(['glass', null])
    expect(mem.get('k2.style.migrated')).toBe('1')
  })

  it('a style someone chose on the daemon still seeds such a client, with its own accent', async () => {
    const store = await freshStore()
    const { migrateStyleFromDaemon, __setPreBootMirrorCompleteForTests } = await import('./style')
    __setPreBootMirrorCompleteForTests(false)
    migrateStyleFromDaemon({ id: 'glass', palette: 'veil', scheme: 'light', gaps: '' })
    expect([store.getState().styleId, store.getState().resolvedPaletteId]).toEqual(['glass', 'veil'])
    expect(store.getState().accent).toBeNull()
    expect(mem.has('k2.accent.glass.veil')).toBe(false)
  })
})

describe('the one-time switch to the default look (first launch of 0.46.11)', () => {
  /** A client from before 0.46.11: Terminal / Charcoal / Dark, a pink accent
   *  under the old shared key, the older daemon migration done, and no
   *  switch flag. */
  const seedOldTerminal = (): void => {
    mem.clear()
    mem.set('k2.style', 'terminal')
    mem.set('k2.palette', 'charcoal')
    mem.set('k2.palette.dark', 'charcoal')
    mem.set('k2.scheme', 'dark')
    mem.set('k2.accent.terminal', '#ff00aa')
    mem.set(
      'k2.accent.stamp.dark',
      JSON.stringify({ style: 'terminal', palette: 'charcoal', vars: { '--color-accent': '#ff00aa' } }),
    )
    mem.set('k2.style.migrated', '1')
  }

  it('moves an existing Terminal client to Glass / Auto / Obsidian + Veil, writes no Glass accent, and sets the flag', async () => {
    seedOldTerminal()
    const store = await freshStore()
    const s = store.getState()
    expect([s.styleId, s.schemeMode, s.autoPalettes, s.resolvedPaletteId, s.accent]).toEqual([
      'glass',
      'auto',
      { dark: 'obsidian', light: 'veil' },
      'obsidian',
      null,
    ])
    expect(mem.get(SWITCHED)).toBe('1')
    expect([mem.get('k2.style'), mem.get('k2.scheme'), mem.get('k2.palette.dark'), mem.get('k2.palette.light')]).toEqual([
      'glass',
      'auto',
      'obsidian',
      'veil',
    ])
    expect([...mem.keys()].filter((k) => k.startsWith('k2.accent.glass'))).toEqual([])
    expect(html.getAttribute('data-style')).toBe('glass')
    expect(accentVar()).toBe('')
    // Terminal's own accent is kept, per palette, for if they go back.
    expect(mem.get('k2.accent.terminal.charcoal')).toBe('#ff00aa')
    expect(mem.has('k2.accent.terminal')).toBe(false)
  })

  it('writes accents only from DEFAULT_PALETTE_ACCENTS, each on its own palette', async () => {
    seedOldTerminal()
    const setItem = vi.spyOn(localStorage, 'setItem')
    try {
      await freshStore()
      const { DEFAULT_PALETTE_ACCENTS } = await import('@/lib/style-resolve')
      const written = setItem.mock.calls
        .map(([k]) => k)
        .filter((k) => k.startsWith('k2.accent.glass'))
      expect(written.sort()).toEqual(Object.keys(DEFAULT_PALETTE_ACCENTS).map((p) => `k2.accent.glass.${p}`).sort())
    } finally {
      setItem.mockRestore()
    }
  })

  it('the first frame of 0.46.11 is already Glass, with each palette’s own accent, before any module runs', () => {
    seedOldTerminal()
    runBootstrap()
    expect([html.getAttribute('data-style'), html.getAttribute('data-palette'), html.getAttribute('data-scheme')]).toEqual([
      'glass',
      'obsidian',
      'dark',
    ])
    expect(accentVar()).toBe('')
    clearHtml()
    osLight = true
    runBootstrap()
    expect([html.getAttribute('data-style'), html.getAttribute('data-palette'), accentVar()]).toEqual([
      'glass',
      'veil',
      '',
    ])
  })

  it('happens once: Terminal chosen afterwards stays Terminal, on every later launch and first paint', async () => {
    seedOldTerminal()
    const store = await freshStore()
    store.getState().applyStyle({ styleId: 'terminal', paletteId: 'charcoal', schemeMode: 'dark' })
    const again = await freshStore()
    expect([again.getState().styleId, again.getState().resolvedPaletteId, again.getState().schemeMode]).toEqual([
      'terminal',
      'charcoal',
      'dark',
    ])
    expect(again.getState().accent).toBe('#ff00aa')
    clearHtml()
    runBootstrap()
    expect([html.getAttribute('data-style'), accentVar()]).toEqual(['terminal', '#ff00aa'])
  })

  it('a client that went through the old switch (Yellow on every Glass palette) ends with no accent keys', async () => {
    mem.clear()
    mem.set(SWITCHED, '1')
    mem.set('k2.style.migrated', '1')
    mem.set('k2.style', 'glass')
    mem.set('k2.scheme', 'auto')
    mem.set('k2.palette', 'obsidian')
    mem.set('k2.palette.dark', 'obsidian')
    mem.set('k2.palette.light', 'veil')
    mem.set('k2.accent.glass', 'yellow')
    const yellowStamp = (palette: string, hex: string): string =>
      JSON.stringify({ style: 'glass', palette, vars: { '--color-accent': hex } })
    mem.set('k2.accent.stamp.dark', yellowStamp('obsidian', '#ffc700'))
    mem.set('k2.accent.stamp.light', yellowStamp('veil', '#e6b400'))
    // The first frame of this build already leaves Yellow off.
    runBootstrap()
    expect(accentVar()).toBe('')
    const store = await freshStore()
    expect([...mem.keys()].filter((k) => k.startsWith('k2.accent.'))).toEqual([])
    expect([store.getState().accent, accentVar()]).toEqual([null, ''])
    for (const p of ['obsidian', 'veil', 'yellow']) {
      store.getState().applyStyle({ autoPalettes: { dark: p, light: 'veil' } })
      expect([p, store.getState().accent, accentVar()]).toEqual([p, null, ''])
    }
  })

  it('a fresh client gets the default and the flag, and no accent key', async () => {
    mem.clear()
    const setItem = vi.spyOn(localStorage, 'setItem')
    try {
      const store = await freshStore()
      expect([store.getState().styleId, store.getState().accent]).toEqual(['glass', null])
      expect(setItem.mock.calls.filter(([k]) => k.startsWith('k2.accent.') && !k.startsWith('k2.accent.stamp.'))).toEqual([])
      expect(setItem.mock.calls.filter(([k]) => k === SWITCHED)).toHaveLength(1)
    } finally {
      setItem.mockRestore()
    }
    expect(mem.get(SWITCHED)).toBe('1')
    expect(mem.get('k2.style.migrated')).toBe('1')
  })

  it('switches a client whose older daemon migration is not done, and the daemon echo can’t undo it', async () => {
    seedOldTerminal()
    mem.delete('k2.style.migrated')
    const store = await freshStore()
    const { migrateStyleFromDaemon } = await import('./style')
    migrateStyleFromDaemon({ id: 'terminal', palette: 'paper', scheme: 'light', gaps: '' })
    expect([store.getState().styleId, store.getState().resolvedPaletteId]).toEqual(['glass', 'obsidian'])
    expect(mem.get('k2.style.migrated')).toBe('1')
  })

  it('does nothing when storage won’t keep the flag, so it never repeats every launch', async () => {
    seedOldTerminal()
    const setItem = vi.spyOn(localStorage, 'setItem').mockImplementation((k: string, v: string) => {
      if (k === SWITCHED) throw new Error('QuotaExceededError')
      mem.set(k, v)
    })
    try {
      const store = await freshStore()
      expect(store.getState().styleId).toBe('terminal')
    } finally {
      setItem.mockRestore()
    }
  })
})

describe('Gardens', () => {
  it('the shield re-points every accent property at a Zen token', async () => {
    const { ZEN_STYLE_SHIELD } = await import('@/lib/zen/zen-style-shield')
    const { ACCENT_VARS } = await import('@/lib/style-accent')
    for (const name of ACCENT_VARS) {
      if (name === '--glass-accent') continue // read only by the palette's own slots, which are shielded
      expect([name, ZEN_STYLE_SHIELD[name]?.startsWith('var(--zen-')]).toEqual([name, true])
    }
  })
})
