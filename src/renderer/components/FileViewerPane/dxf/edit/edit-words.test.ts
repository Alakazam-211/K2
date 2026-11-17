// Every refusal an edit can meet, in plain words: never the code, never the
// kit's own message. Some also refresh the drawing (it changed under you)
// or turn editing off for the file (it can't be changed here at all).

import { describe, expect, it } from 'vitest'
import { NO_DIGEST } from './edit-flow'
import { NO_ROUTE, NOT_SENT, WRITER_OFF, WRITER_OFF_WORDS } from './edit-source'
import { refusalWords, viewOnlyWhy, whyWords, WHY_WORDS } from './edit-words'
import { localRefusal } from './tool-result'

const PACK = [
  'DXF-E001', 'DXF-E002', 'DXF-E003',
  'DXF-E010', 'CAD-E010', 'DXF-E011', 'CAD-E011', 'DXF-E020', 'DXF-E021', 'DXF-E022', 'DXF-E027',
  'DXF-E040', 'DXF-E041', 'DXF-E042', 'DXF-E043', 'DXF-E044', 'DXF-E045', 'DXF-E046', 'DXF-E047',
  'DXF-E048', 'DXF-E049', 'DXF-E050', 'DXF-E053', 'DXF-E054', 'DXF-E090', 'CAD-E004',
]
const KIT = [
  'MPC-E110', 'MPC-E122', 'MPC-E301', 'MPC-E701', 'MPC-E713', 'MPC-E719', 'MPC-E723', 'MPC-E724',
  'MPC-E725', 'MPC-E726', 'MPC-E727', 'MPC-E728', 'MPC-E729', 'MPC-E823',
]
const OURS = [NO_ROUTE, NOT_SENT, NO_DIGEST]
// (A writer that's off shows the server's own words: tested on its own below.)
const say = (id: string, message = '', details: Record<string, unknown> = {}, verb: 'move' | 'relabel' = 'move') =>
  refusalWords(localRefusal(id, message, details), { verb, layerOf: () => 'A-WALL' })

describe('refusalWords', () => {
  it('every refusal has its own words, which never show a code or the kit’s message', () => {
    const seen = new Set<string>()
    for (const id of [...PACK, ...KIT, ...OURS]) {
      const w = say(id, `@h:41 kit message for ${id}`).words
      expect(w, id).not.toMatch(/[A-Z]{3}-E\d|K2-[A-Z]|@h:|kit message/)
      expect(w, id).toMatch(/^[A-Z“]/)
      expect(w, id).toMatch(/[.?]$/)
      seen.add(w)
    }
    // Different refusals read differently (a few share their words on purpose: every failed check reads the same).
    expect(seen.size).toBeGreaterThan(28)
  })

  it('leads with what didn’t happen', () => {
    expect(say('DXF-E045').words).toMatch(/^Not moved\. /)
    expect(say('DXF-E048', '', {}, 'relabel').words).toMatch(/^Not changed\. /)
  })

  it('a locked layer is named', () => {
    expect(say('DXF-E045', '@h:41 is on a locked layer').words).toBe('Not moved. A-WALL is locked. Unlock it in your CAD app, then try again.')
  })

  it('an overlap and an outside save read as written, and refresh the drawing', () => {
    expect(say('MPC-E723')).toMatchObject({ words: 'Not moved. Someone changed this since you looked. The drawing has been refreshed.', refresh: true })
    expect(say('MPC-E719')).toMatchObject({ words: 'Not moved. The file was saved by another app. Look again before changing it.', refresh: true })
    for (const id of ['MPC-E110', 'MPC-E729', 'DXF-E010', 'DXF-E022', 'DXF-E047']) expect(say(id).refresh, id).toBe(true)
    expect(say('DXF-E045').refresh).toBe(false)
  })

  it('a drawing that can’t be changed here at all turns editing off, saying why', () => {
    expect(say('DXF-E040').off).toMatch(/before DXF 2000/)
    expect(say('DXF-E041').off).toMatch(/handles/)
    expect(say('DXF-E001').off).toMatch(/binary/i)
    expect(say('DXF-E045').off).toBeNull()
  })

  it('names who holds it, or whose lease it is', () => {
    expect(say('MPC-E301', '', { by: 'Atlas' }).words).toBe('Not moved. Atlas is working on these. Try again when they’re done.')
    expect(say('MPC-E701', '', { holder: 'Sam' }).words).toMatch(/^Not moved\. Sam has this file locked/)
    expect(say('MPC-E701').words).toMatch(/^Not moved\. Someone has this file locked/)
  })

  it('a kind the verbs don’t change says which reason, from the message’s word', () => {
    expect(say('DXF-E021', '@h:6B isn’t changed by the edit verbs (dimension)').words).toBe(`Not moved. ${WHY_WORDS.dimension}`)
    expect(say('DXF-E021', '@h:4C isn’t changed by the edit verbs (assoc)').words).toBe(`Not moved. ${WHY_WORDS.assoc}`)
    expect(say('DXF-E021', 'anything else').words).toBe(`Not moved. ${WHY_WORDS.unknown}`)
  })

  it('a value that can’t be written says what’s wrong with it', () => {
    const v = (m: string) => say('DXF-E054', `the value can't be written on one line here: ${m}`, {}, 'relabel').words
    expect(v('a line break or control character')).toMatch(/one line/)
    expect(v('longer than 255 bytes')).toMatch(/255/)
    expect(v('a \\U+ sequence, which the format reads as an escape')).toMatch(/\\U\+/)
    expect(v("a character this file's code page can't name")).toMatch(/character/)
  })
})

describe('whyWords', () => {
  it('every reason the model gives has words; an unknown one reads as unknown', () => {
    for (const k of ['hatch', 'dimension', 'leader', 'viewport', 'acis', 'image', 'proxy', 'unknown', 'xref', 'block', 'assoc', 'ocs', 'minted', 'dynamic'])
      expect(whyWords(k), k).toBe(WHY_WORDS[k as keyof typeof WHY_WORDS])
    expect(whyWords('something new')).toBe(WHY_WORDS.unknown)
    expect(whyWords('locked', 'A-WALL')).toBe('It’s on A-WALL, which is locked. Unlock it in your CAD app first.')
  })
})

describe('a writer that’s off', () => {
  it('shows the server’s words as given, and turns editing off with them', () => {
    const hint = 'K2 can’t save drawing edits yet; this file opens read-only.'
    expect(say(WRITER_OFF, hint)).toEqual({ words: hint, refresh: false, off: hint })
  })
})

describe('the drawing model’s reason words', () => {
  it('each maps to a plain-word reason', () => {
    const want: Record<string, string> = {
      grouped: WHY_WORDS.grouped,
      dimensioned: WHY_WORDS.dimensioned,
      'own-frame': WHY_WORDS.ocs,
      'no-handle': WHY_WORDS.minted,
      proxy: WHY_WORDS.proxy,
      xref: WHY_WORDS.xref,
      '3D solid': WHY_WORDS.acis,
      unknown: WHY_WORDS.unknown,
    }
    for (const [w, words] of Object.entries(want)) expect(whyWords(w), w).toBe(words)
    expect(whyWords('locked-layer', 'A-WALL')).toBe('It’s on A-WALL, which is locked. Unlock it in your CAD app first.')
  })
  it('a refusal naming one reads the same', () => {
    expect(say('DXF-E021', '@h:4C isn’t changed by the edit verbs (own-frame)').words).toBe(`Not moved. ${WHY_WORDS.ocs}`)
    expect(say('DXF-E021', '@h:4C isn’t changed by the edit verbs (3D solid)').words).toBe(`Not moved. ${WHY_WORDS.acis}`)
  })
})

describe('viewOnlyWhy', () => {
  const header = { format: 'dxf', acadver: 'AC1032' }
  it('a DXF from 2000 on, with handles: editable', () => {
    expect(viewOnlyWhy(header, [{ h: '@h:41' }])).toBeNull()
  })
  it('DWG, binary, before 2000, or without handles: why not', () => {
    expect(viewOnlyWhy({ ...header, format: 'dwg' }, [])).toMatch(/DWG/)
    expect(viewOnlyWhy({ ...header, binary: true }, [])).toMatch(/binary/i)
    expect(viewOnlyWhy({ ...header, acadver: 'AC1014' }, [])).toMatch(/before DXF 2000/)
    expect(viewOnlyWhy(header, [{ h: '' }])).toMatch(/handles/)
    expect(viewOnlyWhy({ ...header, handles: { missing: 2, duplicates: 0 } }, [])).toMatch(/handles/)
  })
  it('the host’s writer: false is view only, in its words when it gives them', () => {
    expect(viewOnlyWhy({ ...header, writer: false, read_only_reason: 'Edits to drawings are off here.' }, [])).toBe('Edits to drawings are off here.')
    expect(viewOnlyWhy({ ...header, writer: false }, [])).toBe(WRITER_OFF_WORDS)
    expect(viewOnlyWhy({ ...header, writer: true }, [])).toBeNull()
    // DWG keeps its own words.
    expect(viewOnlyWhy({ ...header, format: 'dwg', writer: false }, [])).toMatch(/DWG/)
  })
})
