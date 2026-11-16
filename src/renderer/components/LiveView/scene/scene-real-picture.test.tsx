// @vitest-environment jsdom
// The real picture on the tab (S7): the source is the docs preview route's
// only when the daemon draws the file (else, and outside a file tab, the
// sample's stand-in), and the stage reads its state from it: the first
// still being drawn ("Opening…"), a view K2 couldn't draw ("Couldn't draw
// this view", Retry), the retry landing, its camera and id layer arriving
// with it.

import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { act, cleanup, fireEvent, render, renderHook, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { DocsPreviewPictureSource, type PreviewTransport } from '@/lib/live-view/scene/docs-preview-source'
import { SCENARIOS } from '@/lib/live-view/scene/scenarios-more'
import { simulate } from '@/lib/live-view/scene/turn'
import { LiveFileContext } from '../live-file'
import { SceneStage } from './SceneStage'

const DIR = join(__dirname, '..', '..', '..', '..', '..', 'tests', 'fixtures', 'scene-preview')
const json = (f: string): Record<string, unknown> => JSON.parse(readFileSync(join(DIR, f), 'utf8'))
const b64 = (f: string): string => readFileSync(join(DIR, f)).toString('base64')

const daemon = vi.hoisted(() => ({ answer: null as null | ((p: Record<string, unknown>) => unknown) }))
vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet: async (_s: unknown, _r: string, p: Record<string, unknown>) => daemon.answer!(p) }))
vi.mock('@/components/Room/RoomContext', () => ({ useOptionalRoom: () => ({ scope: { connectionKey: 'test' } }) }))
const { useScenePictures } = await import('./useScenePictures')

function fixtureRoute(fail: () => boolean): PreviewTransport {
  return {
    async get(p) {
      if (!p.container) return { kind: 'scene', ...json('listing.json') }
      if (p.container !== '@scene:main' || fail()) return { kind: 'scene', container: p.container, render_hash: 'h', rev: 1, state: 'failed', failed: { code: 'timeout' } }
      const meta = json('scene.json')
      if (p.layer === 'ids') return { kind: 'scene', container: p.container, render_hash: meta.render_hash, state: 'ready', layer: 'ids', mime: 'image/png', base64: b64('scene.ids.png') }
      return { ...meta, base64: b64('scene.webp') }
    },
  }
}
const decoders = {
  image: async () => ({ width: 640, height: 360 }) as unknown as CanvasImageSource & { width: number; height: number },
  pixels: async () => ({ width: 640, height: 360, data: new Uint8ClampedArray(640 * 360 * 4) }),
}

beforeEach(() => {
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue(null)
})
afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

describe('which source the tab uses', () => {
  const wrap = (path: string | null) => ({ children }: { children: React.ReactNode }) => <LiveFileContext.Provider value={path ? { path } : null}>{children}</LiveFileContext.Provider>
  const S = simulate(SCENARIOS[0], 0)

  it('the daemon draws the file: the docs preview route’s', async () => {
    daemon.answer = (p) => (p.container ? null : { kind: 'scene', ...json('listing.json') })
    const { result } = renderHook(() => useScenePictures(S), { wrapper: wrap('/ws/living-room.blend') })
    await waitFor(() => expect(result.current?.kind).toBe('docs-preview'))
  })

  it('it doesn’t (409 no_preview: the mesh read isn’t built): the sample', async () => {
    daemon.answer = () => {
      throw Object.assign(new Error('K2 draws no previews of this kind of file yet'), { status: 409, code: 'no_preview' })
    }
    const { result } = renderHook(() => useScenePictures(S), { wrapper: wrap('/ws/living-room.blend') })
    await waitFor(() => expect(result.current?.kind).toBe('sample'))
  })

  it('outside a file tab: the sample, and the daemon isn’t asked', async () => {
    const asked = vi.fn()
    daemon.answer = (p) => (asked(p), null)
    const { result } = renderHook(() => useScenePictures(S), { wrapper: wrap(null) })
    await waitFor(() => expect(result.current?.kind).toBe('sample'))
    expect(asked).not.toHaveBeenCalled()
  })
})

describe('the stage on the real picture', () => {
  const S = simulate(SCENARIOS[0], 2)
  const mount = (source: DocsPreviewPictureSource) => render(<SceneStage S={S} control={{ retryPicture: () => undefined }} source={source} narrow={false} size={{ width: 1100, height: 700 }} offscreen={[]} onJump={() => undefined} />)
  const stage = () => screen.getByTestId('lv-scene-stage')

  it('says which source it draws from', async () => {
    mount(new DocsPreviewPictureSource(fixtureRoute(() => false), decoders))
    expect(stage().getAttribute('data-source')).toBe('docs-preview')
  })

  it('opens, then the still lands: ready, with its camera (the overlay draws over it)', async () => {
    mount(new DocsPreviewPictureSource(fixtureRoute(() => false), decoders))
    await waitFor(() => expect(stage().getAttribute('data-state')).toBe('ready'))
    expect(screen.getByTestId('lv-scene-overlay')).toBeTruthy()
  })

  it('a view K2 couldn’t draw: the card with Retry; the retry lands', async () => {
    let fail = true
    mount(new DocsPreviewPictureSource(fixtureRoute(() => fail), decoders))
    await waitFor(() => expect(stage().getAttribute('data-state')).toBe('failed'))
    expect(screen.getByTestId('lv-scene-card').textContent).toContain('Couldn’t draw this view')
    fail = false
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Retry' })))
    await waitFor(() => expect(stage().getAttribute('data-state')).toBe('ready'))
    expect(screen.queryByTestId('lv-scene-card')).toBeNull()
  })
})
