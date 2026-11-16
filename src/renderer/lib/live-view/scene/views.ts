// The views of a scene and how the tab moves between them, keyed by K2
// Render's own view addresses (view-address.ts: `@scene:main`,
// `@cam:<id>`, `@obj:<id>`, `@turntable:<n>`): the view picker's rows
// (Scene, each file camera, the framed object, Turntable, and the agent's
// view under a divider), the keys on the picture (Home Scene, 0 the file's
// main camera, . frame the selection, Esc back), and an object's or a
// camera's view from the doc. Pure; the words are the mock's (v2.5).

import { GOLD } from './samples'
import { UNITS, nameOf, objOf } from './fold'
import type { Doc } from './types'
import { SCENE_VIEW, camView, decId, dupView, objView, parseView, turntableView } from './view-address'

export type ViewKind = 'scene' | 'camera' | 'object' | 'turntable' | 'agent'

export interface ViewRow {
  /** The view's address (`@obj:*` while nothing is selected to frame; `@agent` for the agent's view). */
  v: string
  kind: ViewKind
  name: string
  sub: string
  /** The key that picks it on the picture, if any. */
  key: string
  disabled: boolean
  /** A divider before it. */
  sep: boolean
}

export const isTurntable = (v: string): boolean => v.startsWith('@turntable:')
export function kindOf(v: string): ViewKind {
  if (v === '@agent') return 'agent'
  if (v === '@obj:*') return 'object'
  return parseView(v)?.kind ?? 'scene'
}

/** Same view for the picker's check: any framed object is "Framed object", any turntable frame is "Turntable". */
export const sameView = (a: string, b: string): boolean => a === b || (kindOf(a) === 'object' && kindOf(b) === 'object') || (isTurntable(a) && isTurntable(b))

const units = (D: Doc): string[] => D.lists[UNITS] ?? []

/** An object's view: `@obj:<its name>`, or `@obj-n:<index>` when another object has the same name. */
export function objectView(D: Doc, addr: string): string {
  const name = nameOf(D, addr)
  const all = units(D)
  return all.filter((a) => nameOf(D, a) === name).length > 1 ? dupView('object', all.indexOf(addr)) : objView(name)
}

/** A camera object's view: `@cam:<its name>`, or `@cam-n:<index>` when another camera has the same name. */
export function cameraView(D: Doc, addr: string): string {
  const name = nameOf(D, addr)
  const all = units(D)
  return all.filter((a) => objOf(D, a).kind === 'camera' && nameOf(D, a) === name).length > 1 ? dupView('camera', all.indexOf(addr)) : camView(name)
}

/** The object (its kit address) a camera or object view names, when it is in the doc. */
export function objectOfView(D: Doc, v: string): string | null {
  const p = parseView(v)
  if (!p || (p.kind !== 'camera' && p.kind !== 'object')) return null
  const all = units(D)
  const fits = (a: string): boolean => p.kind === 'object' || objOf(D, a).kind === 'camera'
  if ('index' in p) {
    const a = all[p.index]
    return a && fits(a) ? a : null
  }
  return all.find((a) => fits(a) && nameOf(D, a) === p.id) ?? null
}

/** The file's cameras that are in the doc, in the view list's order (the sample's listing; S7 reads K2 Render's). */
export function cameras(D: Doc): { view: string; object: string; active: boolean }[] {
  return GOLD.previews.views.filter((v) => v.kind === 'camera' && v.object && D.props[v.object]).map((v) => ({ view: cameraView(D, v.object as string), object: v.object as string, active: !!v.active }))
}

/** The file's main camera, when it has one. */
export const mainCamera = (D: Doc): string | null => cameras(D).find((c) => c.active)?.view ?? null

/** Until the mesh read carries camera names to show, cameras read "Camera 1", "Camera 2", in the view list's order. */
export function cameraLabel(D: Doc, v: string): string {
  const k = cameras(D).findIndex((c) => c.view === v)
  return k >= 0 ? `Camera ${k + 1}` : 'Camera'
}

/** The view's name on the picker: "Scene", "Camera 1", "Table, framed", "Turntable". */
export function viewName(D: Doc, v: string): string {
  const p = parseView(v)
  if (!p || p.kind === 'scene') return 'Scene'
  if (p.kind === 'turntable') return 'Turntable'
  if (p.kind === 'camera') return cameraLabel(D, v)
  const a = objectOfView(D, v)
  return `${a ? nameOf(D, a) : 'id' in p ? decId(p.id) : 'Object'}, framed`
}

/** The picker's rows. `framed` is the view "Framed object" frames (your pick, else the agent's); `agent` the agent's view when it has one. */
export function viewRows(D: Doc, o: { framed: string | null; agent: { name: string; view: string } | null }): ViewRow[] {
  const framed = o.framed ? objectOfView(D, o.framed) : null
  const rows: ViewRow[] = [{ v: SCENE_VIEW, kind: 'scene', name: 'Scene', sub: 'The whole room', key: 'Home', disabled: false, sep: false }]
  for (const c of cameras(D)) rows.push({ v: c.view, kind: 'camera', name: cameraLabel(D, c.view), sub: c.active ? 'The file’s main camera' : 'A camera in the file', key: c.active ? '0' : '', disabled: false, sep: false })
  rows.push({ v: o.framed ?? '@obj:*', kind: 'object', name: 'Framed object', sub: framed ? nameOf(D, framed) : 'Pick an object first', key: '.', disabled: !o.framed, sep: false })
  rows.push({ v: turntableView(0), kind: 'turntable', name: 'Turntable', sub: '8 stills around the scene', key: '', disabled: false, sep: false })
  if (o.agent) rows.push({ v: '@agent', kind: 'agent', name: `${o.agent.name}’s view`, sub: `Follows ${o.agent.name}: now ${viewName(D, o.agent.view)}`, key: '', disabled: false, sep: true })
  return rows
}

/** Your view and where Esc goes back to. */
export interface ViewNav {
  view: string | null
  back: string[]
}

/** The key hint on the picture, in order. */
export const KEY_HINT: readonly { key: string; label: string }[] = [
  { key: 'Home', label: 'Scene' },
  { key: '0', label: 'Camera' },
  { key: '.', label: 'Frame' },
  { key: 'Esc', label: 'Back' },
]

/** Go to a view: the one you leave goes on the back stack (unless it's the same). */
export function goView(nav: ViewNav, cur: string, v: string): ViewNav {
  return { view: v, back: cur !== v ? [...nav.back, cur] : nav.back }
}

/** A key on the picture: the new nav, or null when the key does nothing here (so it isn't swallowed). */
export function viewKey(nav: ViewNav, key: string, o: { cur: string; camera: string | null; framed: string | null }): ViewNav | null {
  if (key === 'Home') return goView(nav, o.cur, SCENE_VIEW)
  if (key === '0') return o.camera ? goView(nav, o.cur, o.camera) : null
  if (key === '.') return o.framed ? goView(nav, o.cur, o.framed) : null
  if (key === 'Escape') {
    if (!nav.back.length) return null
    return { view: nav.back[nav.back.length - 1], back: nav.back.slice(0, -1) }
  }
  return null
}

/** A turntable view at another frame (thumbnails show frame 1); any other view as it is. */
export const stillView = (v: string, frame: number): string => (isTurntable(v) ? turntableView(Math.max(0, Math.min(7, frame | 0))) : v)
