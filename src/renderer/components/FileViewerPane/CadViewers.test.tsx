// @vitest-environment jsdom
//
// CAD previews (ported from the K2 Companion's CadViews.test.tsx):
// DXF → SVG, the bytes fallback, the 3D viewer's wiring, and that each
// extension reaches the right viewer inside FileViewerPane. jsdom has no
// WebGL, so `./modelScene` is a stand-in here; modelParse.test.ts runs the
// real three.js loaders.

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, screen, waitFor } from '@testing-library/react'

const daemonCliGet = vi.hoisted(() => vi.fn())
vi.mock('@/lib/daemon-cli', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/lib/daemon-cli')>()
  return {
    ...actual,
    daemonCliGet: (...args: unknown[]) => daemonCliGet(...args),
    daemonCliPost: vi.fn(async () => ({})),
  }
})
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

const scene = vi.hoisted(() => {
  const dispose = vi.fn()
  const reset = vi.fn()
  return {
    dispose,
    reset,
    parseModel: vi.fn(async (_bytes: Uint8Array, _ext: string) => ({
      root: { isStub: true },
      stats: { triangles: 288, meshes: 1, size: [120, 60, 6] as [number, number, number] },
    })),
    hasWebGL2: vi.fn(() => true),
    createViewer: vi.fn((_host: HTMLElement, _root: unknown) => ({ dispose, reset })),
    disposeObject: vi.fn(),
  }
})
vi.mock('./modelScene', () => ({
  parseModel: scene.parseModel,
  hasWebGL2: scene.hasWebGL2,
  createViewer: scene.createViewer,
  disposeObject: scene.disposeObject,
}))

import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { DxfHostBytesViewer, DxfViewer, decodeDxfBytes } from './DxfViewer'
import { ModelViewer, MODEL_MAX_BYTES, modelSizeError } from './ModelViewer'
import { FileViewerPane } from './FileViewerPane'
import { DXF_FIXTURE } from './dxfFixture'
import { getFileCategory, isBinaryPreviewCategory, MODEL3D_EXTS } from './fileCategory'

const b64 = (bytes: Uint8Array): string => {
  let s = ''
  for (const b of bytes) s += String.fromCharCode(b)
  return btoa(s)
}
const BRACKET_DXF = readFileSync(resolve(__dirname, '__fixtures__/k2-bracket.dxf'), 'utf8')

/** Calls to `daemonCliGet` for one route. */
const callsTo = (route: string): unknown[][] => daemonCliGet.mock.calls.filter((c) => c[1] === route)

beforeEach(() => {
  daemonCliGet.mockReset()
  daemonCliGet.mockImplementation(async (_scope: unknown, route: string) => {
    if (route === 'fs/read-binary') return { base64: btoa('solid x\nendsolid x\n') }
    throw new Error(`unexpected route ${route}`)
  })
  scene.parseModel.mockClear()
  scene.createViewer.mockClear()
  scene.dispose.mockClear()
  scene.reset.mockClear()
  scene.disposeObject.mockClear()
  scene.hasWebGL2.mockReturnValue(true)
})
afterEach(() => cleanup())

describe('DxfViewer', () => {
  it('draws the fixture as SVG lines, circles, arc paths and text', async () => {
    renderInPrimaryRoom(<DxfViewer text={DXF_FIXTURE} />)
    const svg = await screen.findByTestId('dxf-svg')
    expect(svg.querySelectorAll('line')).toHaveLength(1)
    expect(svg.querySelectorAll('circle')).toHaveLength(2)
    expect(svg.querySelectorAll('path')).toHaveLength(3)
    expect(svg.querySelectorAll('text')).toHaveLength(2)
    expect(svg.querySelector('path')!.getAttribute('d')).toContain(' A20 20 0 0 1 ')
    // Hairlines at any zoom: vector-effect is set per shape.
    expect(svg.querySelector('line')!.getAttribute('vector-effect')).toBe('non-scaling-stroke')
    // Y flipped once at the root; text flips itself back.
    expect(screen.getByTestId('dxf-shapes').getAttribute('transform')).toBe('scale(1 -1)')
    expect(svg.querySelector('text')!.getAttribute('transform')).toContain('scale(1 -1)')
    expect(svg.getAttribute('viewBox')).toMatch(/^-?[\d.]+ -?[\d.]+ [\d.]+ [\d.]+$/)
  })

  it('notes the entities it could not draw, and shows the stats line', async () => {
    renderInPrimaryRoom(<DxfViewer text={DXF_FIXTURE} />)
    const note = await screen.findByTestId('dxf-skipped')
    expect(note.textContent).toBe('2 entities not shown (POINT, HATCH)')
    expect(screen.getByTestId('dxf-stats').textContent).toMatch(/^\d+ entities · [\d.]+ × [\d.]+ units/)
  })

  it('the Companion bracket sample: 120 × 60 units, nothing skipped', async () => {
    renderInPrimaryRoom(<DxfViewer text={BRACKET_DXF} />)
    const stats = await screen.findByTestId('dxf-stats')
    expect(stats.textContent).toMatch(/^17 entities · 1\d\d × \d\d(\.\d+)? units/)
    expect(screen.queryByTestId('dxf-skipped')).toBeNull()
  })

  it('wheel zooms around the cursor; Fit and double-click restore the fitted view', async () => {
    renderInPrimaryRoom(<DxfViewer text={DXF_FIXTURE} />)
    const svg = await screen.findByTestId('dxf-svg')
    const fitted = svg.getAttribute('viewBox')!
    const box = svg.parentElement!
    fireEvent.wheel(box, { deltaY: -200, clientX: 0, clientY: 0 })
    const zoomed = svg.getAttribute('viewBox')!
    expect(zoomed).not.toBe(fitted)
    const [, , wFit] = fitted.split(' ').map(Number)
    const [, , wZoom] = zoomed.split(' ').map(Number)
    expect(wZoom).toBeLessThan(wFit)
    fireEvent.click(screen.getByTestId('dxf-fit'))
    expect(svg.getAttribute('viewBox')).toBe(fitted)
    fireEvent.wheel(box, { deltaY: -200, clientX: 0, clientY: 0 })
    fireEvent.dblClick(box)
    expect(svg.getAttribute('viewBox')).toBe(fitted)
  })

  it('drag pans the view', async () => {
    renderInPrimaryRoom(<DxfViewer text={DXF_FIXTURE} />)
    const svg = await screen.findByTestId('dxf-svg')
    const fitted = svg.getAttribute('viewBox')!.split(' ').map(Number)
    const box = svg.parentElement!
    fireEvent.pointerDown(box, { pointerId: 1, clientX: 100, clientY: 100 })
    fireEvent.pointerMove(box, { pointerId: 1, clientX: 140, clientY: 100 })
    fireEvent.pointerUp(box, { pointerId: 1, clientX: 140, clientY: 100 })
    const panned = svg.getAttribute('viewBox')!.split(' ').map(Number)
    // Dragging right moves the view left; the size is unchanged.
    expect(panned[0]).toBeLessThan(fitted[0])
    expect(panned[2]).toBe(fitted[2])
  })

  it('binary DXF says so', async () => {
    renderInPrimaryRoom(<DxfViewer text={'AutoCAD Binary DXF\r\n\u001a\u0000'} />)
    expect((await screen.findByTestId('dxf-error')).textContent).toContain('Binary DXF is not supported')
  })
})

describe('DxfHostBytesViewer (read-file refused the DXF)', () => {
  it('decodes a Windows-1252 DXF and draws it', async () => {
    // "°" as 0xB0: not UTF-8, so read-file calls it binary.
    const latin = Uint8Array.from(
      [...BRACKET_DXF.replace('K2', 'K2 90°')].map((ch) => ch.charCodeAt(0) & 0xff),
    )
    daemonCliGet.mockImplementation(async (_s: unknown, route: string) => {
      if (route === 'fs/read-binary') return { base64: b64(latin) }
      throw new Error(`unexpected route ${route}`)
    })
    renderInPrimaryRoom(<DxfHostBytesViewer filePath="/ws/old.dxf" />)
    const svg = await screen.findByTestId('dxf-svg')
    expect(svg.querySelectorAll('circle')).toHaveLength(2)
    expect(callsTo('fs/read-binary')[0][2]).toEqual({ path: '/ws/old.dxf' })
  })

  it('a real binary DXF still says so', async () => {
    const bin = Uint8Array.from([...'AutoCAD Binary DXF\r\n\u001a\u0000'].map((c) => c.charCodeAt(0)))
    daemonCliGet.mockImplementation(async () => ({ base64: b64(bin) }))
    renderInPrimaryRoom(<DxfHostBytesViewer filePath="/ws/b.dxf" />)
    expect((await screen.findByTestId('dxf-error')).textContent).toContain('Binary DXF is not supported')
  })

  it('decodeDxfBytes: UTF-8 first, Windows-1252 otherwise', () => {
    expect(decodeDxfBytes(new TextEncoder().encode('0\nEOF\n°'))).toBe('0\nEOF\n°')
    expect(decodeDxfBytes(Uint8Array.from([0x30, 0x0a, 0xb0]))).toBe('0\n°')
  })
})

describe('ModelViewer', () => {
  it('reads bytes from the room daemon, lazy-loads the scene and shows stats', async () => {
    renderInPrimaryRoom(<ModelViewer filePath="/ws/k2-bracket.stl" />)
    const stats = await screen.findByTestId('model-stats')
    expect(stats.textContent).toBe('288 triangles · 120 × 60 × 6 units')
    expect(callsTo('fs/read-binary')[0][2]).toEqual({ path: '/ws/k2-bracket.stl' })
    expect(scene.parseModel).toHaveBeenCalledWith(expect.any(Uint8Array), '.stl')
    await waitFor(() => expect(scene.createViewer).toHaveBeenCalledTimes(1))
    expect(scene.createViewer.mock.calls[0][0]).toBe(screen.getByTestId('model-host'))
    fireEvent.click(await screen.findByTestId('model-reset'))
    expect(scene.reset).toHaveBeenCalledTimes(1)
  })

  it('disposes the renderer on unmount', async () => {
    const view = renderInPrimaryRoom(<ModelViewer filePath="/ws/k2-cube.obj" />)
    await waitFor(() => expect(scene.createViewer).toHaveBeenCalled())
    expect(scene.dispose).not.toHaveBeenCalled()
    view.unmount()
    expect(scene.dispose).toHaveBeenCalledTimes(1)
  })

  it('no WebGL: says so and keeps the stats', async () => {
    scene.hasWebGL2.mockReturnValue(false)
    renderInPrimaryRoom(<ModelViewer filePath="/ws/k2-bracket.3mf" />)
    expect((await screen.findByTestId('model-nogl')).textContent).toContain('3D preview needs WebGL')
    expect(screen.getByTestId('model-stats').textContent).toContain('288 triangles')
    expect(scene.createViewer).not.toHaveBeenCalled()
    expect(scene.disposeObject).toHaveBeenCalled()
  })

  it('a read failure that is not about size shows the daemon message', async () => {
    daemonCliGet.mockRejectedValue(new Error('Permission denied'))
    renderInPrimaryRoom(<ModelViewer filePath="/ws/locked.stl" />)
    const err = await screen.findByTestId('model-error')
    expect(err.textContent).toContain('Could not load the model')
    expect(err.textContent).toContain('Permission denied')
    expect(scene.parseModel).not.toHaveBeenCalled()
  })

  it('size guard', () => {
    expect(modelSizeError(MODEL_MAX_BYTES)).toBeNull()
    expect(modelSizeError(MODEL_MAX_BYTES + 1)).toMatch(/over the 200\.0 MB preview limit/)
  })

  it('a parse error shows the message', async () => {
    scene.parseModel.mockRejectedValueOnce(new Error('No triangles in this STL.'))
    renderInPrimaryRoom(<ModelViewer filePath="/ws/empty.stl" />)
    expect((await screen.findByTestId('model-error')).textContent).toContain('No triangles in this STL.')
  })
})

describe('FileViewerPane routes each CAD extension to its viewer', () => {
  const pane = (filePath: string) => (
    <FileViewerPane filePath={filePath} paneId="p1" paneGroupId="g1" tabId="t1" />
  )

  it('every 3D extension is a byte preview', () => {
    for (const ext of MODEL3D_EXTS) {
      expect(getFileCategory(`/ws/part${ext}`)).toBe('model3d')
      expect(isBinaryPreviewCategory('model3d')).toBe(true)
    }
  })

  it('.dxf reads text through fs/read-file and draws SVG; Source shows the text', async () => {
    daemonCliGet.mockImplementation(async (_s: unknown, route: string) => {
      if (route === 'fs/read-file') return { content: BRACKET_DXF }
      throw new Error(`unexpected route ${route}`)
    })
    renderInPrimaryRoom(pane('/ws/k2-bracket.dxf'))
    await screen.findByTestId('dxf-svg')
    expect(callsTo('fs/read-file')[0][2]).toEqual({ path: '/ws/k2-bracket.dxf' })
    expect(callsTo('fs/read-binary')).toHaveLength(0)
    fireEvent.click(screen.getByRole('button', { name: 'Source' }))
    await waitFor(() => expect(screen.queryByTestId('dxf-svg')).toBeNull())
    fireEvent.click(screen.getByRole('button', { name: 'Preview' }))
    await screen.findByTestId('dxf-svg')
  })

  it('.dxf that read-file calls binary falls back to bytes', async () => {
    daemonCliGet.mockImplementation(async (_s: unknown, route: string) => {
      if (route === 'fs/read-file') throw new Error('Cannot read binary file')
      if (route === 'fs/read-binary') return { base64: b64(new TextEncoder().encode(BRACKET_DXF)) }
      throw new Error(`unexpected route ${route}`)
    })
    renderInPrimaryRoom(pane('/ws/old.dxf'))
    await screen.findByTestId('dxf-svg')
    expect(callsTo('fs/read-binary')[0][2]).toEqual({ path: '/ws/old.dxf' })
  })

  it('.dxf that is missing says so', async () => {
    daemonCliGet.mockRejectedValue(new Error('File not found'))
    renderInPrimaryRoom(pane('/ws/gone.dxf'))
    expect((await screen.findByTestId('dxf-read-error')).textContent).toContain('File not found')
  })

  it.each(['.stl', '.obj', '.3mf', '.ply', '.glb', '.gltf'])(
    '%s reads bytes (never fs/read-file) and opens the 3D viewer',
    async (ext) => {
      renderInPrimaryRoom(pane(`/ws/part${ext}`))
      await waitFor(() => expect(scene.createViewer).toHaveBeenCalledTimes(1))
      expect(scene.parseModel).toHaveBeenCalledWith(expect.any(Uint8Array), ext)
      expect(callsTo('fs/read-file')).toHaveLength(0)
      expect(screen.queryByRole('button', { name: 'Edit' })).toBeNull()
    },
  )
})
