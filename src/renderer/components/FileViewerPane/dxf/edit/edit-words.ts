// The edit's words, in plain language, never a code:
//   - why an entity can't be changed here (the model's reason, or one
//     read from its kind and layer);
//   - why a whole drawing is view only (DWG, binary, before DXF 2000, no handles);
//   - each refusal the CAD pack and its kit can give, led by what didn't
//     happen ("Not moved."). Some also refresh the drawing (it changed
//     under you), and some turn editing off for the file.

import { namesOf } from '../cad-change-words'
import { NO_DIGEST } from './edit-flow'
import { NO_ROUTE, NOT_SENT, WRITER_OFF, WRITER_OFF_WORDS } from './edit-source'
import type { ToolResult } from './tool-result'

/**
 * Why an entity can't be changed, by the drawing model's edit word (each
 * draw row's `edit`: null when the verbs change it). A row with one of
 * these has no Move, Rotate or Scale.
 */
export const WHY_WORDS = {
  'in-block': 'Part of a block’s definition. Edit the block in your CAD app.',
  hatch: 'Hatches can’t be changed here yet.',
  leader: 'Leaders can’t be changed here yet.',
  'own-frame': 'This is drawn in its own frame. Change it in your CAD app.',
  unknown: 'K2 doesn’t know this kind yet.',
  image: 'Images can’t be changed here yet.',
  dimension: 'Dimensions can’t be changed here yet.',
  solid3d: '3D solids can’t be changed here.',
  // The pack's rarer words.
  viewport: 'Viewports can’t be changed here yet.',
  proxy: 'It was made by another app. Change it in that app.',
  xref: 'It’s part of a linked drawing. Change it in that drawing.',
  dynamic: 'It’s part of a dynamic block’s shape. Change the block in your CAD app.',
  reactors: 'It’s grouped or has a dimension attached, so it can’t be changed here yet.',
  embedded: 'It holds an object from another app, so it can’t be changed here yet.',
  'multiline-attribute': 'It has a label with more than one line. Change it in your CAD app.',
  'no-handle': 'It has no handle. Save the drawing again in your CAD app.',
  version: 'The drawing was saved before DXF 2000. Save it as DXF 2000 or later in your CAD app.',
} as const

/** A reason word as given (any case or spacing). */
export const reasonKey = (why: string): string => why.trim().toLowerCase()

export type WhyKey = keyof typeof WHY_WORDS | 'locked-layer'

export function whyWords(why: string, layer?: string | null): string {
  why = reasonKey(why)
  if (why === 'locked-layer') return `It’s on ${layer || 'a layer'}, which is locked. Unlock it in your CAD app first.`
  return (WHY_WORDS as Record<string, string>)[why] ?? WHY_WORDS.unknown
}

/** Whole-drawing reasons: the header line says these. */
export const OFF_WORDS = {
  dwg: 'View only: K2 can’t change DWG drawings yet. Save it as DXF in your CAD app to change it here.',
  binary: 'View only: this is a binary DXF. Save it as ASCII DXF in your CAD app to change it here.',
  old: 'View only: saved before DXF 2000. Save it as DXF 2000 or later in your CAD app to change it here.',
  handles: 'View only: this drawing is missing handles. Save it again in your CAD app to change it here.',
} as const

type HeaderLike = { format?: unknown; acadver?: unknown; binary?: unknown; handles?: unknown; writer?: unknown; read_only_reason?: unknown }

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
  // The host says this file's edits can't be saved: its words, as given.
  if (header.writer === false) return (typeof header.read_only_reason === 'string' && header.read_only_reason.trim()) || WRITER_OFF_WORDS
  return null
}

/** Every check of a change that fails on K2's side reads the same. */
const CHECK_FAILED = 'K2 couldn’t check this change. Please report it with the file.'

export type RefusalWords = {
  words: string
  /** The drawing changed under you: read it again. */
  refresh: boolean
  /** The drawing can't be changed here at all: editing turns off, with these words. */
  off: string | null
}

/** What an edit does: each refusal leads with what didn't happen. */
export type EditVerb = 'move' | 'rotate' | 'scale' | 'relabel' | 'text' | 'insert'

const NOT_DONE: Record<EditVerb, string> = {
  move: 'Not moved.',
  rotate: 'Not rotated.',
  scale: 'Not scaled.',
  relabel: 'Not changed.',
  text: 'Not changed.',
  insert: 'Not inserted.',
}

export type RefusalContext = {
  verb: EditVerb
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

function valueWords(message: string, text: boolean): string {
  const noun = text ? 'Text' : 'A label'
  if (/line break|control/i.test(message)) return `${noun} must be one line, with no hidden characters.`
  if (/\\U\+/i.test(message)) return `${noun} can’t contain “\\U+”. The file would read it as a character code.`
  if (/code page|character/i.test(message)) return 'One of its characters can’t be saved in this drawing.'
  if (/255|long/i.test(message)) return `It’s longer than the 255 bytes ${text ? 'a line of text' : 'a label'} can hold.`
  return 'It can’t be saved in this drawing.'
}

/** The reason word in a "not changed by the edit verbs (why)" message. */
const whyOf = (message: string): string => /\(([^()]+)\)\s*$/.exec(message.trim())?.[1] ?? 'unknown'

function reason(id: string, e: { message: string; details: Record<string, unknown> }, ctx: RefusalContext): Omit<RefusalWords, 'words'> & { why: string } {
  const plain = (why: string, refresh = false, off: string | null = null) => ({ why, refresh, off })
  const address = /@h:[0-9A-F]+/i.exec(e.message)?.[0]
  switch (id) {
    case 'DXF-E001':
      return plain('This is a binary DXF. Save it as ASCII DXF in your CAD app to change it here.', false, OFF_WORDS.binary)
    case 'DXF-E002':
      return plain('K2 can’t read this file as a DXF drawing.')
    case 'DXF-E003':
      return plain('This drawing is too large to change here.')
    case 'DXF-E010':
    case 'CAD-E010':
      return plain('That item isn’t in the drawing anymore. The drawing has been refreshed.', true)
    case 'DXF-E011':
    case 'CAD-E011':
      return plain('That label appears more than once on this block. Pick which one to change.')
    case 'DXF-E020':
      return plain('That layer isn’t in the drawing anymore. The drawing has been refreshed.', true)
    case 'DXF-E021':
      // Text set takes single-line text only.
      if (ctx.verb === 'text') return plain('Only single-line text can be changed here. Change multi-line text in your CAD app.')
      return plain(whyWords(whyOf(e.message)))
    case 'DXF-E022':
      return plain('The drawing’s units changed. The drawing has been refreshed; try again.', true)
    case 'DXF-E027':
      return plain('Another item, such as a dimension, is linked to it. Remove that link in your CAD app first.')
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
      return plain(WHY_WORDS['in-block'])
    case 'DXF-E047':
      return plain('This block doesn’t have that label anymore. The drawing has been refreshed.', true)
    case 'DXF-E048':
      return plain('Your CAD app fills in this label from a field. Change the field’s source there.')
    case 'DXF-E049':
      return plain('This label has more than one line. Change it in your CAD app.')
    case 'DXF-E050':
      return plain('That’s more than 10,000 items at once. Select fewer and try again.')
    case 'DXF-E053':
      return plain('It uses a linetype or text style the drawing doesn’t have.')
    case 'DXF-E054':
      return plain(valueWords(e.message, ctx.verb === 'text'))
    case 'DXF-E090':
      return plain(CHECK_FAILED)
    case 'CAD-E004':
    case 'MPC-E122':
      return plain('Something went wrong sending this change. Please report it with the file.')
    case 'MPC-E110':
      return plain('The drawing changed while you were working. It has been refreshed; try again.', true)
    case 'MPC-E301':
      return plain(`${name(e.details) ?? 'Someone'} is working on these. Try again when they’re done.`)
    case 'MPC-E701':
      return plain(`${name(e.details) ?? 'Someone'} has this file locked for editing. Try again when they’re done.`)
    case 'MPC-E713':
      return plain('The drawing is still being read. Try again in a moment.')
    case 'MPC-E719':
      return plain('The file was saved by another app. Look again before changing it.', true)
    case 'MPC-E723':
      return plain('Someone changed this since you looked. The drawing has been refreshed.', true)
    case 'MPC-E724':
      return plain('K2 couldn’t be sure which items these are. Select them again.')
    case 'MPC-E725':
    case 'MPC-E726':
    case 'MPC-E727':
    case 'MPC-E728':
      return plain(CHECK_FAILED)
    case 'MPC-E729':
      return plain('The drawing changed while saving. Try again.', true)
    case 'MPC-E823':
      return plain('This change couldn’t be saved. Please report it with the file.')
    case WRITER_OFF:
      return plain(e.message || WRITER_OFF_WORDS, false, e.message || WRITER_OFF_WORDS)
    case NO_ROUTE:
      return plain('This server can’t change drawings yet. Ask its owner to update K2.')
    case NOT_SENT:
      return plain('K2 couldn’t reach the server. Try again.')
    case NO_DIGEST:
      return plain(CHECK_FAILED)
    default:
      return plain('K2 couldn’t make this change. Try again, or report it with the file.')
  }
}

export function refusalWords(r: ToolResult, ctx: RefusalContext): RefusalWords {
  const e = r.error ?? { id: '', message: '', next: '', details: {} }
  const { why, refresh, off } = reason(e.id, e, ctx)
  // The server's own words for a writer that's off, as given.
  if (e.id === WRITER_OFF) return { words: why, refresh, off }
  return { words: `${NOT_DONE[ctx.verb]} ${why}`, refresh, off }
}
