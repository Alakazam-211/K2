// The edit's words, in plain language, never a code:
//   - why an entity can't be changed here (the model's reason, or one
//     read from its kind and layer);
//   - why a whole drawing is view only (DWG, binary, before DXF 2000, no handles);
//   - each refusal the CAD pack and its kit can give, led by what didn't
//     happen ("Not moved."). Some also refresh the drawing (it changed
//     under you), and some turn editing off for the file.

import { namesOf } from '../cad-change-words'
import { NO_DIGEST } from './edit-flow'
import { NO_ROUTE, NOT_SENT } from './edit-source'
import type { ToolResult } from './tool-result'

/** Why an entity can't be changed, by the model's reason word. */
export const WHY_WORDS = {
  hatch: 'Hatches can’t be changed here yet.',
  dimension: 'Dimensions can’t be changed here yet.',
  leader: 'Leaders can’t be changed here yet.',
  viewport: 'Viewports can’t be changed here yet.',
  acis: '3D solids can’t be changed here.',
  image: 'Images and underlays can’t be changed here yet.',
  proxy: 'Made by another app. Change it in that app.',
  unknown: 'K2 can’t change this kind of item yet.',
  xref: 'Part of a linked drawing. Change it in that drawing.',
  block: 'Inside a block. Edit the block in your CAD app.',
  assoc: 'Grouped, or has a dimension attached. It can’t be moved here yet.',
  ocs: 'Drawn on a tilted plane. It can’t be changed here yet.',
  minted: 'Has no handle. Save the drawing again in your CAD app.',
  dynamic: 'Part of a dynamic block’s shape. Change the block in your CAD app.',
} as const

export type WhyKey = keyof typeof WHY_WORDS | 'locked'

export function whyWords(why: string, layer?: string | null): string {
  if (why === 'locked') return `On ${layer || 'a layer'}, which is locked. Unlock it in your CAD app first.`
  return (WHY_WORDS as Record<string, string>)[why] ?? WHY_WORDS.unknown
}

/** Whole-drawing reasons: the header line says these. */
export const OFF_WORDS = {
  dwg: 'View only: K2 can’t change DWG drawings yet.',
  binary: 'View only: this is a binary DXF. Save it as ASCII DXF in your CAD app to change it here.',
  old: 'View only: saved before DXF 2000. Save it as DXF 2000 or later in your CAD app to change it here.',
  handles: 'View only: this drawing is missing handles. Save it again in your CAD app to change it here.',
} as const

type HeaderLike = { format?: unknown; acadver?: unknown; binary?: unknown; handles?: unknown }

/** Why the drawing is view only, or null when the verbs can change it. */
export function viewOnlyWhy(header: HeaderLike, rows: readonly { h?: unknown }[]): string | null {
  const format = typeof header.format === 'string' ? header.format.toLowerCase() : 'dxf'
  if (format === 'dwg') return OFF_WORDS.dwg
  if (header.binary === true || format.includes('binary')) return OFF_WORDS.binary
  // AC1015 is DXF 2000; the versions before it sort below it.
  if (typeof header.acadver === 'string' && /^AC\d{4}$/.test(header.acadver) && header.acadver < 'AC1015') return OFF_WORDS.old
  const h = header.handles as { missing?: unknown; duplicates?: unknown } | null | undefined
  if (h && typeof h === 'object' && (Number(h.missing) > 0 || Number(h.duplicates) > 0)) return OFF_WORDS.handles
  if (rows.some((r) => typeof r.h !== 'string' || !/^@h:[0-9A-F]+$/i.test(r.h))) return OFF_WORDS.handles
  return null
}

export type RefusalWords = {
  words: string
  /** The drawing changed under you: read it again. */
  refresh: boolean
  /** The drawing can't be changed here at all: editing turns off, with these words. */
  off: string | null
}

export type RefusalContext = {
  verb: 'move' | 'relabel'
  /** The layer of an address in the drawing, for a locked layer's name. */
  layerOf?: (address: string) => string | null
}

const name = (d: Record<string, unknown>): string | null => {
  for (const k of ['by', 'holder', 'name']) {
    const v = d[k]
    if (typeof v === 'string' && v) return v
    if (v && typeof v === 'object' && typeof (v as { name?: unknown }).name === 'string') return (v as { name: string }).name
  }
  const holders = d.holders
  if (Array.isArray(holders)) {
    const names = holders.map((x) => (typeof x === 'string' ? x : (x as { name?: unknown })?.name)).filter((x): x is string => typeof x === 'string')
    if (names.length) return namesOf(names)
  }
  return null
}

function valueWords(message: string): string {
  if (/line break|control/i.test(message)) return 'The value has to be one line, with no control characters.'
  if (/\\U\+/i.test(message)) return 'The value can’t contain \\U+, which the file reads as a code.'
  if (/code page|character/i.test(message)) return 'This drawing’s text can’t hold one of the value’s characters.'
  if (/255|long/i.test(message)) return 'The value is longer than the 255 bytes a label can hold.'
  return 'The value can’t be written in this drawing.'
}

/** The reason word in a "not changed by the edit verbs (why)" message. */
const whyOf = (message: string): string => /\(([a-z_]+)\)\s*$/i.exec(message.trim())?.[1]?.toLowerCase() ?? 'unknown'

function reason(id: string, e: { message: string; details: Record<string, unknown> }, ctx: RefusalContext): Omit<RefusalWords, 'words'> & { why: string } {
  const plain = (why: string, refresh = false, off: string | null = null) => ({ why, refresh, off })
  const address = /@h:[0-9A-F]+/i.exec(e.message)?.[0]
  switch (id) {
    case 'DXF-E001':
      return plain('This is a binary DXF. Save it as ASCII DXF in your CAD app to change it here.', false, OFF_WORDS.binary)
    case 'DXF-E002':
      return plain('This file doesn’t read as a DXF drawing.')
    case 'DXF-E003':
      return plain('This drawing is too large to change here.')
    case 'DXF-E010':
    case 'CAD-E010':
      return plain('It isn’t in the drawing anymore. The drawing has been refreshed.', true)
    case 'DXF-E011':
    case 'CAD-E011':
      return plain('That label appears more than once on this block. Pick which one to change.')
    case 'DXF-E020':
      return plain('That layer isn’t in the drawing.')
    case 'DXF-E021':
      return plain(whyWords(whyOf(e.message)))
    case 'DXF-E022':
      return plain('The drawing’s units changed. The drawing has been refreshed; try again.', true)
    case 'DXF-E027':
      return plain('Something else in the drawing points at it. Remove that link in your CAD app first.')
    case 'DXF-E040':
      return plain('This drawing was saved before DXF 2000. Save it as DXF 2000 or later in your CAD app to change it here.', false, OFF_WORDS.old)
    case 'DXF-E041':
      return plain('This drawing is missing handles. Save it again in your CAD app to change it here.', false, OFF_WORDS.handles)
    case 'DXF-E042':
      return plain(WHY_WORDS.xref)
    case 'DXF-E043':
      return plain(WHY_WORDS.proxy)
    case 'DXF-E044':
      return plain(WHY_WORDS.dynamic)
    case 'DXF-E045': {
      const layer = address ? ctx.layerOf?.(address) : null
      return plain(layer ? `${layer} is locked. Unlock it in your CAD app, then try again.` : 'Its layer is locked. Unlock it in your CAD app, then try again.')
    }
    case 'DXF-E046':
      return plain(WHY_WORDS.block)
    case 'DXF-E047':
      return plain('This block doesn’t have that label anymore. The drawing has been refreshed.', true)
    case 'DXF-E048':
      return plain('Your CAD app fills in this label from a field. Change the field’s source there.')
    case 'DXF-E049':
      return plain('This label has more than one line. Change it in your CAD app.')
    case 'DXF-E050':
      return plain('That’s more than 10,000 items at once. Select fewer and try again.')
    case 'DXF-E053':
      return plain('The drawing doesn’t have that linetype or text style.')
    case 'DXF-E054':
      return plain(valueWords(e.message))
    case 'DXF-E090':
      return plain('K2’s check of this change failed, so nothing was written. Please report it with the file.')
    case 'CAD-E004':
    case 'MPC-E122':
      return plain('K2 sent a change the drawing couldn’t read. Nothing was changed.')
    case 'MPC-E110':
      return plain('The drawing changed while you were working. It has been refreshed; try again.', true)
    case 'MPC-E301':
      return plain(`${name(e.details) ?? 'Someone'} is working on these.`)
    case 'MPC-E701':
      return plain(`${name(e.details) ?? 'Someone'} has this file locked for editing. Try again when they’re done.`)
    case 'MPC-E713':
      return plain('The drawing is still being read. Try again in a moment.')
    case 'MPC-E719':
      return plain('The file was saved by another app. Look again before changing it.', true)
    case 'MPC-E723':
      return plain('Someone changed this since you looked. The drawing has been refreshed.', true)
    case 'MPC-E724':
      return plain('K2 can’t be sure which items these are. Nothing was changed.')
    case 'MPC-E725':
    case 'MPC-E726':
    case 'MPC-E727':
    case 'MPC-E728':
      return plain('K2’s own check of this change failed. Nothing was changed.')
    case 'MPC-E729':
      return plain('The drawing changed between the check and the change. Nothing was changed; try again.', true)
    case 'MPC-E823':
      return plain('The drawing tools turned this change down. Nothing was changed.')
    case NO_ROUTE:
      return plain('Changing drawings isn’t on this server yet.')
    case NOT_SENT:
      return plain('K2 couldn’t reach the server. Nothing was changed.')
    case NO_DIGEST:
      return plain('K2 couldn’t check this change first, so it wasn’t made.')
    default:
      return plain('K2 couldn’t make this change. Nothing was changed.')
  }
}

export function refusalWords(r: ToolResult, ctx: RefusalContext): RefusalWords {
  const e = r.error ?? { id: '', message: '', next: '', details: {} }
  const { why, refresh, off } = reason(e.id, e, ctx)
  return { words: `${ctx.verb === 'move' ? 'Not moved.' : 'Not changed.'} ${why}`, refresh, off }
}
