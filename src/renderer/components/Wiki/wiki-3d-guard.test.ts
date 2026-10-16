// prd-wiki-3d-view-v1 §8.1: the 3D probe, node cap, fps classifier and
// fallback copy (node env, fake canvas).

import { describe, expect, it, vi } from 'vitest'
import {
  classifyFps,
  fallbackToastCopy,
  MAX_3D_NODES,
  NO_WEBGL2_TITLE,
  probeWiki3d,
  tooManyFor3d,
} from './wiki-3d-guard'

type Behaviour = {
  /** Context returned with failIfMajorPerformanceCaveat. */
  strict: boolean
  /** Context returned without the flag. */
  plain: boolean
  renderer?: string | null
}

function fakeDoc(b: Behaviour) {
  const loseContext = vi.fn()
  const calls: { attrs: unknown }[] = []
  const doc = {
    createElement: (tag: string) => {
      if (tag !== 'canvas') throw new Error(`unexpected element ${tag}`)
      return {
        getContext: (kind: string, attrs?: { failIfMajorPerformanceCaveat?: boolean }) => {
          if (kind !== 'webgl2') throw new Error(`unexpected context ${kind}`)
          calls.push({ attrs })
          const ok = attrs?.failIfMajorPerformanceCaveat ? b.strict : b.plain
          if (!ok) return null
          return {
            getExtension: (name: string) => {
              if (name === 'WEBGL_lose_context') return { loseContext }
              if (name === 'WEBGL_debug_renderer_info') {
                return b.renderer === undefined ? null : { UNMASKED_RENDERER_WEBGL: 0x9246 }
              }
              return null
            },
            getParameter: (p: number) => {
              if (p !== 0x9246) throw new Error(`unexpected param ${p}`)
              return b.renderer
            },
          }
        },
      }
    },
  } as unknown as Document
  return { doc, loseContext, calls }
}

describe('probeWiki3d', () => {
  it('ok: hardware WebGL 2, context released', () => {
    const f = fakeDoc({ strict: true, plain: true, renderer: 'Apple M1' })
    expect(probeWiki3d(f.doc)).toEqual({ blockReason: null, renderer: 'Apple M1' })
    expect(f.loseContext).toHaveBeenCalledTimes(1)
    expect(f.calls).toEqual([{ attrs: { failIfMajorPerformanceCaveat: true } }])
  })

  it('ok when the GPU name is masked', () => {
    const f = fakeDoc({ strict: true, plain: true })
    expect(probeWiki3d(f.doc)).toEqual({ blockReason: null, renderer: null })
    expect(f.loseContext).toHaveBeenCalledTimes(1)
  })

  it('noWebGL2: no context with or without the caveat flag', () => {
    const f = fakeDoc({ strict: false, plain: false })
    expect(probeWiki3d(f.doc)).toEqual({ blockReason: 'noWebGL2', renderer: null })
    expect(f.calls).toHaveLength(2)
    expect(f.loseContext).not.toHaveBeenCalled()
  })

  it('software: the caveat fails but plain WebGL 2 works, context released', () => {
    const f = fakeDoc({ strict: false, plain: true, renderer: 'ANGLE (Metal)' })
    expect(probeWiki3d(f.doc)).toEqual({ blockReason: 'software', renderer: 'ANGLE (Metal)' })
    expect(f.loseContext).toHaveBeenCalledTimes(1)
  })

  it('software: the GPU name says llvmpipe / SwiftShader even when the caveat passes', () => {
    for (const name of ['llvmpipe (LLVM 15.0.7, 256 bits)', 'Google SwiftShader', 'softpipe', 'Software Rasterizer']) {
      const f = fakeDoc({ strict: true, plain: true, renderer: name })
      expect(probeWiki3d(f.doc)).toEqual({ blockReason: 'software', renderer: name })
      expect(f.loseContext).toHaveBeenCalledTimes(1)
    }
  })

  it('no document → noWebGL2', () => {
    expect(probeWiki3d(undefined)).toEqual({ blockReason: 'noWebGL2', renderer: null })
  })
})

describe('classifyFps', () => {
  it('boundaries: 19.9 fallback, 25 degrade, 30 ok', () => {
    expect(classifyFps([19.9, 19.9, 19.9])).toBe('fallback')
    expect(classifyFps([20, 20, 20])).toBe('degrade')
    expect(classifyFps([25, 25, 25])).toBe('degrade')
    expect(classifyFps([29.9])).toBe('degrade')
    expect(classifyFps([30, 30, 30])).toBe('ok')
    expect(classifyFps([60, 60])).toBe('ok')
  })

  it('uses the median, so one hitch does not fall back', () => {
    expect(classifyFps([60, 60, 5, 60, 60])).toBe('ok')
    expect(classifyFps([10, 12, 15, 60])).toBe('fallback')
    expect(classifyFps([])).toBe('ok')
  })
})

describe('node cap', () => {
  it('3000 is allowed, 3001 is too many', () => {
    expect(MAX_3D_NODES).toBe(3000)
    expect(tooManyFor3d(3000)).toBe(false)
    expect(tooManyFor3d(3001)).toBe(true)
  })
})

describe('fallback copy (§3.8, word for word)', () => {
  it('matches the PRD table', () => {
    expect(fallbackToastCopy('noWebGL2')).toBe("3D needs WebGL 2, which this window doesn't have. Showing 2D.")
    expect(fallbackToastCopy('software')).toBe("3D needs graphics acceleration, which this machine isn't using. Showing 2D.")
    expect(fallbackToastCopy('tooMany', 3214)).toBe('Too many notes for 3D (3,214). Showing 2D.')
    expect(fallbackToastCopy('slow')).toBe('3D was running slowly here. Showing 2D.')
    expect(fallbackToastCopy('loadFailed')).toBe("Couldn't load the 3D view. Showing 2D.")
    expect(fallbackToastCopy('crashed')).toBe('3D view stopped. Showing 2D.')
    expect(NO_WEBGL2_TITLE).toBe("3D needs WebGL 2, which this window doesn't have.")
  })
})
