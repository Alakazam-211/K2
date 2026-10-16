// prd-wiki-3d-view-v1 §6.6 / §8.1: the per-window Wiki view choice.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const webFlag = vi.hoisted(() => ({ value: false }))
const windowLabel = vi.hoisted(() => ({ value: 'main' }))

vi.mock('@/lib/is-web', () => ({ isWebClient: () => webFlag.value }))
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ label: windowLabel.value }),
}))

class MemoryStorage {
  map = new Map<string, string>()
  getItem(k: string): string | null {
    return this.map.has(k) ? (this.map.get(k) as string) : null
  }
  setItem(k: string, v: string): void {
    this.map.set(k, v)
  }
  removeItem(k: string): void {
    this.map.delete(k)
  }
  clear(): void {
    this.map.clear()
  }
}

const localMem = new MemoryStorage()
const sessionMem = new MemoryStorage()
vi.stubGlobal('localStorage', localMem)
vi.stubGlobal('sessionStorage', sessionMem)

import { DEFAULT_WIKI_VIEW, readWikiViewPref, wikiViewKey, writeWikiViewPref } from './wiki-view-pref'

beforeEach(() => {
  webFlag.value = false
  windowLabel.value = 'main'
  localMem.clear()
  sessionMem.clear()
})

afterEach(() => {
  vi.stubGlobal('localStorage', localMem)
  vi.stubGlobal('sessionStorage', sessionMem)
})

describe('wiki view pref', () => {
  it('defaults to the standard Wiki view', () => {
    expect(DEFAULT_WIKI_VIEW).toBe('wiki')
    expect(readWikiViewPref()).toBe('wiki')
  })

  it('round-trips per window label', () => {
    writeWikiViewPref('3d')
    windowLabel.value = 'win-2'
    expect(readWikiViewPref()).toBe('wiki')
    writeWikiViewPref('2d')
    expect(readWikiViewPref()).toBe('2d')
    windowLabel.value = 'main'
    expect(readWikiViewPref()).toBe('3d')
    expect(localMem.getItem(wikiViewKey('main'))).toBe('3d')
    expect(localMem.getItem('k2.wikiView.win-2')).toBe('2d')
  })

  it('a bad stored value reads as the default', () => {
    localMem.setItem(wikiViewKey('main'), 'vr')
    expect(readWikiViewPref()).toBe('wiki')
  })

  it('the web client uses sessionStorage', () => {
    webFlag.value = true
    writeWikiViewPref('2d', 'main')
    expect(sessionMem.getItem('k2.wikiView.main')).toBe('2d')
    expect(localMem.getItem('k2.wikiView.main')).toBeNull()
    expect(readWikiViewPref('main')).toBe('2d')
  })

  it('a throwing storage reads the default and the write does not throw', () => {
    const boom = {
      getItem: () => {
        throw new Error('blocked')
      },
      setItem: () => {
        throw new Error('quota')
      },
    }
    vi.stubGlobal('localStorage', boom)
    expect(readWikiViewPref()).toBe('wiki')
    expect(() => writeWikiViewPref('3d')).not.toThrow()
  })
})
