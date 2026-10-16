// @vitest-environment jsdom
//
// prd-wiki-3d-view-v1 §6.3, §8.1 (W8, W10, W11): WikiGraph3D over a mocked
// react-force-graph-3d. Dispose hands the GL context back exactly once and
// clears window.scene; the render loop pauses after the layout settles and
// resumes on input; clicks / wikilinks fly the camera; search frames the
// matches; slow frames and a lost context fall back.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

type Listener = (e?: unknown) => void

const h = vi.hoisted(() => ({
  props: null as null | Record<string, unknown>,
  fg: null as null | Record<string, ReturnType<typeof vi.fn> | unknown>,
}))

vi.mock('react-force-graph-3d', async () => {
  const React = await import('react')
  const make = () => {
    const canvas = document.createElement('canvas')
    const controlsListeners = new Map<string, Set<Listener>>()
    const controls = {
      target: { x: 0, y: 0, z: 0 },
      enableDamping: false,
      dampingFactor: 0,
      screenSpacePanning: false,
      zoomToCursor: false,
      listenToKeyEvents: vi.fn(),
      addEventListener: vi.fn((t: string, fn: Listener) => {
        if (!controlsListeners.has(t)) controlsListeners.set(t, new Set())
        controlsListeners.get(t)!.add(fn)
      }),
      removeEventListener: vi.fn((t: string, fn: Listener) => controlsListeners.get(t)?.delete(fn)),
      listeners: controlsListeners,
    }
    const threeScene = { add: vi.fn(), remove: vi.fn() }
    ;(window as unknown as { scene?: unknown }).scene = threeScene
    return {
      canvas,
      controls,
      threeScene,
      renderer: vi.fn(() => ({ domElement: canvas, forceContextLoss: rendererLoss })),
      scene: vi.fn(() => threeScene),
      camera: vi.fn(() => ({ position: { x: 0, y: 0, z: 300 } })),
      controlsFn: vi.fn(() => controls),
      pauseAnimation: vi.fn(),
      resumeAnimation: vi.fn(),
      zoomToFit: vi.fn(),
      cameraPosition: vi.fn(),
    }
  }
  const rendererLoss = vi.fn()
  const Stub = React.forwardRef((props: Record<string, unknown>, ref) => {
    const inst = React.useMemo(make, [])
    h.props = props
    h.fg = { ...inst, forceContextLoss: rendererLoss }
    React.useImperativeHandle(ref, () => ({
      renderer: inst.renderer,
      scene: inst.scene,
      camera: inst.camera,
      controls: inst.controlsFn,
      pauseAnimation: inst.pauseAnimation,
      resumeAnimation: inst.resumeAnimation,
      zoomToFit: inst.zoomToFit,
      cameraPosition: inst.cameraPosition,
    }))
    return React.createElement('div', { 'data-stub': 'fg3d' })
  })
  return { default: Stub }
})

import { act, cleanup, render } from '@testing-library/react'
import WikiGraph3D, { CANVAS_LABEL, FLY_MS, HINT_TEXT, IDLE_PAUSE_MS, type WikiGraph3DProps } from './WikiGraph3D'
import type { WikiIndex } from './wiki-api'

const n = (id: string) => ({ id, title: id, aliases: [], tags: [], path: id, exists: true })
const INDEX: WikiIndex = {
  workspacePath: '/w/alpha',
  generatedAt: '',
  nodes: [n('Home.md'), n('A.md'), n('B.md')],
  links: [
    { source: 'Home.md', target: 'A.md' },
    { source: 'A.md', target: 'B.md' },
  ],
  noteCount: 3,
  scope: 'workspace',
}

function props(over: Partial<WikiGraph3DProps> = {}): WikiGraph3DProps {
  return {
    index: INDEX,
    selectedId: 'Home.md',
    search: '',
    mode: 'global',
    onSelect: vi.fn(),
    reducedMotion: false,
    focusRequest: null,
    onFallback: vi.fn(),
    ...over,
  }
}

function fg() {
  if (!h.fg) throw new Error('force graph stub not mounted')
  return h.fg as unknown as {
    canvas: HTMLCanvasElement
    controls: { enableDamping: boolean; dampingFactor: number; screenSpacePanning: boolean; zoomToCursor: boolean; listenToKeyEvents: ReturnType<typeof vi.fn>; listeners: Map<string, Set<Listener>> }
    threeScene: unknown
    forceContextLoss: ReturnType<typeof vi.fn>
    pauseAnimation: ReturnType<typeof vi.fn>
    resumeAnimation: ReturnType<typeof vi.fn>
    zoomToFit: ReturnType<typeof vi.fn>
    cameraPosition: ReturnType<typeof vi.fn>
  }
}

function call(name: string, ...args: unknown[]): void {
  const fn = h.props?.[name]
  if (typeof fn !== 'function') throw new Error(`no ${name} prop`)
  act(() => {
    ;(fn as (...a: unknown[]) => void)(...args)
  })
}

/** jsdom has no 2D canvas; label sprites and the halo draw into this no-op one. */
function fake2d(): CanvasRenderingContext2D {
  const target: Record<string, unknown> = {
    measureText: () => ({ width: 10 }),
    createRadialGradient: () => ({ addColorStop: () => undefined }),
  }
  return new Proxy(target, {
    get: (t, k: string) => (k in t ? t[k] : () => undefined),
    set: (t, k: string, v) => {
      t[k] = v
      return true
    },
  }) as unknown as CanvasRenderingContext2D
}

beforeEach(() => {
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockImplementation(
    ((kind: string) => (kind === '2d' ? fake2d() : null)) as unknown as HTMLCanvasElement['getContext'],
  )
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'Date'] })
  h.props = null
  h.fg = null
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

describe('WikiGraph3D setup', () => {
  it('passes the PRD props and sets up CAD-parity controls and the canvas', () => {
    render(<WikiGraph3D {...props()} />)
    const p = h.props!
    expect([p.controlType, p.showNavInfo, p.enableNodeDrag, p.cooldownTicks, p.cooldownTime, p.warmupTicks, p.linkWidth, p.linkOpacity]).toEqual([
      'orbit', false, true, 40, 2000, 0, 0, 1,
    ])
    expect(p.rendererConfig).toEqual({ antialias: true, alpha: true, powerPreference: 'default' })
    expect((p.nodeLabel as () => string)()).toBe('')
    const c = fg().controls
    expect([c.enableDamping, c.dampingFactor, c.screenSpacePanning, c.zoomToCursor]).toEqual([true, 0.12, true, true])
    expect(c.listenToKeyEvents).toHaveBeenCalledWith(fg().canvas)
    expect(fg().canvas.tabIndex).toBe(0)
    expect(fg().canvas.getAttribute('aria-label')).toBe(CANVAS_LABEL)
    expect(document.querySelector('[data-wiki-3d-hint]')?.textContent).toBe(HINT_TEXT)
    // Same graph data as 2D, with z.
    const data = p.graphData as { nodes: { id: string; z?: number }[] }
    expect(data.nodes.map((x) => x.id)).toEqual(['A.md', 'B.md', 'Home.md'])
    expect(data.nodes.every((x) => typeof x.z === 'number')).toBe(true)
  })

  it('reduced motion settles before the first frame and moves the camera instantly', () => {
    render(<WikiGraph3D {...props({ reducedMotion: true })} />)
    expect([h.props!.warmupTicks, h.props!.cooldownTicks]).toEqual([80, 0])
    const node = (h.props!.graphData as { nodes: { id: string }[] }).nodes.find((x) => x.id === 'A.md')
    call('onNodeClick', node)
    expect(fg().cameraPosition).toHaveBeenCalledTimes(1)
    expect(fg().cameraPosition.mock.calls[0][2]).toBe(0)
  })
})

describe('WikiGraph3D dispose (W10)', () => {
  it('unmount hands the GL context back exactly once, clears window.scene and our listeners', () => {
    const { unmount } = render(<WikiGraph3D {...props()} />)
    const f = fg()
    expect((window as unknown as { scene?: unknown }).scene).toBe(f.threeScene)
    const removeSpy = vi.spyOn(f.canvas, 'removeEventListener')
    f.forceContextLoss.mockClear() // the spy is shared across mounts
    expect(f.forceContextLoss).not.toHaveBeenCalled()
    unmount()
    expect(f.forceContextLoss).toHaveBeenCalledTimes(1)
    expect(f.pauseAnimation).toHaveBeenCalled()
    expect('scene' in window).toBe(false)
    const removed = removeSpy.mock.calls.map((c) => c[0])
    for (const t of ['gesturestart', 'dblclick', 'webglcontextlost', 'pointermove', 'pointerdown', 'wheel', 'keydown']) {
      expect(removed).toContain(t)
    }
    expect(f.controls.listeners.get('start')?.size ?? 0).toBe(0)
    expect(f.controls.listeners.get('change')?.size ?? 0).toBe(0)
  })

  it('a context lost after unmount is not reported as a crash', () => {
    const onFallback = vi.fn()
    const { unmount } = render(<WikiGraph3D {...props({ onFallback })} />)
    const canvas = fg().canvas
    unmount()
    canvas.dispatchEvent(new Event('webglcontextlost'))
    expect(onFallback).not.toHaveBeenCalled()
  })
})

describe('WikiGraph3D render on demand (W11)', () => {
  it('pauses after the engine stops plus 1 s idle, resumes on pointer input', () => {
    render(<WikiGraph3D {...props()} />)
    const f = fg()
    call('onEngineStop')
    act(() => {
      vi.advanceTimersByTime(IDLE_PAUSE_MS - 10)
    })
    expect(f.pauseAnimation).not.toHaveBeenCalled()
    act(() => {
      vi.advanceTimersByTime(20)
    })
    expect(f.pauseAnimation).toHaveBeenCalledTimes(1)
    act(() => {
      f.canvas.dispatchEvent(new Event('pointermove'))
    })
    expect(f.resumeAnimation).toHaveBeenCalledTimes(1)
    // …and pauses again once idle.
    act(() => {
      vi.advanceTimersByTime(IDLE_PAUSE_MS + 10)
    })
    expect(f.pauseAnimation).toHaveBeenCalledTimes(2)
  })

  it('does not pause while the layout is still running', () => {
    render(<WikiGraph3D {...props()} />)
    call('onEngineTick')
    act(() => {
      vi.advanceTimersByTime(5000)
    })
    expect(fg().pauseAnimation).not.toHaveBeenCalled()
  })

  it('pauses while the document is hidden', () => {
    render(<WikiGraph3D {...props()} />)
    vi.spyOn(document, 'hidden', 'get').mockReturnValue(true)
    act(() => {
      document.dispatchEvent(new Event('visibilitychange'))
    })
    expect(fg().pauseAnimation).toHaveBeenCalledTimes(1)
  })
})

describe('WikiGraph3D camera', () => {
  it('first engine stop fits the graph once; polls do not move the camera', () => {
    const { rerender } = render(<WikiGraph3D {...props()} />)
    call('onEngineStop')
    expect(fg().zoomToFit).toHaveBeenCalledTimes(1)
    expect(fg().zoomToFit.mock.calls[0].slice(0, 2)).toEqual([400, 40])
    // Same index object (a poll with no fingerprint change) → nothing.
    rerender(<WikiGraph3D {...props()} />)
    call('onEngineStop')
    expect(fg().zoomToFit).toHaveBeenCalledTimes(1)
    expect(fg().cameraPosition).not.toHaveBeenCalled()
  })

  it('a node click selects it and flies there over 800 ms', () => {
    const onSelect = vi.fn()
    render(<WikiGraph3D {...props({ onSelect })} />)
    const node = (h.props!.graphData as { nodes: { id: string; x: number; y: number; z: number }[] }).nodes.find((x) => x.id === 'B.md')!
    call('onNodeClick', node)
    expect(onSelect).toHaveBeenCalledWith('B.md')
    const [pos, lookAt, ms] = fg().cameraPosition.mock.calls[0]
    expect(lookAt).toEqual({ x: node.x, y: node.y, z: node.z })
    expect(ms).toBe(FLY_MS)
    expect(typeof pos.z).toBe('number')
  })

  it('a wikilink focus request flies once per seq; selection alone does not fly', () => {
    const { rerender } = render(<WikiGraph3D {...props()} />)
    rerender(<WikiGraph3D {...props({ selectedId: 'A.md' })} />)
    expect(fg().cameraPosition).not.toHaveBeenCalled()
    rerender(<WikiGraph3D {...props({ selectedId: 'B.md', focusRequest: { id: 'B.md', seq: 1 } })} />)
    expect(fg().cameraPosition).toHaveBeenCalledTimes(1)
    rerender(<WikiGraph3D {...props({ selectedId: 'B.md', focusRequest: { id: 'B.md', seq: 1 } })} />)
    expect(fg().cameraPosition).toHaveBeenCalledTimes(1)
  })

  it('search frames the matches ~300 ms after typing stops', () => {
    const { rerender } = render(<WikiGraph3D {...props()} />)
    call('onEngineStop')
    fg().zoomToFit.mockClear()
    rerender(<WikiGraph3D {...props({ search: 'B' })} />)
    call('onEngineStop') // the re-heated layout settles before the debounce
    act(() => {
      vi.advanceTimersByTime(299)
    })
    expect(fg().zoomToFit).not.toHaveBeenCalled()
    act(() => {
      vi.advanceTimersByTime(2)
    })
    expect(fg().zoomToFit).toHaveBeenCalledTimes(1)
    const [ms, pad, filter] = fg().zoomToFit.mock.calls[0]
    expect([ms, pad]).toEqual([600, 40])
    expect((filter as (x: { id: string }) => boolean)({ id: 'B.md' })).toBe(true)
    expect((filter as (x: { id: string }) => boolean)({ id: 'A.md' })).toBe(false)
  })

  it('double-click on the background resets the view', () => {
    render(<WikiGraph3D {...props()} />)
    act(() => {
      fg().canvas.dispatchEvent(new Event('dblclick'))
    })
    expect(fg().zoomToFit).toHaveBeenCalledTimes(1)
  })
})

describe('WikiGraph3D fallbacks', () => {
  it('a lost WebGL context reports crashed', () => {
    const onFallback = vi.fn()
    render(<WikiGraph3D {...props({ onFallback })} />)
    act(() => {
      fg().canvas.dispatchEvent(new Event('webglcontextlost'))
    })
    expect(onFallback).toHaveBeenCalledWith('crashed')
  })

  it('slow frames for 3 s report slow (F4)', () => {
    const frames: FrameRequestCallback[] = []
    vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => {
      frames.push(cb)
      return frames.length
    })
    vi.spyOn(performance, 'now').mockReturnValue(0)
    const onFallback = vi.fn()
    render(<WikiGraph3D {...props({ onFallback })} />)
    call('onEngineTick')
    let t = 0
    for (let i = 0; i < 100 && frames.length; i++) {
      const cb = frames.shift()!
      t += 100 // 10 fps
      act(() => cb(t))
    }
    expect(onFallback).toHaveBeenCalledWith('slow')
  })
})
