// SAMPLE ONLY. The mock's sample data names views the way the mock did
// (`@scene`, `@cam:<object number>`, `@obj:<kit address>`,
// `@scene/turntable`, and `@obj:*` for "the framed object"), and its
// presence goldens keep them as pinned. The tab speaks K2 Render's
// addresses (view-address.ts), so the sample's views are read through this
// one adapter wherever they leave the simulation. A live source sends K2
// Render's addresses and needs none of it.

import { cameraView, objectView } from './views'
import { SCENE_VIEW, parseView, turntableView } from './view-address'
import type { Doc } from './types'

/** A view as the sample names it, as K2 Render names it. `@obj:*` stays (the picker's placeholder); null stays null. */
export function fromMockView(D: Doc | null, v: string | null | undefined, turntableFrame = 0): string | null {
  if (v == null) return null
  if (v === '@obj:*' || v === SCENE_VIEW || parseView(v)?.kind === 'turntable') return v
  if (v === '@scene') return SCENE_VIEW
  if (v.startsWith('@scene/turntable')) return turntableView(Math.max(0, Math.min(7, turntableFrame | 0)))
  const cam = /^@cam:(\d+)$/.exec(v)
  if (cam && D) return D.props[`@obj:${cam[1]}`] ? cameraView(D, `@obj:${cam[1]}`) : SCENE_VIEW
  if (/^@obj:\d+$/.test(v) && D) return D.props[v] ? objectView(D, v) : SCENE_VIEW
  return v
}
