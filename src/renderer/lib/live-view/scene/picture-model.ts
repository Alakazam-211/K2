// The sample's state as the picture reads it: the status K2 Render's pushes
// fold to, the view on screen, the doc the still was drawn from, the words'
// context, and the details popover's rows (plain rows, then the raw facts
// once, for an agent). Pure.

import { UNITS, collTitle, nameOf, objOf } from './fold'
import { sceneTree } from './tree'
import { refusalText } from './wording'
import { agentId, agentTarget, type SimState } from './sim'
import { approxOf, myPick, peerView, target, viewNow } from './panel'
import type { ObjectBounds, PictureStatus, StillMeta } from './picture-source'
import type { PictureContext } from './picture-words'
import { pictureFreshness } from './picture-words'
import { cameraLabel, kindOf, mainCamera, objectView, viewName } from './views'
import type { Doc } from './types'

/** The sim's picture record as a PictureStatus. */
export function pictureStatus(S: SimState): PictureStatus {
  const v = S.pv
  return {
    shown: S.tier === 'none' ? null : v.shown,
    want: v.want,
    state: v.state,
    renderMs: v.ms,
    reusedFrom: v.reused,
    withoutModifiers: v.without_modifiers,
    reason: v.reason,
    landed: !!v.landed,
    recovered: !!v.recovered,
    retrying: !!v.retrying,
    again: !!v.again,
  }
}

/** The doc the still on screen was drawn from (an older rev while the new one draws). */
export const stillDoc = (S: SimState): Doc => (S.pv.shown != null && S.revDocs[S.pv.shown]) || S.D

/** The view on screen: yours, else the scenario's (whom you follow's comes in S5). */
export const viewOnScreen = (S: SimState, local: { view: string | null; pick: string | null | undefined }): string => viewNow(S, { view: local.view, pick: local.pick ?? undefined })

/** The view "Framed object" and "." go to: your pick, else what the agent is on, framed. */
export function framedOf(S: SimState, pick: string | null | undefined): string | null {
  const a = myPick(S, pick) || agentTarget(S)
  return a && S.D?.props[a] ? objectView(S.D, a) : null
}

/** The agent's view (K2 Render's address), while it has one here. */
export function agentView(S: SimState): { id: string; name: string; view: string } | null {
  const a = agentId(S)
  const p = a ? S.peers[a] : null
  const view = a ? peerView(S, a) : null
  return p && view && view !== '@obj:*' && S.tier === 'live' && !S.loading && S.conn === 'ok' ? { id: p.handle, name: p.name, view } : null
}

const hasObjects = (D: Doc | null): boolean => !!D && (D.lists[UNITS] ?? []).length > 0

/** The picture words' context. */
export function pictureContext(S: SimState, pick: string | null | undefined): PictureContext {
  const t = target(S, pick)
  const ap = t && S.D.props[t] ? approxOf(S.D, t) : null
  const ag = agentId(S)
  const cam = mainCamera(S.D)
  return {
    loading: !!S.loading,
    tier: S.tier,
    conn: S.conn,
    objects: hasObjects(S.D),
    markers: !!S.D?.pinned && (S.D.lists[UNITS] ?? []).some((a) => objOf(S.D, a).kind === 'empty'),
    approx: ap && t ? { name: nameOf(S.D, t), driven: ap === 'driven' } : null,
    camera: cam ? cameraLabel(S.D, cam) : null,
    askAgent: ag && S.peers[ag] && S.tier === 'live' && S.conn === 'ok' ? S.peers[ag].name : null,
  }
}

/** "Looking at" in the details: the whole scene, a camera in the file, an object framed, the turntable. */
function lookingAt(D: Doc, v: string): string {
  const k = kindOf(v)
  if (k === 'camera') return `${cameraLabel(D, v)}, a camera in the file`
  if (k === 'object') return viewName(D, v)
  return k === 'turntable' ? 'the turntable' : 'the whole scene'
}

export interface Details {
  rows: { label: string; value: string }[]
  /** The raw facts, one line, for "Copy for an agent": revs, addresses, codes. */
  forAgents: string
}

/** The details popover: plain rows, then the raw facts once (copied, never shown). */
export function pictureDetails(S: SimState, o: { view: string; pick: string | null | undefined; size: { width: number; height: number } | null; bounds?: (D: Doc, a: string) => ObjectBounds | null; meta?: StillMeta | null }): Details {
  const pv = S.pv
  const rows: Details['rows'] = []
  const fresh = pictureFreshness(pictureStatus(S))
  if (fresh) rows.push({ label: 'Picture', value: fresh })
  if (o.size && pv.shown != null) rows.push({ label: 'Size', value: `${o.size.width} × ${o.size.height}${pv.ms != null ? `, drawn in ${pv.ms} ms` : ''}` })
  rows.push({ label: 'Looking at', value: lookingAt(S.D, o.view) })
  if (pv.without_modifiers) rows.push({ label: 'Modifiers', value: 'Not shown in this picture' })
  const t = target(S, o.pick)
  if (t && S.D.props[t]) {
    rows.push({ label: 'Selected', value: nameOf(S.D, t) })
    // Driven by the place op, else the listing's approx (a bool) and its approx_reason.
    const b = o.bounds?.(S.D, t) ?? null
    const ap = approxOf(S.D, t) === 'driven' ? 'driven' : b?.approx === true ? (b.approx_reason ?? 'modifiers') : null
    if (ap === 'driven') rows.push({ label: 'Its box', value: 'Approximate: animation moves it, so the box shows where it is saved' })
    else if (ap) rows.push({ label: 'Its box', value: 'Approximate: a modifier adds parts the box leaves out' })
  }
  // What the picture left out, quietly, only when there is any.
  const m = o.meta
  const plural = (n: number, one: string, many: string): string => `${n} ${n === 1 ? one : many}`
  if (m?.objects_hidden) rows.push({ label: 'Left out', value: plural(m.objects_hidden, 'hidden object', 'hidden objects') })
  if (m?.objects_not_evaluated) rows.push({ label: 'Left out', value: plural(m.objects_not_evaluated, 'object that couldn’t be evaluated', 'objects that couldn’t be evaluated') })
  if (S.callout) rows.push({ label: 'Refusal', value: refusalText(S.callout.r, S.D).line })
  for (const r of sceneTree(S.D).refused) {
    const why = r.why === 'unknown' ? 'it isn’t in this file' : r.why === 'cycle' ? 'it would put the collection inside itself' : 'it nests too deep'
    rows.push({ label: 'Outliner', value: `${collTitle(S.D, r.addr)}’s parent can’t be used (${why}), so ${collTitle(S.D, r.addr)} is shown at the top level` })
  }
  if (S.notice?.copy) rows.push({ label: 'The copy', value: S.notice.copy.path })
  const refs = [...new Set([o.view, t, S.callout?.r.unit].filter((x): x is string => !!x))]
  const left = m ? [...(m.objects_hidden ? [`objects_hidden ${m.objects_hidden}`] : []), ...(m.objects_not_evaluated ? [`objects_not_evaluated ${m.objects_not_evaluated}`] : [])] : []
  const facts = [`change ${S.D.rev}`, ...(pv.shown != null ? [`picture of change ${pv.shown}`] : []), ...refs, ...(S.callout ? [S.callout.r.code] : []), ...(S.notice?.code ? [S.notice.code] : []), ...left]
  return { rows, forAgents: facts.join(' · ') }
}
