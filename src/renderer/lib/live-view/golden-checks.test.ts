// The approved mock's checks (mockup-live-workbook-v1.html, 45 of 45 against
// r0), ported to the typed modules and re-based on the r1 goldens, plus the
// r1 additions (written, bands, the outside save, merges, styles, sheet
// states, units, an agent's cells after each kind of verb). Every value comes
// from the goldens; their `_` annotations are read only here.

import { describe, expect, it } from 'vitest'
import { GOLDEN, goldenCase, stripAnnotations, type GoldenGridSheet } from './samples/goldens'
import { fold, play, stateAtRev } from './test-helpers'
import { claimRect, idleOf, modeOf, type ClaimRecord, type LiveWorkbookFrame } from './reducer'
import { checkCells, checkViewport, claimVerdict, dropBroken } from './kit-rules'
import { activityLine, framePhrase, phraseLine, turnGroups } from './wording'
import { EMPTY_LOCAL, stageSheet } from './view-model'
import { SAMPLE_FILE_NAME, sampleRosterSnapshot, sampleSnapshotFrames } from './sample-source'
import {
  a1Of,
  applyFrameToBook,
  cellShown,
  colWidthPx,
  cornerOk,
  cornerPlace,
  gridOf,
  isDefaultStyle,
  letters,
  lettersToN,
  rangeA1,
  rangeRect,
  rowHeightPx,
  sheetsOf,
  styleOf,
  viewportA1,
  type Book,
} from './workbook'
import type { CellsValue, XlsxOpsChangeFrame, XlsxRosterEvent } from './xlsx-provisional'

const P = play('turn')
const B = P.books
const S1 = '@sheet:1'
const S2 = '@sheet:2'
const FRAMES = GOLDEN.turn.frames
const frame = (rev: number): XlsxOpsChangeFrame => stripAnnotations(FRAMES.find((f) => f.rev === rev) as XlsxOpsChangeFrame)
const grid = (rev: number, sheet = S1) => {
  const g = gridOf(B[rev], sheet)
  if (!g) throw new Error(`no grid ${sheet} at rev ${rev}`)
  return g
}
const MAX = GOLDEN.schema.grid.max

/** What the grid shows, in expected-grid.json's form. */
function expectedForm(book: Book, sheet: string): GoldenGridSheet {
  const g = gridOf(book, sheet)
  if (!g) throw new Error(`no grid ${sheet}`)
  const sh = sheetsOf(book).find((s) => s.key === sheet)
  const byRow = new Map<string, Record<string, Record<string, unknown>>>()
  for (const c of g.cells) {
    const shown = cellShown(c)
    const st = styleOf(g, c)
    const form: Record<string, unknown> = shown.kind === 'formula' ? { formula: shown.formula, result: shown.result, result_state: shown.resultState } : { value: shown.text }
    if (!isDefaultStyle(st)) form.style = st
    const row = byRow.get(c.row) ?? {}
    row[letters(c.c)] = form
    byRow.set(c.row, row)
  }
  const keyed = new Set(g.rows.map((r) => r.n))
  const blank: number[] = []
  for (let n = 1; n <= g.lastRow; n++) if (!keyed.has(n)) blank.push(n)
  return {
    sheet,
    name: g.title,
    state: sh?.state ?? 'visible',
    columns: Object.fromEntries(g.cols.map((c) => [c.letter, c.key])),
    merges: g.merges.map((m) => rangeA1(g, m) as string),
    blank_rows: blank,
    rows: g.rows.map((r) => ({ number: r.n, address: r.key, hidden: r.hidden, cells: byRow.get(r.key) ?? {} })),
  }
}

describe('Numbering (kit item VG)', () => {
  it("row numbers from list order plus @gap match the index's _row_number for every unit", () => {
    const bad = GOLDEN.index.units.filter((u) => gridOf(B[20], u.container)?.rowByKey.get(u.key)?.n !== u._row_number)
    expect(bad.map((u) => u.key)).toEqual([])
    expect(GOLDEN.index.units.length).toBe(29)
  })
  it('column letters: 1 → A, 26 → Z, 27 → AA, 16,384 → XFD, and back', () => {
    for (const [n, l] of [[1, 'A'], [26, 'Z'], [27, 'AA'], [MAX.cols, 'XFD']] as const) {
      expect(letters(n)).toBe(l)
      expect(lettersToN(l)).toBe(n)
    }
  })
})

describe('Grid vs expected-grid (values, A1 text, cached or not calculated, merges, style hints)', () => {
  for (const rev of ['rev20', 'rev28', 'rev29'] as const) {
    for (const exp of GOLDEN.grid[rev]) {
      it(`${rev}, ${exp.name}: ${exp.rows.length} rows, blank ${JSON.stringify(exp.blank_rows)}`, () => {
        expect(expectedForm(B[Number(rev.slice(3))], exp.sheet)).toEqual(stripAnnotations(exp))
      })
    }
  }
})

describe('Frames (turn-frames.json)', () => {
  it('rev 23: 2 ops; 10 keyed rows renumbered with no op; new A1 text for 10 formulas in 8 rows', () => {
    const f = frame(23)
    const g0 = grid(22)
    const g1 = grid(23)
    const opKeys = new Set(f.ops.flatMap((o) => (o.kind === 'text' ? [] : [o.key])))
    const moved = g1.rows.filter((r) => g0.rowByKey.has(r.key) && g0.rowByKey.get(r.key)?.n !== r.n)
    const units = f.derived?.units ?? {}
    expect(f.ops.length).toBe(2)
    expect(moved.filter((r) => !opKeys.has(r.key)).length).toBe(10)
    expect(Object.values(units).reduce((n, u) => n + Object.keys(u.a1 ?? {}).length, 0)).toBe(10)
    expect(Object.keys(units).length).toBe(8)
  })
  it('rev 28: one @gap op, no list op; rows 18–21 become 17–20; bands name row 17 by its number before', () => {
    const f = frame(28)
    const g0 = grid(27)
    const g1 = grid(28)
    const moved = g1.rows.filter((r) => g0.rowByKey.get(r.key)?.n !== r.n)
    expect(f.ops.length).toBe(1)
    expect(moved.map((r) => [g0.rowByKey.get(r.key)?.n, r.n])).toEqual([[18, 17], [19, 18], [20, 19], [21, 20]])
    expect(f.derived?.bands).toEqual([{ container: S1, axis: 'rows', op: 'delete', from: 17, to: 17, numbers: 'before' }])
  })
  it('frame.claim is claim-31 exactly when the write lands inside it (revs 21, 23, 24), else null', () => {
    let claims: ClaimRecord[] = []
    const got: Record<number, string | null> = {}
    for (const st of P.steps) {
      for (const e of st.events) {
        if (e.type === 'roster_event' && e.event.kind === 'claim_started_cells') claims.push({ id: e.event.claim, actor: e.event.actor, scope: 'cells', container: e.event.container, keys: null, ranges: e.event.ranges, label: e.event.note ?? '', hold: e.event.hold, started_ms: e.event.started_ms, expires_ms: e.event.expires_ms })
        else if (e.type === 'roster_event' && e.event.kind === 'claim_ended') { const id = e.event.claim; claims = claims.filter((c) => c.id !== id) }
        else if (e.type === 'ops_change') {
          const B1 = B[e.rev]
          const mine = claims.filter((c) => c.actor.id === e.actor.id)
          const rows = e.ops.flatMap((o) => (o.kind !== 'text' && (o.root === 'rows' || o.root === 'cells') ? [o.key] : []))
          const hit = mine.find((c) => {
            const CR = claimRect(B1, c)
            const g = gridOf(B1, c.container)
            if (!CR || !g) return false
            const w = e.derived?.written
            if (w) return w.ranges.some((r) => { const R = rangeRect(g, r); return !!R && R.r0 <= CR.r1 && CR.r0 <= R.r1 && R.c0 <= CR.c1 && CR.c0 <= R.c1 })
            return rows.some((k) => { const r = g.rowByKey.get(k); return !!r && r.n >= CR.r0 && r.n <= CR.r1 })
          })
          // rev 23 inserts a whole row (written A10:XFD10): it lands inside by its rows, as the kit counts an insert.
          got[e.rev] = hit ? hit.id : null
        }
      }
    }
    expect(got).toEqual(Object.fromEntries(FRAMES.map((f) => [f.rev, f.claim])))
  })
  it('rev 29, the outside save: actor kind file, change kind external, no turn; a new merge A20:F20', () => {
    const f = frame(29)
    expect([f.actor.kind, f.change_kind, f.turn, f.verb, f.claim]).toEqual(['file', 'external', null, null, null])
    expect(grid(29).merges.map((m) => rangeA1(grid(29), m))).toEqual(['A1:F1', 'A20:F20'])
  })
})

describe("An agent's cells after each kind of verb (r1 rulings 1 and 2, AG1–AG4)", () => {
  const flushAfter = (rev: number) => {
    const ev = GOLDEN.turn.timeline.find((e) => e.event === 'roster_flush' && e.after_rev === rev)
    const cells = ev?.roster_events?.find((r) => r.kind === 'cells')
    return cells && cells.kind === 'cells' ? (stripAnnotations(cells.cells) as CellsValue) : null
  }
  it("AG1: a verb that writes cells: the frame's written ranges, active at the first range's start (revs 21, 22, 24, 25)", () => {
    for (const rev of [21, 22, 24, 25]) {
      const w = frame(rev).derived?.written
      const c = flushAfter(rev)
      expect(c?.ranges).toEqual(w?.ranges)
      expect(c?.active).toEqual(w?.ranges[0].start)
    }
  })
  it('AG2: an insert: the inserted row as a whole band, A10:XFD10', () => {
    const c = flushAfter(23) as CellsValue
    expect(rangeA1(grid(23), c.ranges[0])).toBe('A10:XFD10')
    expect(c.ranges).toEqual(frame(23).derived?.written?.ranges)
  })
  it('AG3: a delete: a caret on the first VISIBLE cell after the band (A18: row 17 is the hidden fx row), no ranges (r1.1)', () => {
    const c = flushAfter(28) as CellsValue
    const g = grid(28)
    expect(c.ranges).toEqual([])
    expect(a1Of(cornerPlace(g, c.active!)!)).toBe('A18')
    expect(g.rowByNumber.get(17)?.hidden).toBe(true)
    expect(goldenCase('AG3').example).toContain('A18')
  })
  it('AG4: a rename or add focuses that sheet; cells become a caret at A1 of it (FX rates; empty Notes as {null, 1, null, 1})', () => {
    const c26 = flushAfter(26) as CellsValue
    const c27 = flushAfter(27) as CellsValue
    expect([c26.container, a1Of(cornerPlace(grid(26, S2), c26.active!)!)]).toEqual([S2, 'A1'])
    expect(c27.active).toEqual({ row: null, row_off: 1, col: null, col_off: 1 })
    expect(a1Of(cornerPlace(grid(27, '@sheet:5'), c27.active!)!)).toBe('A1')
  })
})

describe('Wording (K2 writes it)', () => {
  for (const f of FRAMES.filter((x) => x.actor.kind === 'agent')) {
    it(`rev ${f.rev}: the place the view works out at the change's rev is in the golden's own words`, () => {
      const ph = framePhrase(frame(f.rev), B[f.rev - 1], B[f.rev])
      expect(f._view.split('. ')[0]).toContain(ph.place)
      expect(ph.who).toBe('Atlas')
    })
  }
  it('rev 29: "budget-golden.xlsx was saved outside K2: C4 and 9 results changed", worded through the file', () => {
    const ph = framePhrase(frame(29), B[28], B[29])
    expect(ph.who).toBeNull()
    expect(phraseLine(ph, SAMPLE_FILE_NAME)).toBe('budget-golden.xlsx was saved outside K2: C4 and 9 results changed')
  })
  it('rev 25 hover: the formula was =E19*C18, now =E19*Rates!B3, not calculated (no v)', () => {
    const f = frame(25)
    const o = f.ops[0]
    if (o.kind !== 'map') throw new Error('rev 25 is a map op')
    const was = B[24].a1[o.key][o.field as string]
    const now = B[25].a1[o.key][o.field as string]
    expect(FRAMES.find((x) => x.rev === 25)?._view).toContain(was)
    expect(FRAMES.find((x) => x.rev === 25)?._view).toContain(now)
    expect((JSON.parse(o.after as string) as { v?: string }).v).toBeUndefined()
  })
  it("the addendum's §3 examples appear in the view's lines", () => {
    const all = FRAMES.map((f) => framePhrase(frame(f.rev), B[f.rev - 1], B[f.rev]).text)
    for (const x of ['inserted a row at 10 on Budget', 'changed the formula in E20', 'renamed Rates to FX rates', 'added the sheet Notes', 'set D5:D7 on Budget', 'deleted row 17 on Budget']) {
      expect(all.some((t) => t.includes(x)), x).toBe(true)
    }
  })
})

describe('Presence points (presence-and-claims.json)', () => {
  for (const pt of GOLDEN.presence.points) {
    it(`${pt.id} (rev ${pt.rev}): focus, selection, cells, viewport, pointer, mode, isSelf; claims; the sheet shown`, () => {
      const s = stateAtRev(P, pt.rev)
      const book = s.book as Book
      expect(book.rev).toBe(pt.rev)
      const want = stripAnnotations(pt)
      for (const q of want.peers) {
        const p = s.peers[q.handle]
        expect(p, q.name).toBeDefined()
        expect({ focus: p.focus, selection: p.selection, cells: p.cells, viewport: p.viewport, pointer: p.pointer, isSelf: p.isSelf, away: p.away }).toEqual({ focus: q.focus, selection: q.selection, cells: q.cells, viewport: q.viewport, pointer: q.pointer, isSelf: q.isSelf, away: q.away })
        expect(modeOf(book, p, s.claims), `${q.name} mode`).toBe(q.mode_derived)
      }
      // The A1 annotations: carets, ranges and viewports worked out from corners and numbers.
      for (const q of pt.peers) {
        if (q.cells) {
          const g = gridOf(book, q.cells.container)!
          if (q.cells._active_a1 && q.cells.active) expect(a1Of(cornerPlace(g, q.cells.active)!)).toBe(q.cells._active_a1)
          for (const r of q.cells.ranges) if (r._a1) expect(rangeA1(g, r)).toBe(r._a1)
        }
        if (q._viewport_a1 && q.viewport) expect(q._viewport_a1.startsWith(viewportA1(q.viewport))).toBe(true)
      }
      expect(s.claims).toEqual(want.claims_active)
      for (const c of pt.claims_active) expect(rangeA1(gridOf(book, c.container)!, c.ranges[0])).toBe(c.ranges[0]._a1)
      expect(s.ended.map(({ id, actor, how, at_ms, at_rev }) => ({ id, actor, how, at_ms, at_rev }))).toEqual(want.claims_ended)
      const self = want.peers.find((q) => q.isSelf)
      expect(stageSheet(s, EMPTY_LOCAL)).toBe(self?.focus?.key)
    })
  }
})

describe('Visible range (VR)', () => {
  it('VR1: {x 1, y 1, w 8, h 30} → A1:H30 on Budget', () => {
    expect(viewportA1(goldenCase('VR1').roster_event.viewport)).toBe(goldenCase('VR1')._a1)
  })
  it('VR2: y 12.5 → A12:H42, row 12 half shown', () => {
    expect(viewportA1(goldenCase('VR2').client_frame.rect)).toBe(goldenCase('VR2')._a1)
  })
  it('VR3: the viewport keeps A1:H30 across rev 23 with no event; under it, Training moves from row 10 to 11', () => {
    const c = goldenCase('VR3')
    expect(c.before.viewport).toEqual(c.after.viewport)
    expect(stateAtRev(P, 23).peers['person:ada'].viewport).toEqual(c.after.viewport)
    expect([grid(22).rowByKey.get('@sheet:1/row:r9')?.n, grid(23).rowByKey.get('@sheet:1/row:r9')?.n]).toEqual([10, 11])
  })
  it("VR4: one flush focus → cells → viewport (the flush order); Ada's caret on Budget dropped; her range read on Rates", () => {
    const c = goldenCase('VR4')
    const base = fold([{ type: 'welcome', client: 77001, self: 'person:sam', rights: 'read' }, ...sampleSnapshotFrames(), sampleRosterSnapshot()])
    const events = (c.roster_events_one_flush as XlsxRosterEvent[]).map((ev): LiveWorkbookFrame => ({ type: 'roster_event', event: ev }))
    const s = fold(events, base)
    const ada = s.peers['person:ada']
    const order = (c.roster_events_one_flush as { kind: string }[]).map((e) => e.kind)
    expect(ada.focus?.key).toBe(S2)
    expect(ada.cells).toBeNull()
    expect(ada.viewport).toEqual(c.roster_events_one_flush[2].viewport)
    expect(order).toEqual(['focus', 'selection', 'cells', 'range', 'pointer', 'viewport'].filter((k) => order.includes(k)))
  })
  it('VR5: a negative size and a watcher are refused (MPC-E114, MPC-E305)', () => {
    const c = goldenCase('VR5')
    const got = (c.rows as { frame: { rect: { x: number; y: number; w: number; h: number } }; from?: string }[]).map((r) => checkViewport(r.frame.rect, { watcher: !!r.from }))
    expect(got).toEqual((c.rows as { reply: { error: { id: string } } }[]).map((r) => r.reply.error.id))
  })
})

describe('Caret and ranges (CS)', () => {
  it('CS1: a caret at E18, accepted', () => {
    const c = goldenCase('CS1')
    expect(a1Of(cornerPlace(grid(20), c.client_frame.cells.active)!)).toBe(c._a1)
    expect(checkCells(B[20], c.client_frame.cells)).toBeNull()
  })
  it('CS2: A1:B4 on Rates as one unit, accepted', () => {
    const c = goldenCase('CS2')
    expect(rangeA1(grid(20, S2), c.client_frame.cells.ranges[0])).toBe(c._a1)
    expect(checkCells(B[20], c.client_frame.cells)).toBeNull()
  })
  it('CS3: A1:J1000, 10,000 cells in two corners, accepted', () => {
    const c = goldenCase('CS3')
    const R = rangeRect(grid(20, S2), c.cells.ranges[0])!
    expect(rangeA1(grid(20, S2), c.cells.ranges[0])).toBe(c._a1)
    expect((R.r1 - R.r0 + 1) * (R.c1 - R.c0 + 1)).toBe(10000)
    expect(checkCells(B[20], c.cells)).toBeNull()
  })
  it('CS4: C4:C15 and E4:E15, active E4', () => {
    const c = goldenCase('CS4')
    const g = grid(20)
    const got = `${c.cells.ranges.map((r: never) => rangeA1(g, r)).join(' and ')}, active ${a1Of(cornerPlace(g, c.cells.active)!)}`
    expect(got).toBe(c._a1)
    expect(checkCells(B[20], c.cells)).toBeNull()
  })
  it('CS5: a whole column D1:D1048576 and a whole row A9:XFD9', () => {
    const c = goldenCase('CS5')
    expect(rangeA1(grid(20), c.column_D)).toBe(c.column_D._a1)
    expect(rangeA1(grid(20), c.row_9)).toBe(c.row_9._a1)
  })
  it('CS6: {r1, 1, c1, 0} → A2, a blank cell, in normal form', () => {
    const c = goldenCase('CS6')
    const got = a1Of(cornerPlace(grid(20), c.active)!)
    expect(c._a1.startsWith(`${got} `)).toBe(true)
    expect(cornerOk(grid(20), c.active, MAX)).toBe(true)
  })
  it('CS7: C4:D15 at rev 22 → C4:D16 at rev 23, same corners, no event', () => {
    const c = goldenCase('CS7')
    expect(rangeA1(grid(22), c.range)).toBe(c._a1_rev22)
    expect(rangeA1(grid(23), c.range)).toBe(c._a1_rev23)
    expect(c.event).toBeNull()
  })
  it("CS8: Brook deletes row 14 (a real frame on a branch from rev 20): Sam's range is dropped, his caret kept; Brook's caret on A14", () => {
    const c = goldenCase('CS8')
    const after = applyFrameToBook(B[20], stripAnnotations(c.frame) as XlsxOpsChangeFrame)
    const flush = stripAnnotations(c.roster_flush) as { kind: string; who: string; cells?: CellsValue }[]
    const sam = flush.find((e) => e.kind === 'cells' && e.who === 'person:sam')
    expect(dropBroken(after, stripAnnotations(c.before.cells))).toEqual(sam?.cells)
    const brook = flush.find((e) => e.kind === 'cells' && e.who === 'agent:brook')?.cells as CellsValue
    expect(a1Of(cornerPlace(gridOf(after, S1)!, brook.active!)!)).toBe('A14')
  })
  it("CS9: Brook inserts a row in Ada's blank stretch (a real frame): her corner {r14, 1} now names a keyed row, so the kit drops it all: cells null", () => {
    const c = goldenCase('CS9')
    const after = applyFrameToBook(B[20], stripAnnotations(c.frame) as XlsxOpsChangeFrame)
    const ada = (stripAnnotations(c.roster_flush) as { kind: string; who: string; cells?: CellsValue | null }[]).find((e) => e.kind === 'cells' && e.who === 'person:ada')
    expect(ada?.cells).toBeNull()
    expect(dropBroken(after, stripAnnotations(c.before.cells))).toBeNull()
  })
  it('CS10: a focus move to Rates drops the cells value', () => {
    const base = fold([{ type: 'welcome', client: 77001, self: 'person:sam', rights: 'read' }, ...sampleSnapshotFrames(), sampleRosterSnapshot()])
    expect(base.peers['person:ada'].cells).not.toBeNull()
    const s = fold([{ type: 'roster_event', event: { kind: 'focus', who: 'person:ada', focus: { root: 'sheets', key: S2 } } }], base)
    expect(s.peers['person:ada'].cells).toBeNull()
    expect(s.peers['person:ada'].focus?.key).toBe(S2)
  })
  it("CS11: cells refused (the kit's frames and ErrorReports): 17 ranges unsent, a corner not live, on another sheet, not in form, past the last row, active outside, a watcher", () => {
    const rows = goldenCase('CS11').rows as { why: string; from?: string; client_frame?: { cells: CellsValue }; client_frame_not_sent?: { cells: CellsValue }; reply?: { error: { id: string; message: string } } }[]
    const got = rows.map((r) => checkCells(B[20], (r.client_frame ?? r.client_frame_not_sent)!.cells, { watcher: !!r.from }))
    // The unsent one never reaches the host: the client's own limit (MPC-E310).
    expect(got).toEqual(rows.map((r) => r.reply?.error.id ?? 'MPC-E310'))
    for (const r of rows) if (r.reply) expect(r.reply.error.message.length).toBeGreaterThan(0)
  })
})

describe('Claims on ranges (CC)', () => {
  it("CC1: the claim's start gives Atlas focus Budget and cells = the claim, active C4", () => {
    const s = stateAtRev(P, 20)
    const atlas = s.peers['agent:atlas']
    const c = goldenCase('CC1')
    const flush = stripAnnotations(c.roster_events_one_flush) as { kind: string; cells?: CellsValue }[]
    expect(atlas.focus?.key).toBe(S1)
    expect(atlas.cells).toEqual(flush.find((e) => e.kind === 'cells')?.cells)
    expect(a1Of(cornerPlace(grid(20), atlas.cells!.active!)!)).toBe('C4')
  })
  it('CC1: C4:D15 at the start → C4:D16 at the release; claim_ended lines = rows 4–16 at version 25', () => {
    const c = goldenCase('CC1')
    const cl = stateAtRev(P, 20).claims[0]
    const R = rangeRect(grid(25), cl.ranges![0])!
    expect(`${rangeA1(grid(20), cl.ranges![0])} at the start; ${rangeA1(grid(25), cl.ranges![0])} at the release`).toBe(c._a1)
    expect([R.r0, R.r1]).toEqual([c.end.lines.first, c.end.lines.last])
    expect(stateAtRev(P, 25).ended[0].lines).toEqual({ first: 4, last: 16 })
  })
  it('CC2: what a range claim holds off: refused for other agents inside (MPC-E301, no frame), applied otherwise, and its own write carries the claim', () => {
    const c = goldenCase('CC2')
    const cl = stateAtRev(P, 20).claims[0]
    const kinds: Record<string, string> = { 'agent:brook': 'agent', 'agent:atlas': 'agent', 'person:ada': 'person' }
    const rows = c.rows as { who: string; verb: string; result: string; error?: { id: string }; frame?: XlsxOpsChangeFrame | null }[]
    for (const r of rows) {
      const v = claimVerdict(B[20], cl, { who: r.who, whoKind: kinds[r.who], verb: r.verb })
      expect(v.startsWith('refused'), r.verb).toBe(r.result.startsWith('refused'))
      if (r.error) expect(v).toBe(`refused ${r.error.id}`)
      if (r.result.includes('claim-31')) expect(v).toContain('claim-31')
      // An applied agent write is a real frame that fits the book, with no claim.
      if (r.frame) expect(applyFrameToBook(B[20], stripAnnotations(r.frame)).rev).toBe(r.frame.rev)
    }
  })
  it("CC3: a rows claim's keys r3, r4 are rows 4 and 5: “rows 4-5”", () => {
    const c = goldenCase('CC3')
    const ns = (c.client_frame.keys as string[]).map((k) => grid(20).rowByKey.get(k)?.n)
    expect(c.client_frame.note).toBe(`rows ${ns[0]}-${ns[ns.length - 1]}`)
  })
})

describe('Sheets and units (r1)', () => {
  it('HS1: sheet states come through: Budget visible, Lists hidden, Setup very hidden', () => {
    const c = goldenCase('HS1')
    const got = sheetsOf(B[20]).map((s) => ({ key: s.key, title: s.title, hidden: s.hidden, state: s.state }))
    for (const want of c.containers as { key: string }[]) expect(got.find((s) => s.key === want.key)).toEqual(want)
  })
  it('UN1: width 22 characters and height 24 points → pixels (fixed digit width until X-18; 96 dpi)', () => {
    const g = grid(20)
    expect(g.cols[0].widthChars).toBe(22)
    expect(g.rows[0].heightPt).toBe(24)
    expect(colWidthPx(22)).toBe(166)
    expect(rowHeightPx(24)).toBe(32)
  })
})

describe('Styles and idle turns (r1.1)', () => {
  it('ST1: a sheet with no style table (Notes, added at rev 27) and an index a table lacks both take the default style', () => {
    const notes = grid(27, '@sheet:5')
    expect(notes.styles).toEqual({})
    const fake = { row: 'x', field: 'c1', r: 1, c: 1, t: 's', v: 'a', s: 3, a1: null, raw: '' }
    expect(styleOf(notes, fake)).toEqual(goldenCase('ST1').default)
    expect(styleOf(grid(27), { ...fake, s: 99 })).toEqual(goldenCase('ST1').default)
  })
  it("a turn with no turn_end ends turn_idle_ms after the agent's last change (rev 30 at 240000 → 260000); presence alone doesn't keep it open", () => {
    const i = P.steps.findIndex((s) => s.label === 'idle')
    const before = P.states[i - 1]
    const after = P.states[i]
    const open = turnGroups(before.log, idleOf(before))
    expect(open[open.length - 1]).toMatchObject({ turn: 't:5e0a9d31c7b2f846.8', ended: false })
    const shut = turnGroups(after.log, idleOf(after))
    expect(shut[shut.length - 1]).toMatchObject({ turn: 't:5e0a9d31c7b2f846.8', ended: true, endedBy: 'idle', endedAt: 260000 })
    // A turn_end arriving later for it changes nothing shown.
    const late = fold([{ type: 'turn_end', turn: 't:5e0a9d31c7b2f846.8', actor: GOLDEN.turn.actors['agent:atlas'] as never, at_ms: 270000 }], after)
    expect(turnGroups(late.log, idleOf(late)).at(-1)).toMatchObject({ endedBy: 'idle', endedAt: 260000 })
  })
})

describe('End of the turn', () => {
  it('one history group for the turn: revs 21–28, claim-31 started and released inside it, ended by turn_end; then the outside save on its own, then the idle-ended turn', () => {
    const last = P.states[P.states.length - 1]
    const groups = turnGroups(last.log)
    const tEnd = GOLDEN.turn.timeline.find((e) => e.event === 'turn_end')?.frame
    const g = groups[0]
    const frames = g.items.filter((e) => e.kind === 'frame')
    expect(groups.length).toBe(3)
    expect(g.turn).toBe(tEnd?.type === 'turn_end' ? tEnd.turn : null)
    expect(frames.map((e) => (e.kind === 'frame' ? e.rev : 0))).toEqual([21, 22, 23, 24, 25, 26, 27, 28])
    expect(g.endedAt).toBe(216500)
    expect([...new Set(g.items.filter((e) => e.kind === 'claim').map((e) => (e.kind === 'claim' ? e.id : '')))]).toEqual(['claim-31'])
    expect(groups[1]).toMatchObject({ turn: null, ended: true, endedBy: 'no turn', actor: { kind: 'file' } })
    expect(groups[0].endedBy).toBe('turn_end')
  })
  it('the line after turn_end: “Atlas is done · deleted row 17 on Budget, a blank row, +7 more”', () => {
    const i = P.steps.findIndex((s) => s.label === 'turn_end')
    const line = activityLine(P.states[i].log, P.states[i].claims)
    expect(line.done).toBe(true)
    expect(line.actor?.name).toBe('Atlas')
    expect(line.latest?.text).toMatch(/^deleted row 17 on Budget/)
    expect(line.more).toBe(7)
  })
})
