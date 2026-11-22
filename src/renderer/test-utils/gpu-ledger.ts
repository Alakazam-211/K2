// A ledger of what canvases would hold in WebKit's GPU process, for leak
// tests under jsdom (which has no canvas). `installGpuLedger()` replaces
// `HTMLCanvasElement.prototype.getContext` with fakes that count:
//
//   - WebGL2 contexts: live (not lost) ones, their drawing buffers
//     (width × height × 4, at least 1×1 as in WebGL) and every GL object
//     made and not deleted (programs, shaders, buffers, VAOs, textures),
//     plus texture bytes from texImage2D. Losing a context frees all of it,
//     as the GPU process does.
//   - 2D contexts: the backing store of every canvas that got one and still
//     has a size. A canvas nobody references is freed by WebKit only at GC,
//     which a test can't see, so the ledger counts it as held until the code
//     frees it itself (width/height 0). That is the point: GPU memory must
//     not wait for the collector.
//
// The fakes behave like the real API where the code under test depends on
// it: getContext returns the same context for the same canvas, a lost
// context makes nothing (createShader → null), readPixels returns the clear
// colour (the painter's sanity probe), WEBGL_lose_context works.

import { vi } from 'vitest'

type GlObject = { kind: string; ctx: FakeGl }

interface FakeGl {
  canvas: HTMLCanvasElement
  lost: boolean
  objects: Set<GlObject>
  textureBytes: Map<GlObject, number>
}

interface Fake2d {
  canvas: HTMLCanvasElement
}

export interface GpuLedger {
  /** WebGL contexts made so far (one per canvas). */
  readonly glContextsMade: number
  /** WebGL contexts not lost. */
  liveGlContexts(): number
  /** Drawing-buffer bytes of the live WebGL contexts. */
  drawingBufferBytes(): number
  /** GL objects made and not deleted, in live contexts. */
  liveGlObjects(): number
  /** Texture bytes uploaded and not freed, in live contexts. */
  textureBytes(): number
  /** Backing-store bytes of 2D canvases that still have a size. */
  canvas2dBytes(): number
  /** Lose a canvas's context as WebKit does under memory pressure, and
   *  (optionally) restore it: dispatches the real events. */
  loseContext(canvas: HTMLCanvasElement): void
  restoreContext(canvas: HTMLCanvasElement): void
  /** One-line summary for failure messages. */
  summary(): string
  restore(): void
}

const GL_CONSTANTS: Record<string, number> = {
  NO_ERROR: 0,
  VERTEX_SHADER: 0x8b31,
  FRAGMENT_SHADER: 0x8b30,
  COMPILE_STATUS: 0x8b81,
  LINK_STATUS: 0x8b82,
  ARRAY_BUFFER: 0x8892,
  STATIC_DRAW: 0x88e4,
  STREAM_DRAW: 0x88e0,
  FLOAT: 0x1406,
  TEXTURE_2D: 0x0de1,
  TEXTURE_MIN_FILTER: 0x2801,
  TEXTURE_MAG_FILTER: 0x2800,
  NEAREST: 0x2600,
  TEXTURE_WRAP_S: 0x2802,
  TEXTURE_WRAP_T: 0x2803,
  CLAMP_TO_EDGE: 0x812f,
  BLEND: 0x0be2,
  SRC_ALPHA: 0x0302,
  ONE_MINUS_SRC_ALPHA: 0x0303,
  COLOR_BUFFER_BIT: 0x4000,
  TRIANGLE_STRIP: 0x0005,
  RGBA: 0x1908,
  UNSIGNED_BYTE: 0x1401,
  TEXTURE0: 0x84c0,
}

function makeGl(canvas: HTMLCanvasElement): { gl: Record<string, unknown>; state: FakeGl } {
  const state: FakeGl = { canvas, lost: false, objects: new Set(), textureBytes: new Map() }
  let clear: [number, number, number, number] = [0, 0, 0, 1]
  let boundTexture: GlObject | null = null
  const make = (kind: string) => (): GlObject | null => {
    if (state.lost) return null
    const o = { kind, ctx: state }
    state.objects.add(o)
    return o
  }
  const del = (o: GlObject | null): void => {
    if (!o) return
    state.objects.delete(o)
    state.textureBytes.delete(o)
  }
  const noop = (): void => {}
  const loseExt = {
    loseContext: () => {
      state.lost = true
      state.objects.clear()
      state.textureBytes.clear()
    },
    restoreContext: () => {
      state.lost = false
    },
  }
  const gl: Record<string, unknown> = {
    ...GL_CONSTANTS,
    canvas,
    isContextLost: () => state.lost,
    getExtension: (name: string) => (name === 'WEBGL_lose_context' ? loseExt : null),
    createShader: make('shader'),
    createProgram: make('program'),
    createBuffer: make('buffer'),
    createVertexArray: make('vao'),
    createTexture: make('texture'),
    deleteShader: del,
    deleteProgram: del,
    deleteBuffer: del,
    deleteVertexArray: del,
    deleteTexture: (o: GlObject | null) => {
      if (boundTexture === o) boundTexture = null
      del(o)
    },
    shaderSource: noop,
    compileShader: noop,
    getShaderParameter: () => !state.lost,
    getShaderInfoLog: () => '',
    attachShader: noop,
    linkProgram: noop,
    getProgramParameter: () => !state.lost,
    getProgramInfoLog: () => '',
    getUniformLocation: () => ({}),
    bindBuffer: noop,
    bufferData: noop,
    bindVertexArray: noop,
    enableVertexAttribArray: noop,
    vertexAttribPointer: noop,
    vertexAttribDivisor: noop,
    bindTexture: (_t: number, o: GlObject | null) => {
      boundTexture = o
    },
    texParameteri: noop,
    texImage2D: (...args: unknown[]) => {
      if (state.lost || !boundTexture) return
      const src = args[args.length - 1] as { width?: number; height?: number }
      state.textureBytes.set(boundTexture, (src.width ?? 0) * (src.height ?? 0) * 4)
    },
    activeTexture: noop,
    enable: noop,
    blendFunc: noop,
    useProgram: noop,
    uniform1i: noop,
    uniform1f: noop,
    uniform2f: noop,
    viewport: noop,
    clearColor: (r: number, g: number, b: number, a: number) => {
      clear = [r, g, b, a]
    },
    clear: noop,
    drawArraysInstanced: noop,
    readPixels: (_x: number, _y: number, _w: number, _h: number, _f: number, _t: number, out: Uint8Array) => {
      for (let i = 0; i < 4; i++) out[i] = Math.round(clear[i] * 255)
    },
    getError: () => 0,
  }
  return { gl, state }
}

function make2d(canvas: HTMLCanvasElement): Record<string, unknown> {
  const noop = (): void => {}
  return {
    canvas,
    font: '',
    fillStyle: '',
    strokeStyle: '',
    lineWidth: 1,
    globalAlpha: 1,
    textBaseline: 'alphabetic',
    measureText: (text: string) => ({
      width: 8 * [...text].length,
      fontBoundingBoxAscent: 12,
      fontBoundingBoxDescent: 4,
    }),
    getImageData: (_x: number, _y: number, w: number, h: number) => {
      const data = new Uint8ClampedArray(Math.max(0, w * h * 4))
      data.fill(255)
      return { data, width: w, height: h }
    },
    fillText: noop,
    fillRect: noop,
    clearRect: noop,
    drawImage: noop,
    save: noop,
    restore: noop,
    beginPath: noop,
    closePath: noop,
    rect: noop,
    clip: noop,
    moveTo: noop,
    lineTo: noop,
    arc: noop,
    arcTo: noop,
    fill: noop,
    stroke: noop,
  }
}

export function installGpuLedger(): GpuLedger {
  const gls = new Map<HTMLCanvasElement, { gl: Record<string, unknown>; state: FakeGl }>()
  const twoDs = new Map<HTMLCanvasElement, Fake2d & { ctx: Record<string, unknown> }>()
  const spy = vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockImplementation(function (
    this: HTMLCanvasElement,
    kind: string,
  ) {
    if (kind === 'webgl2') {
      if (twoDs.has(this)) return null
      let entry = gls.get(this)
      if (!entry) {
        entry = makeGl(this)
        gls.set(this, entry)
      }
      return entry.gl
    }
    if (kind === '2d') {
      if (gls.has(this)) return null
      let entry = twoDs.get(this)
      if (!entry) {
        entry = { canvas: this, ctx: make2d(this) }
        twoDs.set(this, entry)
      }
      return entry.ctx
    }
    return null
  } as unknown as HTMLCanvasElement['getContext'])

  const live = () => [...gls.values()].filter((e) => !e.state.lost)
  const ledger: GpuLedger = {
    get glContextsMade() {
      return gls.size
    },
    liveGlContexts: () => live().length,
    drawingBufferBytes: () =>
      live().reduce((n, e) => n + Math.max(1, e.state.canvas.width) * Math.max(1, e.state.canvas.height) * 4, 0),
    liveGlObjects: () => live().reduce((n, e) => n + e.state.objects.size, 0),
    textureBytes: () =>
      live().reduce((n, e) => n + [...e.state.textureBytes.values()].reduce((a, b) => a + b, 0), 0),
    canvas2dBytes: () => [...twoDs.keys()].reduce((n, c) => n + c.width * c.height * 4, 0),
    loseContext(canvas) {
      const e = gls.get(canvas)
      if (!e) throw new Error('gpu-ledger: that canvas has no WebGL context')
      const ev = new Event('webglcontextlost', { cancelable: true })
      ;(e.gl.getExtension as (n: string) => { loseContext(): void })('WEBGL_lose_context').loseContext()
      canvas.dispatchEvent(ev)
    },
    restoreContext(canvas) {
      const e = gls.get(canvas)
      if (!e) throw new Error('gpu-ledger: that canvas has no WebGL context')
      e.state.lost = false
      canvas.dispatchEvent(new Event('webglcontextrestored'))
    },
    summary: () =>
      `glContexts live=${ledger.liveGlContexts()}/${gls.size} drawingBuffers=${ledger.drawingBufferBytes()}B ` +
      `glObjects=${ledger.liveGlObjects()} textures=${ledger.textureBytes()}B canvas2d=${ledger.canvas2dBytes()}B`,
    restore: () => spy.mockRestore(),
  }
  return ledger
}
