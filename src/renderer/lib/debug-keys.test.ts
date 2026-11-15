import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { debugKeyNames, readDebugKey } from './debug-keys'

// The suite runs in node, which has no localStorage: each test gets its own.
function memStorage(): Pick<Storage, 'getItem' | 'setItem'> {
  const m = new Map<string, string>()
  return {
    getItem: (k: string) => (m.has(k) ? m.get(k)! : null),
    setItem: (k: string, v: string) => {
      m.set(k, v)
    },
  }
}

describe('debug keys', () => {
  beforeEach(() => {
    vi.stubGlobal('localStorage', memStorage())
  })
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('each switch has a K5_ key first and keeps its K2SO_ key', () => {
    expect(debugKeyNames('WEBGL_DIAG')).toEqual(['K5_WEBGL_DIAG', 'K2SO_WEBGL_DIAG'])
  })

  it('is null when neither key is set', () => {
    expect(readDebugKey('WEBGL_DIAG')).toBeNull()
    localStorage.setItem('K5_OTHER', '1')
    expect(readDebugKey('WEBGL_DIAG')).toBeNull()
  })

  it('reads the old key', () => {
    localStorage.setItem('K2SO_WEBGL_DIAG', '1')
    expect(readDebugKey('WEBGL_DIAG')).toBe('1')
  })

  it('reads the new key', () => {
    localStorage.setItem('K5_WEBGL_DIAG', '1')
    expect(readDebugKey('WEBGL_DIAG')).toBe('1')
  })

  it('the new key wins when both are set, whatever its value', () => {
    localStorage.setItem('K2SO_WEBGL_TEXT_GAMMA', '2.5')
    localStorage.setItem('K5_WEBGL_TEXT_GAMMA', '1.2')
    expect(readDebugKey('WEBGL_TEXT_GAMMA')).toBe('1.2')
    localStorage.setItem('K5_WEBGL_TEXT_GAMMA', '')
    expect(readDebugKey('WEBGL_TEXT_GAMMA')).toBe('')
  })

  it('is null, and does not throw, when localStorage throws', () => {
    vi.stubGlobal('localStorage', {
      getItem: () => {
        throw new Error('blocked')
      },
    })
    expect(readDebugKey('WEBGL_DIAG')).toBeNull()
  })

  it('is null when there is no localStorage', () => {
    vi.stubGlobal('localStorage', undefined)
    expect(readDebugKey('WEBGL_DIAG')).toBeNull()
  })
})
