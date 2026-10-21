// @vitest-environment jsdom
// "Open other servers' agents in Home" (MS55; prd-home-seamless-0432 Z1,
// Z2, Z4, Z28, T1.1, T1.2, T1.4; Rosson 2026-10-03 Q1, Q2; Rosson
// 2026-10-11; graduated Rosson 2026-10-21): on by default on every OS from
// 0.45.5 (Windows included), an explicit off (or on) is kept on every OS,
// junk is migrated to "never touched", an Experimental-era off is reset
// exactly once (0.46.3 graduation), per computer in localStorage, shared
// across windows, and the Settings row flips it.

import { describe, it, expect, beforeEach, vi } from 'vitest'
import { createElement } from 'react'
import { createRoot } from 'react-dom/client'
import { act } from 'react'
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'
import {
  LS_REMOTE_ROOMS_GRADUATED,
  LS_REMOTE_ROOMS_PREVIEW,
  REMOTE_ROOMS_DEFAULT_ON,
  graduateRemoteRoomsPreview,
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

/** An in-memory storage that records writes. `value` is the switch key,
 *  `flag` the graduation flag; any other key throws. Writes to the switch
 *  are recorded as `set:<v>` / `remove`, to the flag as `flag:<v>` /
 *  `flag:remove`. */
function memStorage(
  initial: string | null,
  flag: string | null = null,
): PreviewStorage & { value: string | null; flag: string | null; writes: string[] } {
  const s = {
    value: initial,
    flag,
    writes: [] as string[],
    getItem: (k: string) => {
      if (k === LS_REMOTE_ROOMS_PREVIEW) return s.value
      if (k === LS_REMOTE_ROOMS_GRADUATED) return s.flag
      throw new Error(`unexpected key ${k}`)
    },
    setItem: (k: string, v: string) => {
      if (k === LS_REMOTE_ROOMS_PREVIEW) {
        s.writes.push(`set:${v}`)
        s.value = v
      } else if (k === LS_REMOTE_ROOMS_GRADUATED) {
        s.writes.push(`flag:${v}`)
        s.flag = v
      } else throw new Error(`unexpected key ${k}`)
    },
    removeItem: (k: string) => {
      if (k === LS_REMOTE_ROOMS_PREVIEW) {
        s.writes.push('remove')
        s.value = null
      } else if (k === LS_REMOTE_ROOMS_GRADUATED) {
        s.writes.push('flag:remove')
        s.flag = null
      } else throw new Error(`unexpected key ${k}`)
    },
  }
  return s
}

beforeEach(() => {
  localStorage.clear()
  useRemoteRoomsPreviewStore.setState({ enabled: false })
  vi.restoreAllMocks()
})

describe('the default (Q2; 0.45.5 on everywhere)', () => {
  it('is on for every OS, Windows and an unknown OS included', () => {
    expect(REMOTE_ROOMS_DEFAULT_ON).toEqual({ mac: true, linux: true, windows: true, other: true })
  })

  it('a missing key reads on, on every OS', () => {
    for (const os of OSES) expect([os, readRemoteRoomsPreview(memStorage(null), os)]).toEqual([os, true])
  })
})

describe('what the key means (Z2, T1.1)', () => {
  it("'0' is off on every OS: an explicit off is kept (Q1)", () => {
    for (const os of OSES) expect([os, parseRemoteRoomsPreview('0', os)]).toEqual([os, false])
  })

  it("'1' is on on every OS, Windows included", () => {
    for (const os of OSES) expect([os, parseRemoteRoomsPreview('1', os)]).toEqual([os, true])
  })

  it("missing, blank or anything else reads the default (on) on every OS", () => {
    for (const raw of [null, '', 'x', 'true', 'false', ' 1']) {
      for (const os of OSES) expect([raw, os, parseRemoteRoomsPreview(raw, os)]).toEqual([raw, os, true])
    }
  })

  it('blocked storage reads the default (on) on every OS', () => {
    for (const os of OSES) {
      expect([os, readRemoteRoomsPreview(blocked, os)]).toEqual([os, true])
      expect([os, readRemoteRoomsPreview(null, os)]).toEqual([os, true])
    }
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
    expect(readRemoteRoomsPreview()).toBe(true)
    localStorage.setItem(LS_REMOTE_ROOMS_PREVIEW, '0')
    expect(readRemoteRoomsPreview()).toBe(false)
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
    // The 0.45.5 Windows flip reaches them with no migration.
    expect(readRemoteRoomsPreview(s, 'windows')).toBe(true)
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
    // Past the 0.46.3 graduation, so a '0' below is a post-graduation off.
    localStorage.setItem(LS_REMOTE_ROOMS_GRADUATED, '1')
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
    vi.spyOn(navigator, 'userAgent', 'get').mockReturnValue('Mozilla/5.0 (Windows NT 10.0; Win64; x64)')
    vi.resetModules()
    const windows = await import('./remote-rooms-preview')
    expect(windows.useRemoteRoomsPreviewStore.getState().enabled).toBe(true)
    // The default is never written: nothing stored after the load.
    expect(localStorage.getItem(LS_REMOTE_ROOMS_PREVIEW)).toBe(null)
  })
})

describe('the 0.45.5 default flip (Rosson 2026-10-11)', () => {
  // Only the toggle has ever written the key ('1' / '0', since bd76de15d),
  // so a stored '0' is always an explicit off and a missing key is "never
  // touched". No migration is needed to tell them apart.
  async function loadOn(platform: string, ua: string): Promise<boolean> {
    vi.spyOn(navigator, 'platform', 'get').mockReturnValue(platform)
    vi.spyOn(navigator, 'userAgent', 'get').mockReturnValue(ua)
    vi.resetModules()
    const fresh = await import('./remote-rooms-preview')
    return fresh.useRemoteRoomsPreviewStore.getState().enabled
  }
  const PLATFORMS: ReadonlyArray<readonly [string, string, string]> = [
    ['mac', 'MacIntel', 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)'],
    ['linux', 'Linux x86_64', 'Mozilla/5.0 (X11; Linux x86_64)'],
    ['windows', 'Win32', 'Mozilla/5.0 (Windows NT 10.0; Win64; x64)'],
    ['other', 'FreeBSD amd64', 'Mozilla/5.0 (X11; FreeBSD amd64)'],
  ]

  it('someone who never touched it loads on, on every OS (Windows included)', async () => {
    for (const [name, platform, ua] of PLATFORMS) {
      localStorage.removeItem(LS_REMOTE_ROOMS_PREVIEW)
      expect([name, await loadOn(platform, ua)]).toEqual([name, true])
      expect([name, localStorage.getItem(LS_REMOTE_ROOMS_PREVIEW)]).toEqual([name, null])
    }
  })

  it('an explicit off (a stored "0", after the 0.46.3 graduation) loads off on every OS and is left as is', async () => {
    localStorage.setItem(LS_REMOTE_ROOMS_GRADUATED, '1')
    for (const [name, platform, ua] of PLATFORMS) {
      localStorage.setItem(LS_REMOTE_ROOMS_PREVIEW, '0')
      expect([name, await loadOn(platform, ua)]).toEqual([name, false])
      expect([name, localStorage.getItem(LS_REMOTE_ROOMS_PREVIEW)]).toEqual([name, '0'])
    }
  })
})

describe('the 0.46.3 graduation (Rosson 2026-10-21)', () => {
  it("resets an Experimental-era '0' exactly once: flag first, then the '0' is removed and it reads on", () => {
    const s = memStorage('0')
    expect(graduateRemoteRoomsPreview(s)).toBe('reset')
    expect(s.writes).toEqual(['flag:1', 'remove'])
    expect(s.value).toBe(null)
    expect(s.flag).toBe('1')
    for (const os of OSES) expect([os, readRemoteRoomsPreview(s, os)]).toEqual([os, true])
    // A second run (the next load, another window) does nothing.
    expect(graduateRemoteRoomsPreview(s)).toBe('already')
    expect(s.writes).toEqual(['flag:1', 'remove'])
  })

  it("a '0' set after the flag (turned off after graduation) is kept forever", () => {
    const s = memStorage(null)
    expect(graduateRemoteRoomsPreview(s)).toBe('marked')
    expect(s.writes).toEqual(['flag:1'])
    // The person turns it off after graduation.
    s.setItem(LS_REMOTE_ROOMS_PREVIEW, '0')
    for (let i = 0; i < 3; i++) {
      expect(graduateRemoteRoomsPreview(s)).toBe('already')
      expect(migrateRemoteRoomsPreview(s)).toBe('kept')
    }
    expect(s.value).toBe('0')
    expect(s.writes).toEqual(['flag:1', 'set:0'])
    expect(readRemoteRoomsPreview(s, 'mac')).toBe(false)
  })

  it("never touches '1' (the flag is set, the '1' stays)", () => {
    const s = memStorage('1')
    expect(graduateRemoteRoomsPreview(s)).toBe('marked')
    expect(s.value).toBe('1')
    expect(s.writes).toEqual(['flag:1'])
    const done = memStorage('1', '1')
    expect(graduateRemoteRoomsPreview(done)).toBe('already')
    expect(done.value).toBe('1')
    expect(done.writes).toEqual([])
  })

  it('a missing key stays missing (the default is never written); only the flag is set', () => {
    const s = memStorage(null)
    expect(graduateRemoteRoomsPreview(s)).toBe('marked')
    expect(s.value).toBe(null)
    expect(s.writes).toEqual(['flag:1'])
  })

  it("if the flag cannot be written, the '0' is kept (no reset on every load)", () => {
    const s = memStorage('0')
    const failing: PreviewStorage = {
      getItem: (k) => s.getItem(k),
      setItem: (k) => {
        throw new Error(`quota: ${k}`)
      },
      removeItem: (k) => s.removeItem(k),
    }
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    expect(graduateRemoteRoomsPreview(failing)).toBe('untouched')
    expect(s.value).toBe('0')
    expect(s.writes).toEqual([])
  })

  it('blocked storage is left alone', () => {
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    expect(graduateRemoteRoomsPreview(blocked)).toBe('untouched')
    expect(graduateRemoteRoomsPreview(null)).toBe('untouched')
  })

  it('the hosted web client is left alone and still reads off', () => {
    const features = webFeatures as { multiHost: boolean }
    expect(features.multiHost).toBe(true)
    features.multiHost = false
    try {
      const s = memStorage('0')
      expect(graduateRemoteRoomsPreview(s)).toBe('untouched')
      expect(s.writes).toEqual([])
      expect(readRemoteRoomsPreview(memStorage(null), 'mac')).toBe(false)
    } finally {
      features.multiHost = true
    }
  })

  it("runs when the store loads: a pre-0.46.3 '0' loads on once, then a new '0' loads off on every later load", async () => {
    vi.spyOn(navigator, 'platform', 'get').mockReturnValue('MacIntel')
    localStorage.setItem(LS_REMOTE_ROOMS_PREVIEW, '0')
    expect(localStorage.getItem(LS_REMOTE_ROOMS_GRADUATED)).toBe(null)
    vi.resetModules()
    const first = await import('./remote-rooms-preview')
    expect(first.useRemoteRoomsPreviewStore.getState().enabled).toBe(true)
    expect(localStorage.getItem(LS_REMOTE_ROOMS_PREVIEW)).toBe(null)
    expect(localStorage.getItem(LS_REMOTE_ROOMS_GRADUATED)).toBe('1')

    // The person turns it off again after graduation, through the toggle.
    first.useRemoteRoomsPreviewStore.getState().setEnabled(false)
    expect(localStorage.getItem(LS_REMOTE_ROOMS_PREVIEW)).toBe('0')
    for (let i = 0; i < 2; i++) {
      vi.resetModules()
      const later = await import('./remote-rooms-preview')
      expect(later.useRemoteRoomsPreviewStore.getState().enabled).toBe(false)
      expect(localStorage.getItem(LS_REMOTE_ROOMS_PREVIEW)).toBe('0')
    }
  })

  it("runs when the store loads: a stored '1' loads on and is untouched", async () => {
    vi.spyOn(navigator, 'platform', 'get').mockReturnValue('MacIntel')
    localStorage.setItem(LS_REMOTE_ROOMS_PREVIEW, '1')
    vi.resetModules()
    const fresh = await import('./remote-rooms-preview')
    expect(fresh.useRemoteRoomsPreviewStore.getState().enabled).toBe(true)
    expect(localStorage.getItem(LS_REMOTE_ROOMS_PREVIEW)).toBe('1')
    expect(localStorage.getItem(LS_REMOTE_ROOMS_GRADUATED)).toBe('1')
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

  it('nothing else in src/renderer writes the key, and no "Remote rooms (preview)" copy is left (T1.2, T1.4)', () => {
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
      // T1.4 is about Remote rooms' old "(preview)" label. Compute nodes are
      // still a preview by design (0.45.3+, Settings → Compute), so match
      // the rooms label, not every "(preview)".
      if (/rooms \(preview\)/i.test(text)) preview.push(rel)
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
    expect(REMOTE_ROOMS_LABEL).toBe("Open other servers' agents in Home (don't switch servers)")
    expect(el.textContent).toContain("Open other servers' agents in Home (don't switch servers)")
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
