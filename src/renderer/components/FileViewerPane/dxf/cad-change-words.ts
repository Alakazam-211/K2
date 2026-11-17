// The one-line note a change shows: who changed what, in plain words.
//   - one version: "Atlas moved a line on A-WALL", "You moved 3 lines";
//   - a layer change, by name: "Sam froze A-FURN";
//   - several versions by one author: "Atlas made 3 changes";
//   - several authors: "Atlas and Sam made 4 changes" (no Undo);
//   - an outside save in the range: "Changed by an outside save" (no Undo).
// Undo reverts the note's versions, newest first, and only when one author
// made them all.

import type { DxfDrawing } from '../dxfModel'
import type { CadChange, CadEntityRow, CadVersion } from './cad-change'

// TODO: take the past tense from the CAD pack's verb list once its edit verbs
// land; until then, the verbs it reports today and a fallback by operation.
export const PAST_VERB: Record<string, string> = {
  'entity move': 'moved',
  'entity add': 'added',
  'entity delete': 'deleted',
  'entity remove': 'deleted',
  'entity rotate': 'rotated',
  'entity scale': 'scaled',
  'entity copy': 'copied',
  'entity offset': 'offset',
  'entity set': 'changed',
  'text set': 'edited',
  'attrib set': 'changed a label in',
  'layer set': 'changed',
  'layer add': 'added',
  'block set': 'changed',
  revert: 'undid',
  undo: 'undid',
}

const BY_OP: Record<string, string> = { added: 'added', removed: 'deleted', changed: 'changed' }

/** What an entity kind is called in a sentence, with its article. */
const KIND_WORDS: Record<string, string> = {
  LINE: 'a line',
  CIRCLE: 'a circle',
  ARC: 'an arc',
  ELLIPSE: 'an ellipse',
  LWPOLYLINE: 'a polyline',
  POLYLINE: 'a polyline',
  SPLINE: 'a spline',
  TEXT: 'text',
  MTEXT: 'text',
  INSERT: 'a block',
  DIMENSION: 'a dimension',
  HATCH: 'a hatch',
  SOLID: 'a solid',
  POINT: 'a point',
  '3DFACE': 'a face',
  LEADER: 'a leader',
  MLEADER: 'a leader',
}

/** The same, several of one kind: "3 lines". */
const KIND_PLURAL: Record<string, string> = {
  LINE: 'lines',
  CIRCLE: 'circles',
  ARC: 'arcs',
  ELLIPSE: 'ellipses',
  LWPOLYLINE: 'polylines',
  POLYLINE: 'polylines',
  SPLINE: 'splines',
  INSERT: 'blocks',
  DIMENSION: 'dimensions',
  HATCH: 'hatches',
  SOLID: 'solids',
  POINT: 'points',
  '3DFACE': 'faces',
  LEADER: 'leaders',
  MLEADER: 'leaders',
}

/** "3 lines", "2 lines and 1 block"; past three kinds, "6 entities". */
function countsOf(rows: readonly CadEntityRow[]): string {
  const by = new Map<string, number>()
  for (const r of rows) {
    const k = KIND_PLURAL[r.kind] && KIND_WORDS[r.kind] ? r.kind : '?'
    by.set(k, (by.get(k) ?? 0) + 1)
  }
  if (by.has('?') || by.size > 3) return `${rows.length} entities`
  const parts = [...by].sort((a, b) => b[1] - a[1]).map(([k, n]) => `${n} ${n === 1 ? KIND_WORDS[k].replace(/^an? /, '') : KIND_PLURAL[k]}`)
  return parts.length === 1 ? parts[0] : `${parts.slice(0, -1).join(', ')} and ${parts[parts.length - 1]}`
}

export type ChangeNote = {
  text: string
  /** The versions Undo reverts, newest first; null when there is no Undo. */
  undo: number[] | null
}

/** "Atlas", "Atlas and Sam", "Atlas, Sam and Kai". */
export function namesOf(names: readonly string[]): string {
  if (names.length <= 1) return names[0] ?? 'Someone'
  return `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}`
}

/** A layer row's sentence, from what changed and the layer's state now. */
function layerWords(fields: readonly string[], name: string, state: { frozen: boolean; off: boolean } | undefined): string {
  if (fields.includes('frozen')) return `${state?.frozen === false ? 'thawed' : 'froze'} ${name}`
  if (fields.some((f) => f === 'on' || f === 'off' || f === 'visible')) return `turned ${state?.off === false ? 'on' : 'off'} ${name}`
  if (fields.includes('color') || fields.includes('rgb')) return `recoloured ${name}`
  return `changed layer ${name}`
}

function oneVersion(c: CadChange, v: CadVersion | undefined, drawing: Pick<DxfDrawing, 'index'>): string {
  const name = v?.by.name ?? 'Someone'
  // Rows inside a block definition are the block's change, told by the block row.
  const rows = c.entities.filter((r) => !r.owner.startsWith('@h:'))
  const n = rows.length + c.entitiesMore
  if (n > 0) {
    const verb = (v && PAST_VERB[v.verb]) || BY_OP[rows[0]?.op ?? 'changed'] || 'changed'
    const layers = [...new Set(rows.map((r) => r.layer))]
    const on = c.entitiesMore === 0 && layers.length === 1 && layers[0] ? ` on ${layers[0]}` : ''
    if (n === 1 && rows.length === 1) return `${name} ${verb} ${KIND_WORDS[rows[0].kind] ?? 'an entity'}${on}`
    return `${name} ${verb} ${c.entitiesMore === 0 ? countsOf(rows) : `${n} entities`}${on}`
  }
  if (c.blocks.length === 1) return `${name} changed block ${c.blocks[0].name}`
  if (c.blocks.length > 1) return `${name} changed ${c.blocks.length} blocks`
  if (c.tables.length === 1) return `${name} ${layerWords(c.tables[0].fields, c.tables[0].name, drawing.index.layerState[c.tables[0].name])}`
  if (c.tables.length > 1) return `${name} changed ${c.tables.length} layers`
  return `${name} ${(v && PAST_VERB[v.verb]) || 'changed'} the drawing`
}

export function changeNote(c: CadChange, drawing: Pick<DxfDrawing, 'index'>): ChangeNote {
  if (c.pairing !== 'kept') return { text: 'Changed by an outside save', undo: null }
  const pids = [...new Set(c.versions.map((v) => v.by.pid))]
  const undo = pids.length === 1 ? c.versions.map((v) => v.version).sort((a, b) => b - a) : null
  if (c.versions.length <= 1) return { text: oneVersion(c, c.versions[0], drawing), undo }
  const names = [...new Set(c.versions.map((v) => v.by.name))]
  return { text: `${namesOf(names)} made ${c.versions.length} changes`, undo }
}
