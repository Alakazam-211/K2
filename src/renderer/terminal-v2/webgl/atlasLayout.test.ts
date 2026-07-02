import { describe, it, expect } from 'vitest'
import {
  allocSlot,
  ATLAS_MAX_SIZE,
  createLayout,
  growLayout,
} from './atlasLayout'

describe('atlasLayout — shelf packing', () => {
  it('packs left→right on one shelf', () => {
    const l = createLayout(64)
    expect(allocSlot(l, 10, 20)).toEqual({ x: 0, y: 0 })
    expect(allocSlot(l, 10, 20)).toEqual({ x: 10, y: 0 })
    expect(allocSlot(l, 20, 20)).toEqual({ x: 20, y: 0 })
  })

  it('wraps to a new shelf when the row is full', () => {
    const l = createLayout(32)
    expect(allocSlot(l, 20, 10)).toEqual({ x: 0, y: 0 })
    // 20 + 20 > 32 → wrap.
    expect(allocSlot(l, 20, 10)).toEqual({ x: 0, y: 10 })
    expect(allocSlot(l, 10, 10)).toEqual({ x: 20, y: 10 })
  })

  it('shelf height is the tallest slot on the shelf', () => {
    const l = createLayout(32)
    allocSlot(l, 10, 8)
    allocSlot(l, 10, 12) // grows the shelf
    allocSlot(l, 20, 8) // wraps: 20+20 > 32
    expect(l.shelfY).toBe(12)
  })

  it('returns null when the page is full', () => {
    const l = createLayout(16)
    expect(allocSlot(l, 16, 16)).toEqual({ x: 0, y: 0 })
    expect(allocSlot(l, 16, 16)).toBeNull()
    expect(allocSlot(l, 32, 8)).toBeNull() // oversized
  })

  it('existing coordinates stay valid across growth', () => {
    const l = createLayout(16)
    const a = allocSlot(l, 16, 16)
    expect(a).toEqual({ x: 0, y: 0 })
    expect(allocSlot(l, 16, 16)).toBeNull()
    expect(growLayout(l)).toBe(true)
    expect(l.size).toBe(32)
    // Packing continues where it left off — no reflow of `a`.
    expect(allocSlot(l, 16, 16)).toEqual({ x: 16, y: 0 })
  })

  it('refuses to grow past the cap', () => {
    const l = createLayout(ATLAS_MAX_SIZE)
    expect(growLayout(l)).toBe(false)
  })
})
