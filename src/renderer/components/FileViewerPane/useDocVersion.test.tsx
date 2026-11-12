// @vitest-environment jsdom
//
// Live re-read of the any-file previews: when the daemon says a file has a
// new version (`doc_version_changed`, from any writer), an open 3D model,
// large DXF or image reads its bytes again and shows the new version, without
// closing the tab. Another file's event, or the same version twice, reads
// nothing. jsdom has no WebGL, so `./modelScene` is a stand-in.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, renderHook, screen, waitFor } from '@testing-library/react'
import type { DocVersionChangedEvent } from '@/stores/session-events'
import type { ServerScope } from '@/kessel/server-scope'

const bus = vi.hoisted(() => ({ handlers: new Set<(e: DocVersionChangedEvent) => void>() }))
vi.mock('@/stores/session-events', async (orig) => ({
  ...(await orig<typeof import('@/stores/session-events')>()),
  onDocVersionChanged: (_scope: unknown, fn: (e: DocVersionChangedEvent) => void) => {
    bus.handlers.add(fn)
    return () => void bus.handlers.delete(fn)
  },
}))

const host = vi.hoisted(() => ({
  bytes: new Map<string, Uint8Array>(),
  loadHostBinary: vi.fn(),
  loadHostImageObjectUrl: vi.fn(),
  revokeObjectUrl: vi.fn(),
}))
vi.mock('@/lib/load-host-binary', () => ({
  loadHostBinary: (...args: unknown[]) => host.loadHostBinary(...args),
  loadHostImageObjectUrl: (...args: unknown[]) => host.loadHostImageObjectUrl(...args),
  revokeObjectUrl: (...args: unknown[]) => host.revokeObjectUrl(...args),
}))
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

const scene = vi.hoisted(() => {
  const viewers: Array<{ dispose: ReturnType<typeof vi.fn>; reset: ReturnType<typeof vi.fn>; view: ReturnType<typeof vi.fn> }> = []
  return {
    viewers,
    parseModel: vi.fn(async (bytes: Uint8Array, _ext: string) => {
      const n = new TextDecoder().decode(bytes).split('\n').filter((l) => l.startsWith('f ')).length
      return { root: { faces: n }, stats: { triangles: n, meshes: 1, size: [1, 1, 1] as [number, number, number] } }
    }),
    hasWebGL2: vi.fn(() => true),
    createViewer: vi.fn((_host: HTMLElement, _root: unknown, _keep?: unknown) => {
      const i = viewers.length
      const v = {
        dispose: vi.fn(),
        reset: vi.fn(),
        view: vi.fn(() => ({ position: [i + 1, 2, 3], target: [0, i, 0] })),
      }
      viewers.push(v)
      return v
    }),
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
import { DxfHostBytesViewer } from './DxfViewer'
import { ImageViewer } from './ImageViewer'
import { ModelViewer } from './ModelViewer'
import { docEventFilePath, useDocVersion } from './useDocVersion'

const WS = '/srv/acme'

/** An ASCII DXF of `n` circles in a row. */
function circles(n: number): string {
  const out = ['0', 'SECTION', '2', 'ENTITIES']
  for (let i = 0; i < n; i++) out.push('0', 'CIRCLE', '8', '0', '10', String(i * 30), '20', '0', '30', '0', '40', '10')
  out.push('0', 'ENDSEC', '0', 'EOF')
  return out.join('\n') + '\n'
}

function ev(rel: string, version: number): DocVersionChangedEvent {
  return { kind: 'doc_version_changed', projectId: 'p1', workspace: WS, rel, version }
}

function send(e: DocVersionChangedEvent): void {
  act(() => {
    for (const fn of bus.handlers) fn(e)
  })
}

/** An OBJ with `n` triangles. */
function obj(n: number): Uint8Array {
  const lines = ['v 0 0 0', 'v 1 0 0', 'v 0 1 0']
  for (let i = 0; i < n; i++) lines.push('f 1 2 3')
  return new TextEncoder().encode(lines.join('\n') + '\n')
}

beforeEach(() => {
  bus.handlers.clear()
  host.bytes.clear()
  scene.viewers.length = 0
  host.loadHostBinary.mockReset()
  host.loadHostBinary.mockImplementation(async (_scope: unknown, path: string) => {
    const b = host.bytes.get(path)
    if (!b) throw new Error(`no such file ${path}`)
    return b
  })
  host.loadHostImageObjectUrl.mockReset()
  host.revokeObjectUrl.mockReset()
  scene.parseModel.mockClear()
  scene.createViewer.mockClear()
  scene.disposeObject.mockClear()
  scene.hasWebGL2.mockReturnValue(true)
})
afterEach(() => cleanup())

describe('useDocVersion', () => {
  const scope = { id: 'local', connectionKey: 'local' } as unknown as ServerScope

  it('joins the workspace and rel into the file path', () => {
    expect(docEventFilePath({ workspace: '/srv/acme/', rel: 'play/house.obj' })).toBe('/srv/acme/play/house.obj')
    expect(docEventFilePath({ workspace: '/srv/acme', rel: '/a.png' })).toBe('/srv/acme/a.png')
  })

  it("follows this file's versions only, and a repeat is no change", () => {
    const { result } = renderHook(() => useDocVersion(scope, `${WS}/play/house.obj`))
    expect(result.current).toBeNull()
    send(ev('play/other.obj', 7))
    expect(result.current).toBeNull()
    send(ev('play/house.obj', 2))
    expect(result.current).toBe(2)
    send(ev('play/house.obj', 3))
    expect(result.current).toBe(3)
  })

  it('a new path starts over', () => {
    const { result, rerender } = renderHook(({ p }) => useDocVersion(scope, p), {
      initialProps: { p: `${WS}/a.obj` },
    })
    send(ev('a.obj', 5))
    expect(result.current).toBe(5)
    rerender({ p: `${WS}/b.obj` })
    expect(result.current).toBeNull()
  })
})

describe('ModelViewer re-reads a new version', () => {
  const path = `${WS}/play/house.obj`

  it('reads the new bytes, swaps the viewer in with the camera kept, and leaves the old one only after', async () => {
    host.bytes.set(path, obj(1))
    renderInPrimaryRoom(<ModelViewer filePath={path} />)
    expect((await screen.findByTestId('model-stats')).textContent).toContain('1 triangle ')
    await waitFor(() => expect(scene.createViewer).toHaveBeenCalledTimes(1))
    expect(scene.createViewer.mock.calls[0][2]).toBeNull()

    // Three writes by another writer: v2, v3, v4.
    for (const [v, n] of [[2, 2], [3, 3], [4, 4]] as const) {
      host.bytes.set(path, obj(n))
      send(ev('play/house.obj', v))
      await waitFor(() => expect(screen.getByTestId('model-stats').textContent).toContain(`${n} triangles`))
      await waitFor(() => expect(scene.createViewer).toHaveBeenCalledTimes(n))
      const before = scene.viewers[n - 2]
      // The camera of the viewer it replaced, and that one freed after.
      expect(scene.createViewer.mock.calls[n - 1][2]).toEqual(before.view.mock.results[0].value)
      expect(before.dispose).toHaveBeenCalledTimes(1)
      expect(scene.viewers[n - 1].dispose).not.toHaveBeenCalled()
      // No "Loading model…" flash: the old model stayed up.
      expect(screen.queryByText('Loading model…')).toBeNull()
    }
    expect(host.loadHostBinary).toHaveBeenCalledTimes(4)
    expect(scene.parseModel.mock.calls.map((c) => c[1])).toEqual(['.obj', '.obj', '.obj', '.obj'])
  })

  it("another file's version, or the same version again, reads nothing", async () => {
    host.bytes.set(path, obj(1))
    renderInPrimaryRoom(<ModelViewer filePath={path} />)
    await waitFor(() => expect(scene.createViewer).toHaveBeenCalledTimes(1))
    send(ev('play/garage.obj', 2))
    send(ev('play/house.obj', 2))
    await waitFor(() => expect(scene.createViewer).toHaveBeenCalledTimes(2))
    send(ev('play/house.obj', 2))
    // Give a stray re-read time to show up.
    await new Promise((r) => setTimeout(r, 30))
    expect(host.loadHostBinary).toHaveBeenCalledTimes(2)
    expect(scene.createViewer).toHaveBeenCalledTimes(2)
  })

  it('a failed re-read shows its note and frees the old viewer', async () => {
    host.bytes.set(path, obj(1))
    renderInPrimaryRoom(<ModelViewer filePath={path} />)
    await waitFor(() => expect(scene.createViewer).toHaveBeenCalledTimes(1))
    host.bytes.delete(path)
    send(ev('play/house.obj', 2))
    expect((await screen.findByTestId('model-error')).textContent).toContain('no such file')
    expect(scene.viewers[0].dispose).toHaveBeenCalledTimes(1)
  })

  it('unmount frees the viewer on screen once', async () => {
    host.bytes.set(path, obj(1))
    const view = renderInPrimaryRoom(<ModelViewer filePath={path} />)
    await waitFor(() => expect(scene.createViewer).toHaveBeenCalledTimes(1))
    host.bytes.set(path, obj(2))
    send(ev('play/house.obj', 2))
    await waitFor(() => expect(scene.createViewer).toHaveBeenCalledTimes(2))
    view.unmount()
    expect(scene.viewers[0].dispose).toHaveBeenCalledTimes(1)
    expect(scene.viewers[1].dispose).toHaveBeenCalledTimes(1)
  })
})

describe('the large-DXF bytes view re-reads a new version', () => {
  it('draws the new drawing', async () => {
    const path = `${WS}/plans/old.dxf`
    host.bytes.set(path, new TextEncoder().encode(circles(1)))
    renderInPrimaryRoom(<DxfHostBytesViewer filePath={path} />)
    const svg = await screen.findByTestId('dxf-svg')
    expect(svg.querySelectorAll('circle')).toHaveLength(1)
    host.bytes.set(path, new TextEncoder().encode(circles(3)))
    send(ev('plans/old.dxf', 2))
    await waitFor(() => expect(screen.getByTestId('dxf-svg').querySelectorAll('circle')).toHaveLength(3))
    expect(host.loadHostBinary).toHaveBeenCalledTimes(2)
  })
})

describe('ImageViewer re-reads a new version', () => {
  beforeEach(() => {
    vi.stubGlobal(
      'ResizeObserver',
      class {
        observe(): void {}
        unobserve(): void {}
        disconnect(): void {}
      },
    )
  })
  afterEach(() => vi.unstubAllGlobals())

  it('swaps the image in place and frees the old URL after', async () => {
    const path = `${WS}/art/logo.png`
    let n = 0
    host.loadHostImageObjectUrl.mockImplementation(async () => {
      n += 1
      return { url: `blob:img-${n}`, mime: 'image/png', byteLength: 10 * n }
    })
    renderInPrimaryRoom(<ImageViewer filePath={path} alt="logo" />)
    await waitFor(() => expect(document.querySelector('img')?.getAttribute('src')).toBe('blob:img-1'))
    send(ev('art/logo.png', 2))
    await waitFor(() => expect(document.querySelector('img')?.getAttribute('src')).toBe('blob:img-2'))
    expect(screen.queryByText('Loading image...')).toBeNull()
    expect(host.revokeObjectUrl).toHaveBeenCalledWith('blob:img-1')
    expect(host.revokeObjectUrl).not.toHaveBeenCalledWith('blob:img-2')
  })
})
