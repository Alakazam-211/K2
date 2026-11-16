// The mock's 55 model checks (mockup-live-scene-v2.html, goldenChecks), part
// one: the view schema, the outliner from the keyed state, numbers and
// placements, the turn's frames, and presence at P1 to P3. One `it` per
// check, in the mock's order and words. Part two: model-checks-more.test.ts.

import { describe, expect, it } from 'vitest'
import { GOLD } from './samples'
import { isAddr, mul, obFileKey, parse9, placementFrom, placementMatrix, same, xformParts, xformText, f32 } from './math'
import { CF, CONT, DOCS, UF, UNITS, VS, applyFrame, collTitle, nameOf, objOf, worldOf } from './fold'
import { flatRows, sceneTree } from './tree'
import { ATLAS, BROOK, actorOf, framePhrase } from './wording'
import { TURN, simulate, stepOfRev, turnGroups } from './turn'

const D0 = DOCS[GOLD.state.upto]
const IX = GOLD.index
const T0 = sceneTree(D0)
const FRAMES = GOLD.turn.frames

describe('View schema (plan section 2, view.rs)', () => {
  it('containers colls, units objs, unit_container_field coll, container_id_field none, native_unit scene-unit, no page, no 2D geometry', () => {
    expect(VS.containers === 'colls' && VS.units === 'objs' && VS.unit_container_field === 'coll' && VS.container_id_field === null && VS.native_unit === 'scene-unit' && VS.page === null && VS.geometry === null).toBe(true)
  })
  it('container title = name, hidden = hidden; unit kind = the list value, name = name, hidden = hidden, no text', () => {
    expect(CF.title === 'name' && CF.hidden === 'hidden' && CF.layout === null && same(UF.kind, { list_value: true }) && UF.name === 'name' && UF.hidden === 'hidden' && UF.text === null).toBe(true)
  })
  it('field_class: xform and quat geometry; the three visibility bits; name text; the rest meta. Every field name in the state has a class', () => {
    const fc = VS.field_class
    const objFields = new Set(Object.values(D0.props).flatMap((p) => Object.keys(p)))
    const classed = [...objFields].filter((f) => fc[f])
    const want = fc.xform === 'geometry' && fc.quat === 'geometry' && ['hidden', 'noselect', 'norender'].every((f) => fc[f] === 'visibility') && fc.name === 'text' && ['nid', 'coll', 'also', 'data', 'parent', 'rotmode', 'edit'].every((f) => fc[f] === 'meta')
    expect(want && classed.length === objFields.size).toBe(true)
  })
  it("derived per unit: transform, data; the 3D slot places objects from xform, rotmode, quat, parent (f32); collections aren't placed", () => {
    expect(same(VS.unit_derived, ['transform', 'data']) && same(VS.space.unit_fields, ['xform', 'rotmode', 'quat', 'parent']) && VS.space.precision === 'f32' && same(VS.space.container_fields, [])).toBe(true)
  })
})

describe('The outliner from the keyed state', () => {
  it("containers in the colls list's file order, @coll:0 last", () => {
    const want = D0.lists[CONT]
    const got = IX.containers.map((c) => c.key)
    expect(same(got, want) && want[want.length - 1] === '@coll:0' && IX.containers.every((c, i) => c.position === i + 1) && want.every((a) => isAddr(a, '@coll:'))).toBe(true)
    expect(want.map((a) => collTitle(D0, a))).toEqual(['Scene', 'Room', 'Furniture', 'Props', 'Not in a collection'])
  })
  it("every unit's container is its coll field, an address as sent, and each container's unit_count matches", () => {
    const bad = IX.units.filter((u) => u.container !== D0.props[u.key].coll)
    const counts = IX.containers.every((c) => c.unit_count === (T0.objsIn[c.key] ?? []).length)
    expect(!bad.length && counts && IX.units.length === D0.lists[UNITS].length).toBe(true)
  })
  it("unit kind, name and hidden equal the state's list value, name and hidden fields for all units", () => {
    const bad = IX.units.filter((u) => {
      const o = objOf(D0, u.key)
      return u.kind !== o.kind || u.name !== o.name || u.hidden !== o.hidden
    })
    expect(bad.map((u) => u.key)).toEqual([])
  })
  it('collections nest by the schema\'s "parent" relation (@coll:0 has no title: K2 says "Not in a collection")', () => {
    expect(T0.roots.length === 2 && T0.roots[1] === '@coll:0' && IX.containers.find((c) => c.key === '@coll:0')?.title === null).toBe(true)
  })
  it('an object in several collections shows once, in its first (Q-B2), with the rest named', () => {
    const multi = IX.units.filter((u) => objOf(D0, u.key).also.length)
    const rows = flatRows(T0, new Set(), '')
    expect(multi.length > 0 && multi.every((u) => rows.filter((r) => r.addr === u.key).length === 1)).toBe(true)
  })
  it('derived transform = "loc x,y,z rot x,y,z scale x,y,z" of xform for all units (view.rs xform_text)', () => {
    expect(IX.units.filter((u) => u.derived?.transform !== xformText(D0.props[u.key].xform)).map((u) => u.key)).toEqual([])
  })
  it('derived data = the data field; none for the empties', () => {
    expect(IX.units.filter((u) => (D0.props[u.key].data || undefined) !== u.derived?.data).map((u) => u.key)).toEqual([])
  })
})

describe('Numbers and placements', () => {
  it('every xform is 9 finite f32s, each its shortest round-trip decimal', () => {
    const all = Object.values(D0.props).filter((p) => p.xform)
    const bad = all.filter((p) => !parse9(p.xform) || p.xform.split(',').some((s) => String(f32(Number(s))) !== s))
    expect(bad).toEqual([])
  })
  it("placements match place.rs from each object's own fields: trs = xform, the Euler word = rotmode in capitals, a quaternion from quat (Ceiling fan)", () => {
    expect(IX.units.filter((u) => !same(u.placement, placementFrom(D0.props[u.key]))).map((u) => u.key)).toEqual([])
  })
  it("parented objects are placed relative to the parent's address with [parent inverse, trs]", () => {
    const par = IX.units.filter((u) => D0.props[u.key].parent)
    const ok = par.every((u) => u.placement?.relative_to === D0.props[u.key].parent && u.placement?.steps.length === 2 && !!u.placement.steps[0].matrix) && IX.units.filter((u) => !D0.props[u.key].parent).every((u) => u.placement?.relative_to === null)
    expect(ok).toBe(true)
  })
  it('a driven transform\'s placement says approx "driven"; no other does', () => {
    const dr = IX.units.filter((u) => D0.props[u.key].edit === 'a')
    const ok = dr.every((u) => u.placement?.approx === 'driven') && IX.units.filter((u) => D0.props[u.key].edit !== 'a').every((u) => !u.placement?.approx)
    expect(ok && dr.length > 0).toBe(true)
  })
  it('world = parent world · parent inverse · own (kit placement.ts): Book.001 at (2.3, …, 1.12) m', () => {
    const w = worldOf(D0, '@obj:17')
    const sh = worldOf(D0, '@obj:16')
    const own = placementMatrix(D0.units['@obj:17'].placement)
    const want = mul(sh!, own!)
    expect(!!w && [12, 13, 14].every((i) => Math.abs(w[i] - want[i]) < 1e-9) && Math.abs(w[12] - 2.3) < 1e-6 && Math.abs(w[14] - 1.12) < 1e-6).toBe(true)
  })
  it('Euler XYZ is Rz · Ry · Rx about fixed axes (wire "Placements in 3D"): Table at 15° about Z gives cos, sin in the first column', () => {
    const m = placementMatrix(DOCS[62].units['@obj:10'].placement)
    const a = Math.fround((15 * Math.PI) / 180)
    expect(!!m && Math.abs(m[0] - Math.cos(a)) < 1e-6 && Math.abs(m[1] - Math.sin(a)) < 1e-6 && Math.abs(m[4] + Math.sin(a)) < 1e-6).toBe(true)
  })
  it("the doc's space: scene-unit, 1 m per unit (unit_scale 1), Z up, right-handed", () => {
    const sp = D0.space!
    expect(sp.length_unit === 'scene-unit' && sp.metres_per_unit === 1 && sp.up === 'z' && sp.handed === 'right' && D0.scene.unit_scale === '1').toBe(true)
  })
  it("3 decimals: Chair.002's Z rotation reads 180.000° (3.142 rad with the radians toggle)", () => {
    const vs = (parse9(D0.props['@obj:12'].xform) as number[])[5]
    expect(((vs * 180) / Math.PI).toFixed(3)).toBe('180.000')
    expect(vs.toFixed(3)).toBe('3.142')
  })
})

describe('Frames: the turn', () => {
  it('object place: one op setting xform, a unit delta "place" whose placement_after is place.rs\'s for the new xform, and the new transform text: revs 61, 62, 63', () => {
    const pl = FRAMES.filter((f) => f.verb === 'object place')
    const bad = pl.filter((f) => {
      const op = f.ops[0]
      const d = f.units[0]
      const p = applyFrame(DOCS[f.rev - 1], f).props[op.key]
      return !(f.ops.length === 1 && op.op === 'set' && op.field === 'xform' && f.units.length === 1 && same(d.ops, ['place']) && same(d.fields, ['xform']) && same(d.placement_after, placementFrom(p)) && f.derived?.units[op.key].transform === xformText(op.after))
    })
    expect(pl.length).toBe(3)
    expect(bad).toEqual([])
  })
  it('one place op each for a move, a turn and a resize, worded from what changed in xform', () => {
    const ph = FRAMES.filter((f) => f.verb === 'object place').map((f) => framePhrase(f, DOCS[f.rev - 1], DOCS[f.rev]).text)
    expect(ph.join(' | ')).toBe('moved Table | turned Table | resized Rug')
  })
  it('a rename sets name and nid in one change (B-B1), spaces escaped (scene.rs escape_name)', () => {
    const f = FRAMES.find((x) => x.verb === 'object rename')!
    const nm = f.ops.find((o) => o.field === 'name')
    const nid = f.ops.find((o) => o.field === 'nid')
    expect(!!nm && !!nid && f.ops.length === 2 && nid.after === obFileKey(nm.after as string) && nid.after === '@ob:Tall%20vase').toBe(true)
  })
  it('no frame places a driven (a) or linked (0) object, or turns a quaternion one (t)', () => {
    const bad = FRAMES.filter((f) => f.verb === 'object place').filter((f) => {
      const p = DOCS[f.rev - 1].props[f.ops[0].key]
      const rotChanged = !same(xformParts(f.ops[0].before)?.rot, xformParts(f.ops[0].after)?.rot)
      return p.edit === 'a' || p.edit === '0' || (p.edit === 't' && rotChanged)
    })
    expect(bad).toEqual([])
  })
  it("one turn groups Atlas's changes, its claim and its refused move; turn_end closes it. Brook's rename is a turn of its own; the outside save has none", () => {
    const S = simulate(TURN, TURN.steps.length - 1)
    const g = turnGroups(S)
    const atl = g.find((x) => x.turn && x.actor.id === ATLAS)
    const brk = g.find((x) => x.turn && x.actor.id === BROOK)
    const n = atl ? atl.items.filter((e) => e.kind === 'frame').length : 0
    const fr = FRAMES.filter((f) => actorOf(f.actor).id === ATLAS)
    const ok = !!atl && n === 3 && fr.every((f) => f.turn === atl.turn) && !!atl.ended && !!S.turnEnds[atl.turn as string] && atl.items.some((e) => e.kind === 'refused') && !!brk && !!brk.ended && brk.turn !== atl.turn && brk.items.filter((e) => e.kind === 'frame').length === 1 && FRAMES.filter((f) => actorOf(f.actor).kind === 'file').every((f) => f.turn === null)
    expect(ok).toBe(true)
  })
  it("claims name objects by address (≤ 256 keys); frames made under a claim name it; Atlas's held claim ends released; Brook's held claim on Vase ends released right after the rename", () => {
    const ros = GOLD.turn.timeline.flatMap((e) => e.roster_events ?? [])
    const st = ros.filter((r) => r.kind === 'claim_started_elems') as Record<string, unknown>[]
    const en = ros.filter((r) => r.kind === 'claim_ended') as Record<string, unknown>[]
    const ok = st.every((c) => c.root === UNITS && (c.keys as string[]).length <= 256 && (c.keys as string[]).every((k) => isAddr(k, '@obj:'))) && FRAMES.filter((f) => f.claim).every((f) => st.some((c) => c.claim === f.claim && (c.keys as string[]).includes(f.ops[0].key)))
    const c13 = st.find((c) => c.claim === 'claim-13')!
    const e13 = en.find((e) => e.claim === 'claim-13')!
    expect(ok && en.find((e) => e.claim === 'claim-12')?.how === 'released' && c13.who === BROOK && !!c13.hold && e13.how === 'released' && e13.at_rev === 64).toBe(true)
  })
  it('Vase rides on Table: its world spot moves (0.600, 0.400) → (0.2, …) with no op of its own', () => {
    simulate(TURN, stepOfRev(61))
    const v0 = worldOf(DOCS[60], '@obj:20')
    const v1 = worldOf(DOCS[61], '@obj:20')
    const f = FRAMES.find((x) => x.rev === 61)!
    expect(!!v0 && !!v1 && Math.abs(v1[12] - 0.2) < 1e-6 && Math.abs(v0[12] - 0.6) < 1e-6 && !f.ops.some((o) => o.key === '@obj:20')).toBe(true)
  })
})

describe('Presence vs the points', () => {
  it.each(GOLD.pc.points.map((P) => [P.id, P] as const))('%s: who is on what, focus, claims, modes and views', (_id, P) => {
    const j = TURN.steps.findIndex((s) => s.point === P.id)
    const S = simulate(TURN, j)
    const bad: string[] = []
    for (const [h, keys] of Object.entries(P.on)) if (!same(S.peers[h]?.selection?.keys, keys)) bad.push(h + ' on')
    for (const [h, k] of Object.entries(P.focus)) if (S.peers[h]?.focus?.key !== k) bad.push(h + ' focus')
    if (!same(S.claims.map((c) => ({ claim: c.id, by: actorOf(c.actor).id, keys: c.keys, note: c.label })), P.claims)) bad.push('claims')
    for (const [h, m] of Object.entries(P.modes)) if (S.peers[h].mode !== m) bad.push(h + ' mode')
    for (const [h, v] of Object.entries(P.views ?? {})) if (S.peers[h]?.view !== v) bad.push(h + ' view')
    expect(j).toBeGreaterThanOrEqual(0)
    expect(bad).toEqual([])
    expect(nameOf(S.D, '@obj:4')).toBe('Floor')
  })
})
