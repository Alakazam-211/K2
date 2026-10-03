// @vitest-environment jsdom
// "Open agents from other servers here" (MS55; prd-home-seamless-0432 Z1,
// Z2, Z4, Z28, T1.1, T1.2, T1.4; Rosson 2026-10-03 Q1, Q2): on by default on
// macOS and Linux, off by default on Windows, an explicit off (or on) is
// kept on every OS, junk is migrated to "never touched", per computer in
// localStorage, shared across windows, and the Settings row flips it.

import { describe, it, expect, beforeEach, vi } from 'vitest'
import { createElement } from 'react'
import { createRoot } from 'react-dom/client'
import { act } from 'react'
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'
import {
  LS_REMOTE_ROOMS_PREVIEW,
  REMOTE_ROOMS_DEFAULT_ON,
  installRemoteRoomsPreviewSync,
  migrateRemoteRoomsPreview,
  parseRemoteRoomsPreview,
  readRemoteRoomsPreview,
  remoteRoomsPreviewEnabled,
  useRemoteRoomsPreview,
  useRemoteRoomsPreviewStore,
  type PreviewStorage,
} from './remote-rooms-preview'
import { RemoteRoomsPreviewRow, REMOTE_ROOMS_LABEL } from '@/components/Settings/sections/RemoteRoomsPreviewRow'
import { webFeatures } from '@/web/features'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const OSES = ['mac', 'linux', 'windows', 'other'] as const

/** A storage whose every call throws (blocked site data). */
const blocked: PreviewStorage = {
  getItem: () => {
    throw new Error('blocked')
  },
  setItem: () => {
    throw new Error('blocked')
  },
  removeItem: () => {
    throw new Error('blocked')
  },
}

/** An in-memory storage that records writes. */
function memStorage(initial: string | null): PreviewStorage & { value: string | null; writes: string[] } {
  const s = {
    value: initial,
    writes: [] as string[],
    getItem: (k: string) => (k === LS_REMOTE_ROOMS_PREVIEW ? s.value : null),
    setItem: (k: string, v: string) => {
      if (k !== LS_REMOTE_ROOMS_PREVIEW) throw new Error(`unexpected key ${k}`)
      s.writes.push(`set:${v}`)
      s.value = v
    },
    removeItem: (k: string) => {
      if (k !== LS_REMOTE_ROOMS_PREVIEW) throw new Error(`unexpected key ${k}`)
      s.writes.push('remove')
      s.value = null
    },
  }
  return s
}

beforeEach(() => {
  localStorage.clear()
  useRemoteRoomsPreviewStore.setState({ enabled: false })
  vi.restoreAllMocks()
})

describe('the platform default (Q2)', () => {
  it('is on for macOS and Linux, off for Windows (until G-Win) and an unknown OS', () => {
    expect(REMOTE_ROOMS_DEFAULT_ON).toEqual({ mac: true, linux: true, windows: false, other: false })
  })

  it('a missing key reads the default for each OS', () => {
    expect(readRemoteRoomsPreview(memStorage(null), 'mac')).toBe(true)
    expect(readRemoteRoomsPreview(memStorage(null), 'linux')).toBe(true)
    expect(readRemoteRoomsPreview(memStorage(null), 'windows')).toBe(false)
    expect(readRemoteRoomsPreview(memStorage(null), 'other')).toBe(false)
  })
})

describe('what the key means (Z2, T1.1)', () => {
  it("'0' is off on every OS: an explicit off is kept (Q1)", () => {
    for (const os of OSES) expect([os, parseRemoteRoomsPreview('0', os)]).toEqual([os, false])
  })

  it("'1' is on on every OS, Windows included", () => {
    for (const os of OSES) expect([os, parseRemoteRoomsPreview('1', os)]).toEqual([os, true])
  })

  it("missing, blank or anything else reads the platform default", () => {
    for (const raw of [null, '', 'x', 'true', 'false', ' 1']) {
      expect([raw, parseRemoteRoomsPreview(raw, 'mac')]).toEqual([raw, true])
      expect([raw, parseRemoteRoomsPreview(raw, 'linux')]).toEqual([raw, true])
      expect([raw, parseRemoteRoomsPreview(raw, 'windows')]).toEqual([raw, false])
    }
  })

  it('blocked storage reads the platform default', () => {
    expect(readRemoteRoomsPreview(blocked, 'mac')).toBe(true)
    expect(readRemoteRoomsPreview(blocked, 'linux')).toBe(true)
    expect(readRemoteRoomsPreview(blocked, 'windows')).toBe(false)
    expect(readRemoteRoomsPreview(null, 'mac')).toBe(true)
    expect(readRemoteRoomsPreview(null, 'windows')).toBe(false)
  })

  it('the hosted web client always reads off, even with "1" stored (MS56)', () => {
    const features = webFeatures as { multiHost: boolean }
    expect(features.multiHost).toBe(true)
    features.multiHost = false
    try {
      expect(readRemoteRoomsPreview(memStorage('1'), 'mac')).toBe(false)
      expect(readRemoteRoomsPreview(memStorage(null), 'linux')).toBe(false)
    } finally {
      features.multiHost = true
    }
  })

  it('this window reads its own OS from the navigator', () => {
    vi.spyOn(navigator, 'platform', 'get').mockReturnValue('MacIntel')
    expect(readRemoteRoomsPreview()).toBe(true)
    vi.spyOn(navigator, 'platform', 'get').mockReturnValue('Linux x86_64')
    expect(readRemoteRoomsPreview()).toBe(true)
    vi.spyOn(navigator, 'platform', 'get').mockReturnValue('Win32')
    vi.spyOn(navigator, 'userAgent', 'get').mockReturnValue('Mozilla/5.0 (Windows NT 10.0; Win64; x64)')
    expect(readRemoteRoomsPreview()).toBe(false)
    localStorage.setItem(LS_REMOTE_ROOMS_PREVIEW, '1')
    expect(readRemoteRoomsPreview()).toBe(true)
  })
})

describe('the 0.43.2 migration (Q1)', () => {
  it("keeps '1' and '0' exactly, and writes nothing", () => {
    for (const raw of ['1', '0']) {
      const s = memStorage(raw)
      expect(migrateRemoteRoomsPreview(s)).toBe('kept')
      expect(s.value).toBe(raw)
      expect(s.writes).toEqual([])
    }
    // Someone who turned it off before 0.43.2 stays off after it.
    const off = memStorage('0')
    migrateRemoteRoomsPreview(off)
    expect(readRemoteRoomsPreview(off, 'mac')).toBe(false)
  })

  it('never writes the default for someone who never touched it (a later Windows flip still reaches them)', () => {
    const s = memStorage(null)
    expect(migrateRemoteRoomsPreview(s)).toBe('untouched')
    expect(s.writes).toEqual([])
    expect(s.value).toBe(null)
    expect(readRemoteRoomsPreview(s, 'mac')).toBe(true)
    expect(readRemoteRoomsPreview(s, 'windows')).toBe(false)
  })

  it('removes any other value (never written by the toggle): it then reads the default', () => {
    for (const raw of ['', 'true', 'junk']) {
      const s = memStorage(raw)
      expect(migrateRemoteRoomsPreview(s)).toBe('cleared')
      expect(s.writes).toEqual(['remove'])
      expect(s.value).toBe(null)
      expect(readRemoteRoomsPreview(s, 'linux')).toBe(true)
    }
  })

  it('blocked storage is left alone', () => {
    expect(migrateRemoteRoomsPreview(blocked)).toBe('untouched')
    expect(migrateRemoteRoomsPreview(null)).toBe('untouched')
  })

  it('runs when the store loads: a fresh window on a mac with junk stored reads on, an explicit off stays off', async () => {
    vi.spyOn(navigator, 'platform', 'get').mockReturnValue('MacIntel')
    localStorage.setItem(LS_REMOTE_ROOMS_PREVIEW, 'junk')
    vi.resetModules()
    const fresh = await import('./remote-rooms-preview')
    expect(fresh.useRemoteRoomsPreviewStore.getState().enabled).toBe(true)
    expect(localStorage.getItem(LS_REMOTE_ROOMS_PREVIEW)).toBe(null)

    localStorage.setItem(LS_REMOTE_ROOMS_PREVIEW, '0')
    vi.resetModules()
    const again = await import('./remote-rooms-preview')
    expect(again.useRemoteRoomsPreviewStore.getState().enabled).toBe(false)
    expect(localStorage.getItem(LS_REMOTE_ROOMS_PREVIEW)).toBe('0')

    localStorage.removeItem(LS_REMOTE_ROOMS_PREVIEW)
    vi.spyOn(navigator, 'platform', 'get').mockReturnValue('Win32')
    vi.resetModules()
    const windows = await import('./remote-rooms-preview')
    expect(windows.useRemoteRoomsPreviewStore.getState().enabled).toBe(false)
    expect(localStorage.getItem(LS_REMOTE_ROOMS_PREVIEW)).toBe(null)
  })
})

describe('writes (T1.2)', () => {
  it("setEnabled writes '1' / '0' and the store; a reload reads it back", () => {
    useRemoteRoomsPreviewStore.getState().setEnabled(true)
    expect(localStorage.getItem(LS_REMOTE_ROOMS_PREVIEW)).toBe('1')
    expect(remoteRoomsPreviewEnabled()).toBe(true)
    expect(readRemoteRoomsPreview(localStorage, 'windows')).toBe(true)
    useRemoteRoomsPreviewStore.getState().setEnabled(false)
    expect(localStorage.getItem(LS_REMOTE_ROOMS_PREVIEW)).toBe('0')
    expect(remoteRoomsPreviewEnabled()).toBe(false)
    expect(readRemoteRoomsPreview(localStorage, 'mac')).toBe(false)
  })

  it('nothing else in src/renderer writes the key, and no "(preview)" copy is left (T1.2, T1.4)', () => {
    const root = join(__dirname, '..')
    const files: string[] = []
    const walk = (dir: string): void => {
      for (const name of readdirSync(dir)) {
        const p = join(dir, name)
        if (statSync(p).isDirectory()) walk(p)
        else if (/\.(ts|tsx)$/.test(name) && !/\.(test|mstest)\.tsx?$/.test(name)) files.push(p)
      }
    }
    walk(root)
    expect(files.length).toBeGreaterThan(100)
    const writers: string[] = []
    const preview: string[] = []
    for (const f of files) {
      const text = readFileSync(f, 'utf8')
      const rel = relative(root, f)
      if (/setItem\(\s*(LS_REMOTE_ROOMS_PREVIEW|['"]k2\.homeRemoteRooms\.v1['"])/.test(text)) writers.push(rel)
      if (text.includes('(preview)')) preview.push(rel)
    }
    expect(writers).toEqual(['lib/remote-rooms-preview.ts'])
    expect(preview).toEqual([])
  })
})

describe('across windows and React', () => {
  it('another window changing it reaches this one (storage event)', () => {
    vi.spyOn(navigator, 'platform', 'get').mockReturnValue('MacIntel')
    const target = new EventTarget() as unknown as Window
    const uninstall = installRemoteRoomsPreviewSync(target)
    localStorage.setItem(LS_REMOTE_ROOMS_PREVIEW, '1')
    target.dispatchEvent(new StorageEvent('storage', { key: LS_REMOTE_ROOMS_PREVIEW }))
    expect(remoteRoomsPreviewEnabled()).toBe(true)
    localStorage.setItem(LS_REMOTE_ROOMS_PREVIEW, '0')
    // An unrelated key changes nothing.
    target.dispatchEvent(new StorageEvent('storage', { key: 'k2.homes.v1' }))
    expect(remoteRoomsPreviewEnabled()).toBe(true)
    target.dispatchEvent(new StorageEvent('storage', { key: LS_REMOTE_ROOMS_PREVIEW }))
    expect(remoteRoomsPreviewEnabled()).toBe(false)
    // A clear() elsewhere (key null) re-reads: the mac default, on.
    localStorage.clear()
    target.dispatchEvent(new StorageEvent('storage', { key: null }))
    expect(remoteRoomsPreviewEnabled()).toBe(true)
    uninstall()
    localStorage.setItem(LS_REMOTE_ROOMS_PREVIEW, '0')
    target.dispatchEvent(new StorageEvent('storage', { key: LS_REMOTE_ROOMS_PREVIEW }))
    expect(remoteRoomsPreviewEnabled()).toBe(true)
  })

  it('the hook re-renders on change', () => {
    const seen: boolean[] = []
    function Probe(): null {
      seen.push(useRemoteRoomsPreview())
      return null
    }
    const el = document.createElement('div')
    const root = createRoot(el)
    act(() => root.render(createElement(Probe)))
    act(() => useRemoteRoomsPreviewStore.getState().setEnabled(true))
    act(() => root.unmount())
    expect(seen[0]).toBe(false)
    expect(seen[seen.length - 1]).toBe(true)
  })

  it('the Settings row: plain label, shows the state, and turning it off writes "0"', () => {
    useRemoteRoomsPreviewStore.setState({ enabled: true })
    const el = document.createElement('div')
    document.body.appendChild(el)
    const root = createRoot(el)
    act(() => root.render(createElement(RemoteRoomsPreviewRow)))
    expect(REMOTE_ROOMS_LABEL).toBe('Open agents from other servers here')
    expect(el.textContent).toContain('Open agents from other servers here')
    expect(el.textContent).toContain('Off: it switches this window to that server.')
    const sw = el.querySelector(`[aria-label="${REMOTE_ROOMS_LABEL}"]`)
    if (!sw) throw new Error('no switch rendered')
    expect(sw.getAttribute('aria-checked')).toBe('true')
    act(() => (sw as HTMLElement).click())
    expect(remoteRoomsPreviewEnabled()).toBe(false)
    expect(localStorage.getItem(LS_REMOTE_ROOMS_PREVIEW)).toBe('0')
    expect(sw.getAttribute('aria-checked')).toBe('false')
    act(() => root.unmount())
    el.remove()
  })
})
