import { describe, expect, it, vi, afterEach } from 'vitest'
import { randomUUID, installRandomUUIDPolyfill } from './random-uuid'

const UUID_V4 =
  /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i

describe('randomUUID', () => {
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('returns a v4-shaped id when native crypto.randomUUID exists', () => {
    const id = randomUUID()
    expect(id).toMatch(UUID_V4)
  })

  it('falls back to getRandomValues when randomUUID is missing', () => {
    const real = globalThis.crypto
    const getRandomValues = real.getRandomValues.bind(real)
    vi.stubGlobal('crypto', {
      getRandomValues,
      // no randomUUID
    })
    const id = randomUUID()
    expect(id).toMatch(UUID_V4)
  })
})

describe('installRandomUUIDPolyfill', () => {
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('defines crypto.randomUUID when absent', () => {
    const real = globalThis.crypto
    const getRandomValues = real.getRandomValues.bind(real)
    const fake = { getRandomValues } as Crypto
    vi.stubGlobal('crypto', fake)
    expect(typeof (crypto as Crypto).randomUUID).not.toBe('function')
    installRandomUUIDPolyfill()
    expect(typeof crypto.randomUUID).toBe('function')
    expect(crypto.randomUUID()).toMatch(UUID_V4)
  })
})
