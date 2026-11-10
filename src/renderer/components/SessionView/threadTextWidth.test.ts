import { beforeEach, describe, expect, it, vi } from 'vitest'
import {
  LS_THREAD_TEXT_WIDTH,
  THREAD_TEXT_WIDTH_DEFAULT,
  clampThreadTextWidthPx,
  isThreadTextWidthDefault,
  normalizeThreadTextWidth,
  parseThreadTextWidth,
  readThreadTextWidth,
  threadTextCapPx,
  writeThreadTextWidth,
} from './threadTextWidth'

const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => (mem.has(k) ? mem.get(k)! : null),
  setItem: (k: string, v: string) => void mem.set(k, v),
  removeItem: (k: string) => void mem.delete(k),
})

describe('threadTextWidth', () => {
  beforeEach(() => mem.clear())

  it('clamps to the range and snaps to the 20px steps', () => {
    expect(clampThreadTextWidthPx(100)).toBe(480)
    expect(clampThreadTextWidthPx(5000)).toBe(960)
    expect(clampThreadTextWidthPx(733)).toBe(740)
    expect(clampThreadTextWidthPx(729)).toBe(720)
    expect(clampThreadTextWidthPx(480)).toBe(480)
    expect(clampThreadTextWidthPx(960)).toBe(960)
  })

  it('reads anything that is not a finite number as the default width', () => {
    for (const bad of [NaN, Infinity, '720', null, undefined, {}]) {
      expect(clampThreadTextWidthPx(bad)).toBe(820)
    }
  })

  it('normalizes bad shapes to the default and keeps Full width', () => {
    expect(normalizeThreadTextWidth(null)).toEqual(THREAD_TEXT_WIDTH_DEFAULT)
    expect(normalizeThreadTextWidth('x')).toEqual(THREAD_TEXT_WIDTH_DEFAULT)
    expect(normalizeThreadTextWidth({ limited: false, px: 9999 })).toEqual({ limited: false, px: 960 })
    expect(normalizeThreadTextWidth({ px: 500 })).toEqual({ limited: true, px: 500 })
  })

  it('parses bad JSON as the default', () => {
    expect(parseThreadTextWidth('{not json')).toEqual(THREAD_TEXT_WIDTH_DEFAULT)
    expect(parseThreadTextWidth(null)).toEqual(THREAD_TEXT_WIDTH_DEFAULT)
    expect(parseThreadTextWidth('{"limited":true,"px":600}')).toEqual({ limited: true, px: 600 })
  })

  it('round-trips through its localStorage key, normalized', () => {
    writeThreadTextWidth({ limited: false, px: 1000 })
    expect(JSON.parse(mem.get(LS_THREAD_TEXT_WIDTH)!)).toEqual({ limited: false, px: 960 })
    expect(readThreadTextWidth()).toEqual({ limited: false, px: 960 })
  })

  it('caps only when limited', () => {
    expect(threadTextCapPx({ limited: true, px: 640 })).toBe(640)
    expect(threadTextCapPx({ limited: false, px: 640 })).toBeNull()
    expect(threadTextCapPx(undefined)).toBe(820)
  })

  it('knows the default for the Reset link', () => {
    expect(isThreadTextWidthDefault(THREAD_TEXT_WIDTH_DEFAULT)).toBe(true)
    expect(isThreadTextWidthDefault({ limited: false, px: 720 })).toBe(false)
    expect(isThreadTextWidthDefault({ limited: true, px: 740 })).toBe(false)
  })
})
