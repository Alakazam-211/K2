// Inserting a block: which of the drawing's blocks can be placed, the labels
// a new reference asks for, and the verb's argv.
//   - Insertable: the header's blocks, leaving out linked drawings (xrefs),
//     layouts' own blocks (model and paper space), the ones a CAD app makes
//     for itself (anonymous: a name starting "*") and dynamic blocks when
//     the model marks them; by name.
//   - Labels: every attribute definition that isn't constant, with its
//     default. When the model doesn't list a block's definitions, the tags
//     an existing reference of it carries, with no default (left empty, the
//     block's own default is used).
//   - The verb places the block's base point at the point given; each label
//     is sent once, by tag.

import type { ModelBlock, ModelHeader, ModelRow, XY } from '../model/types'
import { fmtCoord } from './edit-flow'

export function insertableBlocks(header: Pick<ModelHeader, 'blocks' | 'layouts' | 'model_space_block'>): ModelBlock[] {
  const spaces = new Set([header.model_space_block, ...header.layouts.flatMap((l) => [l.handle, l.space])].filter((h): h is string => !!h).map((h) => h.toUpperCase()))
  return header.blocks
    .filter((b) => !b.xref && !b.anon && !b.dynamic && !b.name.startsWith('*') && !spaces.has(b.handle.toUpperCase()))
    .sort((a, b) => a.name.localeCompare(b.name))
}

/** A label a new reference asks for: its tag, its default, and whether the default is known. */
export type InsertLabel = { tag: string; value: string; prompt: string | null; known: boolean }

export function insertLabels(block: ModelBlock, rows: readonly ModelRow[]): InsertLabel[] {
  if (Array.isArray(block.attdefs))
    return block.attdefs.filter((a) => !a.constant).map((a) => ({ tag: a.tag, value: a.default, prompt: a.prompt ?? null, known: true }))
  const ref = rows.find((r) => r.kind === 'INSERT' && r.geom?.block === block.handle && Array.isArray(r.geom.attribs))
  const attribs = (ref?.geom?.attribs ?? []) as { tag?: unknown; constant?: unknown }[]
  const tags = [...new Set(attribs.filter((a) => typeof a.tag === 'string' && a.constant !== true).map((a) => a.tag as string))]
  return tags.map((tag) => ({ tag, value: '', prompt: null, known: false }))
}

/** Place `block` (its handle) with its base point at `at`; `labels` as tag and value, each once. */
export const insertArgv = (block: string, at: XY, unit: string, labels: readonly (readonly [string, string])[] = []): string[] => [
  'block',
  'insert',
  '--block',
  block,
  '--at',
  `${fmtCoord(at[0])},${fmtCoord(at[1])}`,
  '--unit',
  unit,
  ...labels.flatMap(([tag, value]) => ['--attr', `${tag}=${value}`]),
]

/** The new reference an insert made: the change's first added row. */
export function insertedAddress(data: unknown): string | null {
  const rows = data && typeof data === 'object' ? (data as { entities?: unknown }).entities : null
  if (!Array.isArray(rows)) return null
  const r = rows.find((e) => e && typeof e === 'object' && (e as { op?: unknown }).op === 'added' && typeof (e as { address?: unknown }).address === 'string')
  return r ? (r as { address: string }).address : null
}
