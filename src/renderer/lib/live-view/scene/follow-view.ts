// Following on the scene (S5), the picture's side of it; the shell keeps
// whom you follow (lib/live-view/shared/follow.ts). Pure:
//   - the sample's scripted follows: a step that presses Follow, follows
//     someone by name, stops, or looks away by picking a view;
//   - when the one you follow stops (their turn ends, or they let go of
//     their hold): following ends with the editors' and the timeline's
//     words, "Atlas stopped. Stopped following." with Back to my place and
//     Follow again; "Sam left the file. Stopped following." when they go;
//   - what the say row points at: the view on screen, as K2 Render's view
//     address (`@scene:main`, `@cam:<id>`, `@obj:<id>`, `@turntable:<n>`);
//   - who looks through another view than yours (the chips by the picker);
//   - how two views split the stage (live-scene-v2-notes.md, v2.1: side by
//     side only when each half is at least 4:3, else one above the other);
//   - the turntable's frame under a drag across its strip.

import { isLive, peerView } from './panel'
import type { SimState } from './sim'
import type { Doc } from './types'
import { SCENE_VIEW } from './view-address'
import { kindOf, sameView, viewName } from './views'

export type ScriptedFollow = { type: 'follow'; who: string } | { type: 'away'; who: string } | { type: 'stop' }

/** What the sample's step scripts for Follow (the mock's `watch` and `view` events). */
export function scriptedFollow(S: SimState): ScriptedFollow | null {
  const evs = S.group?.events ?? []
  const w = evs.find((e) => e.kind === 'watch')
  if (w) return w.v === false ? { type: 'stop' } : S.watch ? { type: 'follow', who: S.watch } : null
  if (evs.some((e) => e.kind === 'view') && S.lastFollow) return { type: 'away', who: S.lastFollow }
  return null
}

/** The one you follow stopped at this step: their turn ended, or they let go of their hold. */
export function stoppedAt(S: SimState, who: string): boolean {
  return S.log.some((e) => e.i === S.i && e.actor === who && (e.kind === 'turn_end' || (e.kind === 'claim_end' && e.how === 'released')))
}

/** Following ended: they stopped, or they left the file. */
export interface FollowEnd {
  who: string
  name: string
  why: 'stopped' | 'left'
}

/** The finished line and its two ways on (no Follow again once they've left). */
export function followEndWords(f: FollowEnd): { text: string; back: string; again: string | null } {
  return f.why === 'left'
    ? { text: `${f.name} left the file. Stopped following.`, back: 'Back to my place', again: null }
    : { text: `${f.name} stopped. Stopped following.`, back: 'Back to my place', again: 'Follow again' }
}

/** What a message points at: the view on screen, in words, and its address. */
export function sayAtOf(D: Doc, view: string): { label: string; title: string; address: string } {
  const label = viewName(D, view)
  const what = view === SCENE_VIEW ? 'the whole scene' : kindOf(view) === 'turntable' ? 'the turntable' : label
  return { label, title: `What you’re looking at: ${what}`, address: view }
}

export interface OtherView {
  id: string
  name: string
  kind: string
  colour: string
  away: boolean
  view: string
  /** Their view in words ("Camera 2", "Table, framed"). */
  label: string
}

/** Everyone else here whose view isn't the one on screen. */
export function othersOnViews(S: SimState, view: string): OtherView[] {
  if (!isLive(S)) return []
  const out: OtherView[] = []
  for (const p of Object.values(S.peers)) {
    if (p.handle === S.self) continue
    const v = peerView(S, p.handle)
    if (!v || v === '@obj:*' || sameView(v, view)) continue
    out.push({ id: p.handle, name: p.name, kind: p.kind, colour: p.colour, away: !!p.away, view: v, label: viewName(S.D, v) })
  }
  return out
}

export interface PaneRect {
  x: number
  y: number
  w: number
  h: number
}

/** One view fills the stage; two sit side by side when each half is at least 4:3, else one above the other. */
export function paneRects(w: number, h: number, layout: 1 | 2): { split: 'one' | 'side' | 'stacked'; panes: PaneRect[] } {
  if (layout === 1) return { split: 'one', panes: [{ x: 0, y: 0, w, h }] }
  const half = Math.round(w / 2)
  if (h > 0 && half / h >= 4 / 3) return { split: 'side', panes: [{ x: 0, y: 0, w: half, h }, { x: half, y: 0, w: w - half, h }] }
  const top = Math.round(h / 2)
  return { split: 'stacked', panes: [{ x: 0, y: 0, w, h: top }, { x: 0, y: top, w, h: h - top }] }
}

export const TURNTABLE_FRAMES = 8

/** The turntable frame under a point across its strip. */
export const turntableFrameAt = (x: number, width: number): number => Math.max(0, Math.min(TURNTABLE_FRAMES - 1, Math.floor((x / Math.max(1, width)) * TURNTABLE_FRAMES)))
