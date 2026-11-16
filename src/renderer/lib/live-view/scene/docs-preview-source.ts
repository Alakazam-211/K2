// The real picture (S7): K2 Render's stills of a served scene doc from the
// docs preview route (`GET /cli/docs/preview`, preview-route.ts reads its
// answers), behind the same PictureSource seam as the sample's stand-in.
//
// - Served or not: the route lists a doc the daemon draws (`kind: "scene"`);
//   one it doesn't is `409 no_preview` (today every .blend: the BLEND pack's
//   mesh read isn't vendored, so `SceneRenderer::serves` is false). The tab
//   uses this source only when `serves` says yes; otherwise the sample.
// - A still: the head's image at the width's bucket, `look=transparent`
//   (the stage paints behind it), the pane's shape, then its id image
//   (`layer=ids`, a PNG decoded without colour conversion). Sealed for
//   React. Kept by (view, shape, width) until a push or a retry says
//   otherwise.
// - Drawing: a `rendering` answer is asked again every `pollMs` until it
//   lands or fails (no socket is wired before S8; the docs socket's pushes
//   come in through `push` and do the same). A `stale` answer is shown at
//   once while the new one draws. What happens is said to subscribers in
//   the pushes' own shape (`preview_rendering`, `preview_ready`), so the
//   tab folds them with foldPreviewPush whether they came from a poll or
//   the socket.
// - Bounds: each object's local box from the listing (an object it lists
//   without bounds, and every view, has none).

import { httpCodeOf } from '@/lib/daemon-http-error'
import { decodeIdPixels, type IdLayer } from './ids'
import type { CameraLeftOut } from './left-out'
import { sealStill, type ObjectBounds, type PictureSource, type Still, type StillRequest } from './picture-source'
import { addressOfId, parseIdsImage, parseSceneImage, parseSceneListing, widthBucket, type SceneImage, type SceneListing } from './preview-route'
import { releaseStill, StillCache } from './still-cache'
import type { Doc } from './types'

/** How the source asks the route: the query's params, the answer's JSON (a refusal throws, as daemonCliGet does). */
export interface PreviewTransport {
  get(params: Record<string, string>, signal?: AbortSignal): Promise<unknown>
}

/** Turning an answer's bytes into something to draw, and an id PNG into its exact RGB. */
export interface PreviewDecoders {
  image(bytes: Uint8Array, mime: string): Promise<CanvasImageSource & { width: number; height: number }>
  pixels(bytes: Uint8Array, mime: string): Promise<{ width: number; height: number; data: ArrayLike<number> }>
}

/** What happened to a view's picture, in the docs socket's push shape. */
export type PictureEvent =
  | { type: 'preview_rendering'; container: string; rev: number | null }
  | { type: 'preview_ready'; container: string; state: 'ready' | 'failed'; rev: number | null; failed: { code: string } | null; render_ms?: number | null; without_modifiers?: boolean; reused_from?: string | null }
  | { type: 'listing'; container: ''; rev: number | null }

const sleep = (ms: number, signal?: AbortSignal): Promise<void> =>
  new Promise((res, rej) => {
    if (signal?.aborted) return rej(new DOMException('aborted', 'AbortError'))
    const t = setTimeout(res, ms)
    signal?.addEventListener('abort', () => (clearTimeout(t), rej(new DOMException('aborted', 'AbortError'))), { once: true })
  })

export class DocsPreviewPictureSource implements PictureSource {
  readonly kind = 'docs-preview' as const
  private readonly cache = new StillCache()
  private readonly listeners = new Set<(e: PictureEvent) => void>()
  private listing: SceneListing | null = null
  private loading: Promise<SceneListing> | null = null
  private readonly boundsBy = new Map<string, ObjectBounds>()
  private closed = false

  constructor(
    private readonly t: PreviewTransport,
    private readonly d: PreviewDecoders,
    private readonly o: { pollMs?: number; pollMax?: number } = {},
  ) {}

  /** Whether the daemon draws this doc: it lists it as a scene. A refusal (`no_preview`, an older daemon) or any other answer: no. */
  static async serves(t: PreviewTransport, signal?: AbortSignal): Promise<boolean> {
    try {
      parseSceneListing(await t.get({}, signal))
      return true
    } catch (e) {
      const code = httpCodeOf(e)
      if (code && code !== 'no_preview') console.warn(`[live scene] docs/preview: ${code}; the sample stays`)
      return false
    }
  }

  subscribe(fn: (e: PictureEvent) => void): () => void {
    this.listeners.add(fn)
    return () => this.listeners.delete(fn)
  }

  private emit(e: PictureEvent): void {
    for (const fn of this.listeners) fn(e)
  }

  /** The listing (once; again after a push). */
  ready(): Promise<SceneListing> {
    if (this.listing) return Promise.resolve(this.listing)
    this.loading ??= this.t.get({}).then((b) => {
      const L = parseSceneListing(b)
      this.listing = L
      this.boundsBy.clear()
      for (const c of L.containers) if (c.bounds) this.boundsBy.set(c.key, c.bounds)
      this.emit({ type: 'listing', container: '', rev: L.rev })
      return L
    })
    return this.loading.finally(() => (this.loading = null))
  }

  /** The cameras the mesh read left out, as the listing says (none until it is in). */
  camerasLeftOut(): CameraLeftOut[] {
    return this.listing?.camerasLeftOut ?? []
  }

  bounds(_D: Doc, a: string): ObjectBounds | null {
    return this.boundsBy.get(a) ?? null
  }

  /** The listed object under pixel (x, y) of a still (its id image), by address. */
  addressAt(st: Still, x: number, y: number): string | null {
    return this.listing && st.ids ? addressOfId(this.listing, st.ids, x, y) : null
  }

  /** A push from the doc's socket: a view drawing or drawn drops its cached still, and is passed on. */
  push(frame: unknown): void {
    const f = frame as { type?: unknown; container?: unknown }
    if (typeof f?.container !== 'string' || (f.type !== 'preview_rendering' && f.type !== 'preview_ready')) return
    this.drop(f.container)
    this.emit(frame as PictureEvent)
  }

  /** Ask a view again (after a failure): its cached still goes, and the tab asks. */
  retry(view: string): void {
    this.drop(view)
    this.emit({ type: 'preview_rendering', container: view, rev: this.listing?.rev ?? null })
  }

  private drop(view: string): void {
    for (const k of this.cache.keys()) if (k.startsWith(`${view}|`)) this.cache.forget(k)
  }

  async still(req: StillRequest, signal?: AbortSignal): Promise<Still | null> {
    if (this.closed) return null
    const width = widthBucket(req.width)
    const key = `${req.view}|${req.shape}|${width}|${req.ids ? 'ids' : ''}`
    const hit = this.cache.get(key)
    if (hit) return hit
    const params = { container: req.view, width, look: 'transparent', shape: req.shape }
    void this.ready().catch(() => undefined)
    let a: SceneImage
    try {
      a = parseSceneImage(await this.t.get(params, signal))
      for (let n = 0; a.state === 'rendering' && n < (this.o.pollMax ?? 40); n++) {
        this.emit({ type: 'preview_rendering', container: req.view, rev: a.rev })
        await sleep(this.o.pollMs ?? 700, signal)
        a = parseSceneImage(await this.t.get(params, signal))
      }
    } catch (e) {
      if ((e as { name?: string })?.name === 'AbortError' || signal?.aborted) return null
      this.emit({ type: 'preview_ready', container: req.view, state: 'failed', rev: null, failed: { code: httpCodeOf(e) ?? 'unreachable' } })
      return null
    }
    if (a.state === 'failed' || a.state === 'rendering') {
      this.emit({ type: 'preview_ready', container: req.view, state: 'failed', rev: a.rev, failed: { code: a.failedCode ?? 'timeout' } })
      return null
    }
    const image = await this.d.image(a.bytes!, a.mime ?? 'image/webp')
    const ids = req.ids && a.ids ? await this.idLayer(params, a, signal) : null
    if (this.closed || signal?.aborted) {
      releaseStill({ image: { kind: 'bitmap', image } } as Still)
      return null
    }
    const st = sealStill({
      rev: req.rev,
      view: req.view,
      shape: req.shape,
      width: a.width ?? image.width,
      height: a.height ?? image.height,
      image: { kind: 'bitmap', image },
      camera: a.camera,
      ids,
      renderMs: a.renderMs,
      meta: a.meta,
    })
    // A stale image is shown, not kept: the current one is still drawing, and is followed until it lands.
    if (a.state === 'ready') this.cache.set(key, st)
    else void this.follow(req, params)
    this.emit(a.state === 'ready' ? { type: 'preview_ready', container: req.view, state: 'ready', rev: a.rev, failed: null, render_ms: a.renderMs, without_modifiers: a.withoutModifiers, reused_from: a.reusedFrom } : { type: 'preview_rendering', container: req.view, rev: a.rev })
    return st
  }

  /** Ask again until the current still lands (then it is cached and said ready) or fails. */
  private async follow(req: StillRequest, params: Record<string, string>): Promise<void> {
    for (let n = 0; n < (this.o.pollMax ?? 40) && !this.closed; n++) {
      await sleep(this.o.pollMs ?? 700)
      let a: SceneImage
      try {
        a = parseSceneImage(await this.t.get(params))
      } catch {
        return
      }
      if (a.state === 'ready') return void (await this.still(req))
      if (a.state === 'failed') return this.emit({ type: 'preview_ready', container: req.view, state: 'failed', rev: a.rev, failed: { code: a.failedCode ?? 'timeout' } })
    }
  }

  /** The still's id image (none when the route has none: drawn without one, or gone). */
  private async idLayer(params: Record<string, string>, a: SceneImage, signal?: AbortSignal): Promise<IdLayer | null> {
    try {
      const r = parseIdsImage(await this.t.get({ ...params, layer: 'ids' }, signal))
      if (r.state !== 'ready' || !r.bytes) return null
      const px = await this.d.pixels(r.bytes, r.mime ?? 'image/png')
      return { width: px.width, height: px.height, values: decodeIdPixels(px.data, px.width, px.height), objects: a.ids?.objects ?? [] }
    } catch {
      return null
    }
  }

  close(): void {
    this.closed = true
    this.cache.clear()
    this.listeners.clear()
  }
}
