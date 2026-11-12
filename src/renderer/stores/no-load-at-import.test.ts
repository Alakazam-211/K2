/**
 * No store or lib module starts a load when it is imported.
 *
 * The stores used to fetch from the daemon the moment they were first
 * imported ("Initialize on import"), so every test that imported one
 * fired a request into whatever mock was installed. The first loads now
 * live in boot functions that the app calls once (boot-stores.ts from
 * index.tsx, boot-app-stores.ts from App.tsx). This keeps it that way.
 *
 * Listening is fine at import (onDaemonConnected, onActiveHostChange):
 * it asks the daemon nothing.
 */
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'
import { describe, expect, it } from 'vitest'

const RENDERER = join(__dirname, '..')

/** A statement at the top level of a module (column 0) that starts a load. */
const LOAD_AT_IMPORT = /^(?:void\s+)?(?:use[A-Z]\w*\.getState\(\)\.(?:fetch|init|load)\w*\(|init[A-Z]\w*\(|import\()/

function sources(dir: string): string[] {
  const out: string[] = []
  for (const name of readdirSync(dir)) {
    const p = join(dir, name)
    if (statSync(p).isDirectory()) out.push(...sources(p))
    else if (/\.tsx?$/.test(name) && !/\.test\.tsx?$/.test(name)) out.push(p)
  }
  return out
}

function loadsAtImport(text: string): string[] {
  return text.split('\n').filter((line) => LOAD_AT_IMPORT.test(line))
}

describe('no load at import', () => {
  it('the check knows the shapes the stores used to have', () => {
    expect(loadsAtImport('useSettingsStore.getState().fetchSettings()')).toHaveLength(1)
    expect(loadsAtImport('usePanelsStore.getState().initFromSettings()')).toHaveLength(1)
    expect(loadsAtImport('void initTerminalZoomListeners().catch(() => {})')).toHaveLength(1)
    expect(loadsAtImport("import('./custom-themes').then(({ useCustomThemesStore }) => {")).toHaveLength(1)
    // Inside a function (indented), or only listening: fine.
    expect(loadsAtImport('  useSettingsStore.getState().fetchSettings()')).toEqual([])
    expect(loadsAtImport('onDaemonConnected(refetchGroupsIfNeeded)')).toEqual([])
    expect(loadsAtImport("import { create } from 'zustand'")).toEqual([])
  })

  it('no module under stores/ or lib/ starts a load at its top level', () => {
    const files = [...sources(join(RENDERER, 'stores')), ...sources(join(RENDERER, 'lib'))]
    expect(files.length).toBeGreaterThan(100)
    const found = files.flatMap((f) => loadsAtImport(readFileSync(f, 'utf8')).map((line) => `${relative(RENDERER, f)}: ${line}`))
    expect(found, 'move each into a boot function called from boot-stores.ts or boot-app-stores.ts').toEqual([])
  })
})
