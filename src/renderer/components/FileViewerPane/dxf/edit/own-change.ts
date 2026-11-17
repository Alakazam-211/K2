// Your own landed edit as a change the highlight shows: the result's
// `cad.change/1` leaves who and which version to the result itself, so
// they're filled in here: the version the call made, the one before it,
// and you as the author (outlined in your own accent, never a presence
// colour). A dry run, a refusal or a call that changed nothing has no change.

import { parseCadChange, type CadChange } from '../cad-change'
import { SELF_PID } from '../cad-change-marks'
import type { ToolResult } from './tool-result'

export function ownChange(r: ToolResult): CadChange | null {
  if (!r.ok || r.dryRun || r.after === null || !r.data || typeof r.data !== 'object') return null
  const data = r.data as Record<string, unknown>
  return asOwn(data, r.after - 1, r.after, typeof data.verb === 'string' ? data.verb : r.verb)
}

/** A change you made (a verb's, or an undo's), as yours: one version, by you. */
export function asOwn(data: Record<string, unknown>, from: number, to: number, verb: string): CadChange | null {
  const rows = Array.isArray(data.entities) ? data.entities : []
  return parseCadChange({
    ...data,
    from,
    to,
    versions: [{ version: to, by: { pid: SELF_PID, name: 'You', kind: 'person' }, verb }],
    entities: rows.map((e) => (e && typeof e === 'object' ? { ...e, by: SELF_PID, version: to } : e)),
  })
}
