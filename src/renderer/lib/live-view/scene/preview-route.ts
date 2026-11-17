// The docs preview route's scene answers (`GET /cli/docs/preview`,
// crates/k2-daemon/src/docs/preview/route.rs and extras.rs) and the docs
// socket's preview pushes, read into the tab's shapes:
// - the listing (no `container`): `{kind: "scene", rev, look, shape,
//   containers: [{key, position, title, hidden, hidden_by?, render_hash,
//   previews, bounds?, camera?}]}`. Addresses are K2 Render's native view
//   addresses (`@scene:main`, `@cam:…`, `@obj:…`, `@turntable:…`);
// - a still (`container` + `width`): `ready` or `stale` with its base64,
//   `size`, `look`, `shape`, the `camera` that drew it, its id table
//   (`ids {layer, image_len, count, objects}`) and what it left out
//   (`base_values`, `hidden_left_out`, and the counts); `rendering` (202,
//   `preview_ready` follows); `failed {code}`;
// - its id image (`layer=ids`): a PNG at the still's size;
// - the pushes: `preview_rendering {container, rev, …}` and `preview_ready
//   {container, state, failed, rev, render_ms, without_modifiers,
//   reused_from?, …}`, folded per view into a PictureStatus.
// The bounds, cameras and meta are read by listing-read.ts (one place for
// those keys). Loud on a shape this client doesn't know; quiet (null) on a
// field it can do without.
//
// A live scene's units carry the pack's derived `mesh_hash` and `hidden_by`
// ("self" or "collection:@coll:N") on the keyed watch (keyed-doc.ts reads
// them); the listing's hidden ("self" or "collection", never an address)
// is the picture's own, for the sample and before the watch is in.

import { entryOf, idIndexAt, type IdEntry, type IdLayer } from './ids'
import type { CameraLeftOut } from './left-out'
import { listingCameraFrom, objectListingFrom, stillCameraFrom, stillMetaFrom, type ListingCamera } from './listing-read'
import type { ObjectBounds, PictureStatus, StillMeta } from './picture-source'
import type { StillCamera } from './project'

type Raw = Record<string, unknown>
const isObj = (v: unknown): v is Raw => !!v && typeof v === 'object' && !Array.isArray(v)
function obj(v: unknown, what: string): Raw {
  if (!isObj(v)) throw new Error(`docs/preview: ${what} is not an object`)
  return v
}
const str = (v: unknown): string | null => (typeof v === 'string' ? v : null)
const int = (v: unknown): number | null => (typeof v === 'number' && Number.isFinite(v) ? v : null)

/** The route's width buckets. */
export type RouteWidth = 'thumb' | 'thumb2x' | 'full'
/** The bucket for a still of `px` pixels across (thumb 320, thumb2x 640, full 1920). */
export const widthBucket = (px: number): RouteWidth => (px <= 400 ? 'thumb' : px <= 960 ? 'thumb2x' : 'full')

export interface SceneContainer {
  key: string
  position: number
  title: string
  hidden: boolean
  hiddenBy: string | null
  renderHash: string
  /** An object's local bounds (null for a view, or an object with none). */
  bounds: ObjectBounds | null
  /** A camera's display values. */
  camera: ListingCamera | null
}

export interface SceneListing {
  rev: number
  look: string | null
  shape: string | null
  containers: SceneContainer[]
  /** The cameras the mesh read left out (`cameras_left_out`, passed through by the route); none when it says nothing. */
  camerasLeftOut: CameraLeftOut[]
  /** More left out than are named: past the route's cap (`cameras_left_out_more`), and entries dropped as bad (`cameras_left_out_dropped`, plus any this reader can't read). Counted, never named. */
  camerasLeftOutMore: number
}

const count = (v: unknown): number => (typeof v === 'number' && Number.isInteger(v) && v > 0 ? v : 0)

/** The cameras left out: those named, and how many more (past the cap, dropped by the route, or unreadable here). */
function leftOut(b: Raw): { camerasLeftOut: CameraLeftOut[]; camerasLeftOutMore: number } {
  const raw = Array.isArray(b.cameras_left_out) ? b.cameras_left_out : []
  const named = raw.flatMap((c): CameraLeftOut[] => (isObj(c) && typeof c.name === 'string' && typeof c.reason === 'string' ? [{ name: c.name, key: str(c.key), reason: c.reason }] : []))
  return { camerasLeftOut: named, camerasLeftOutMore: count(b.cameras_left_out_more) + count(b.cameras_left_out_dropped) + (raw.length - named.length) }
}

export function parseSceneListing(body: unknown): SceneListing {
  const b = obj(body, 'the answer')
  if (b.kind !== 'scene') throw new Error(`docs/preview: kind ${String(b.kind)} is not a scene`)
  if (!Array.isArray(b.containers)) throw new Error('docs/preview: no containers')
  return {
    rev: int(b.rev) ?? 0,
    look: str(b.look),
    shape: str(b.shape),
    ...leftOut(b),
    containers: b.containers.map((c, i) => {
      const o = obj(c, `containers[${i}]`)
      const key = str(o.key)
      if (!key) throw new Error(`docs/preview: containers[${i}] has no key`)
      return {
        key,
        position: int(o.position) ?? i + 1,
        title: str(o.title) ?? '',
        hidden: o.hidden === true,
        hiddenBy: str(o.hidden_by),
        renderHash: str(o.render_hash) ?? '',
        bounds: objectListingFrom(o),
        camera: isObj(o.camera) ? listingCameraFrom(o.camera) : null,
      }
    }),
  }
}

export interface SceneImage {
  state: 'ready' | 'stale' | 'rendering' | 'failed'
  container: string | null
  renderHash: string
  rev: number | null
  bytes: Uint8Array | null
  mime: string | null
  width: number | null
  height: number | null
  look: string | null
  shape: string | null
  camera: StillCamera | null
  ids: { count: number; objects: IdEntry[] } | null
  meta: StillMeta
  withoutModifiers: boolean
  renderMs: number | null
  reusedFrom: string | null
  failedCode: string | null
}

function decodeBase64(b64: string): Uint8Array {
  const bin = atob(b64)
  const out = new Uint8Array(bin.length)
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i)
  return out
}

function idTable(v: unknown): SceneImage['ids'] {
  if (!isObj(v) || !Array.isArray(v.objects)) return null
  const objects = v.objects
    .flatMap((e): IdEntry[] => (isObj(e) && typeof e.index === 'number' && typeof e.id === 'string' ? [{ index: e.index, id: e.id, ...(typeof e.key === 'string' ? { key: e.key } : {}) }] : []))
    .sort((a, b) => a.index - b.index)
  return { count: int(v.count) ?? objects.length, objects }
}

/** A still's answer (or a stale one's). */
export function parseSceneImage(body: unknown): SceneImage {
  const b = obj(body, 'the answer')
  const s = b.state
  if (s !== 'ready' && s !== 'stale' && s !== 'rendering' && s !== 'failed') throw new Error(`docs/preview: state ${String(s)} is not an image state`)
  const has = s === 'ready' || s === 'stale'
  if (has && typeof b.base64 !== 'string') throw new Error('docs/preview: an image answer without its bytes')
  const size = has && isObj(b.size) ? b.size : null
  return {
    state: s,
    container: str(b.container),
    renderHash: str(b.render_hash) ?? '',
    rev: int(b.rev),
    bytes: has ? decodeBase64(b.base64 as string) : null,
    mime: has ? str(b.mime) : null,
    width: size ? int(size.w) : null,
    height: size ? int(size.h) : null,
    look: str(b.look),
    shape: str(b.shape),
    camera: stillCameraFrom(b.camera),
    ids: idTable(b.ids),
    meta: stillMetaFrom(b),
    withoutModifiers: b.without_modifiers === true,
    renderMs: int(b.render_ms),
    reusedFrom: str(b.reused_from),
    failedCode: s === 'failed' && isObj(b.failed) ? str(b.failed.code) : null,
  }
}

/** An id image's answer (`layer=ids`): a PNG, or not drawn yet. */
export function parseIdsImage(body: unknown): { state: SceneImage['state']; bytes: Uint8Array | null; mime: string | null } {
  const b = obj(body, 'the answer')
  const s = b.state === 'ready' || b.state === 'rendering' || b.state === 'failed' ? b.state : null
  if (!s) throw new Error(`docs/preview: state ${String(b.state)} is not an id image state`)
  if (s === 'ready' && typeof b.base64 !== 'string') throw new Error('docs/preview: an id image answer without its bytes')
  return { state: s, bytes: s === 'ready' ? decodeBase64(b.base64 as string) : null, mime: s === 'ready' ? (str(b.mime) ?? 'image/png') : null }
}

/** The address of the object at pixel (x, y): its entry's `key` when the table names it (K2 Render adds it after
 *  0.46.14); else its id, matched to the one listed object with that title. Null for the background, an index the
 *  table left out, or a name two listed objects share. */
export function addressOfId(listing: SceneListing, layer: IdLayer, x: number, y: number): string | null {
  const i = idIndexAt(layer, x, y)
  const e = i === null ? null : entryOf(layer, i)
  if (e?.key && /^@obj:(0|[1-9]\d*)$/.test(e.key)) return e.key
  const id = e?.id
  if (!id) return null
  const hits = listing.containers.filter((c) => c.key.startsWith('@obj:') && c.title === id)
  return hits.length === 1 ? hits[0].key : null
}

/** A push on the doc's socket, folded into one view's picture state (another view's push leaves it as it is). */
export function foldPreviewPush(st: PictureStatus, frame: unknown, view: string): PictureStatus {
  if (!isObj(frame) || frame.container !== view) return st
  const rev = int(frame.rev)
  if (frame.type === 'preview_rendering') {
    const failedBefore = st.state === 'failed'
    return { ...st, want: rev ?? st.want, state: st.shown === null ? 'rendering' : 'stale', landed: false, recovered: false, retrying: failedBefore, again: false }
  }
  if (frame.type !== 'preview_ready') return st
  if (frame.state === 'failed') {
    const code = isObj(frame.failed) ? str(frame.failed.code) : null
    return { ...st, want: rev ?? st.want, state: 'failed', reason: code, landed: false, again: st.retrying, retrying: false }
  }
  if (frame.state !== 'ready') return st
  return {
    ...st,
    shown: rev ?? st.shown,
    want: rev ?? st.want,
    state: 'ready',
    renderMs: int(frame.render_ms),
    reusedFrom: typeof frame.reused_from === 'string' && frame.reused_from ? rev : null,
    withoutModifiers: frame.without_modifiers === true,
    reason: null,
    landed: true,
    recovered: st.state === 'failed' || st.retrying,
    retrying: false,
    again: false,
  }
}
