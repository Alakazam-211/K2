// The mock's model checks 1–17: the view schema, the pack test's pins, and
// ticks. Each check is one of mockup-live-timeline-v2.html's, on the same
// data, through the same rules.

import { describe, it } from 'vitest'
import { bookFromPages } from './adapter'
import { modelOf } from './fold'
import { GOLD } from './samples'
import { GB } from './scenarios-monitor'
import { COMPOSITE, KEYED_ROOTS, PROPS, UNIT, VIEW } from './schema'
import { same } from './sim'
import { ok } from './test-world'
import { big, MAX53, rateOf, tcText } from './time'
import { B30, BOOKS, FRAMES, FRT } from './world'

const VS = GOLD.schema.view
const M30 = modelOf(B30)

describe('View schema (plan §2)', () => {
  it('#1 containers = tracks, units = items, unit_container_field = track, no container id field, native unit tick, no page', () => {
    ok(VS.containers === 'tracks' && VS.units === 'items' && VS.unit_container_field === 'track' && VS.container_id_field === null && VS.native_unit === 'tick' && VS.page === null, 'the view schema slots')
    ok(VS.containers === VIEW.containers && VS.units === VIEW.units && same(GOLD.schema.keyed.roots, KEYED_ROOTS) && same(GOLD.schema.keyed.composite, COMPOSITE), 'the model reads the same schema it was given')
  })

  it('#2 geometry geom [x, w]; title = name; kind = list value; hidden = off; editable_text from edit; derived units tc, containers label and kind', () => {
    ok(
      VS.geometry.field === 'geom' &&
        same(VS.geometry.parts, ['x', 'w']) &&
        VS.container_fields.title === 'name' &&
        same(VS.unit_fields.kind, { list_value: true }) &&
        VS.unit_fields.hidden === 'off' &&
        same(VS.unit_fields.editable_text, { field: 'edit', values: { 1: 'plain', 0: 'never' } }) &&
        same(VS.derived, { containers: ['label', 'kind'], units: ['tc'] }),
      'the view schema fields',
    )
    ok(VIEW.geometry === VS.geometry.field && VIEW.container_title === VS.container_fields.title && VIEW.hidden === VS.unit_fields.hidden, 'the model reads the same fields')
  })

  it('#3 lanes come from the tracks list order', () => {
    const bad = GOLD.index.containers.filter((c, i) => {
      const t = M30.tracks[i]
      return !t || t.key !== c.key || t.title !== c.title || c.position !== i + 1
    })
    ok(!bad.length && M30.tracks.length === GOLD.index.containers.length, `lanes: ${M30.tracks.map((t) => t.key + ' ' + t.title).join(', ')}`)
  })

  it('#4 lane labels and kinds are the index’s derived values (kind_label, kind), not worked out from list order', () => {
    const dc = Object.fromEntries(GOLD.index.containers.map((c) => [c.key, c.derived || {}]))
    const bad = M30.tracks.filter((t) => t.labelFrom !== 'derived' || t.kindFrom !== 'derived' || t.label !== dc[t.key].kind_label || t.kind !== dc[t.key].kind)
    ok(!bad.length, `labels ${M30.tracks.map((t) => t.label + ' ' + t.kind).join(', ')}`)
  })

  it('#5 every unit: bbox x, w = geom; container = its track field’s address; kind, name, hidden = off, editable_text', () => {
    const bad = GOLD.index.units.filter((u) => {
      const it = M30.items[u.key]
      return !it || !u.bbox || BigInt(u.bbox.x) !== it.s || BigInt(u.bbox.w) !== it.d || u.container !== it.track || u.kind !== it.kind || u.name !== it.name || u.hidden !== it.off || u.editable_text !== it.edit
    })
    ok(!bad.length && GOLD.index.units.length === M30.count, `differ: ${bad.map((u) => u.key).join(', ')}`)
  })

  it('#6 derived tc: the view’s timecode from ticks equals the kit’s derived.tc for every clip and transition, in the index, the frames and the pinned golden', () => {
    let n = 0
    const bad: string[] = []
    const chk = (M: ReturnType<typeof modelOf>, key: string, want: unknown): void => {
      n++
      const it = M.items[key]
      const got = it ? M.tc(it.s) : null
      if (got !== want) bad.push(`${key} ${got}≠${String(want)}`)
    }
    GOLD.index.units.forEach((u) => chk(M30, u.key, u.derived.tc))
    ;(GOLD.golden.cut.index as unknown as { units: { key: string; derived: { tc: string } }[] }).units.forEach((u) => chk(modelOf(GB.cut), u.key, u.derived.tc))
    FRAMES.forEach((f) => Object.entries(f.derived?.units || {}).forEach(([k, v]) => chk(modelOf(BOOKS[f.rev]), k, v.tc)))
    ok(!bad.length && n === 43, `${n} units: ${bad.slice(0, 3).join('; ')}`)
  })

  it('#7 markers aren’t units; an item’s marker is media time, placed at start + (at − src)', () => {
    const ms = M30.markers
    const items = GOLD.index.units.map((u) => u.key)
    const it = ms.find((m) => m.owner === 'item')
    const host = it && it.ownerKey ? M30.items[it.ownerKey] : null
    const exp = host && it && it.at != null && host.src != null ? host.s + (it.at - host.src) : null
    ok(ms.every((m) => !items.includes(m.key)) && it && it.rec === exp && M30.tc(it.rec) === '01:00:45:04', `“${it?.name}” at ${it ? M30.tc(it.rec) : '?'}`)
  })
})

describe('Pinned by the pack’s test (c9739a65)', () => {
  const C = GOLD.cut
  const P = C._pinned.expect
  const M = modelOf(bookFromPages({ upto: 0, roots: C.roots }))
  const i1 = M.items[P.item1.key]

  it('#8 the Cut fixture’s lanes are V1, A1, V2', () => ok(same(M.tracks.map((t) => t.title), P.lanes), JSON.stringify(M.tracks.map((t) => t.title))))
  it('#9 six units: Shot A, Shot B, the dissolve, Shot C, Shot A again, Music', () => ok(M.count === P.units, `${M.count} units`))
  it('#10 @item:1 on @trk:1 at x = 24·F, w = 48·F, “Shot A”', () => ok(i1 && i1.track === P.item1.container && Number(i1.s) === P.item1.x && Number(i1.d) === P.item1.w && i1.name === P.item1.name && BigInt(P.F) === M.FR, 'item 1'))
  it('#11 derived tc of @item:1 is 01:00:01:00-01:00:03:00', () => ok(`${M.tc(i1.s)}-${M.tc(i1.e)}` === P.tc_item1, `${M.tc(i1.s)}-${M.tc(i1.e)}`))
  it('#12 a transition’s geom is [cut − in, in + out] and it sorts by its cut before the item that starts there', () => {
    const tr = M.items['@item:3']
    ok(tr && tr.kind === 'transition' && tr.f0 === 90 && tr.f1 === 102 && M.byT['@trk:1'].items.map((x) => x.name || x.kind).join(',') === 'Shot A,Shot B,transition,Shot C,Shot A again', `frames ${tr?.f0}–${tr?.f1}`)
  })
  it('#13 view.rs timecodes_count_from_the_start_timecode: 01:00:00:00, 00:00:01:01, none between frames', () => {
    const u = C._pinned.view_rs_unit_tests
    const R24 = rateOf('24/1')!
    const fr = R24.frame!
    ok(tcText(0n, R24, 3600n * 24n * fr) === u['timecode(0, 3600*24*fr, 24)'] && tcText(25n * fr, R24, 0n) === u['timecode(25*fr, 0, 24)'] && tcText(fr / 2n, R24, 0n) === u['timecode(fr/2, 0, 24)'], 'view.rs unit test')
  })
})

describe('Ticks (plan §1.4)', () => {
  it('#14 a tick is 1/254,016,000,000 s: frames at 24, 25, 30000/1001 and samples at 48 kHz, 44.1 kHz are whole ticks', () => {
    const r = (s: string) => rateOf(s)!.frame
    ok(r('24/1') === 10584000000n && r('25/1') === 10160640000n && r('30000/1001') === 8475667200n && r('48000/1') === 5292000n && r('44100/1') === 5760000n, 'frame ticks')
  })

  it('#15 a time-of-day src past 2^53 is kept as a string and read exactly as BigInt; one tick later wouldn’t survive a double', () => {
    const s = B30.maps[PROPS[UNIT]]['@item:1'].src
    const b = big(s)!
    const odd = b + 1n
    const viaNum = BigInt(Number(String(odd)))
    ok(typeof s === 'string' && b > MAX53 && b % FRT === 0n && tcText(b, rateOf('24/1'), 0n) === '14:03:22:10' && viaNum !== odd, `src ${s}`)
  })

  it('#16 every geom at every rev is on the 24 fps grid and record times stay under 2^53 ticks', () => {
    const bad: string[] = []
    for (const B of Object.values(BOOKS)) {
      const M = modelOf(B)
      for (const x of Object.values(M.items)) {
        if (x.s % M.FR !== 0n || x.d % M.FR !== 0n) bad.push(x.key)
        if (x.e > MAX53) bad.push(x.key + ' past 2^53')
      }
    }
    ok(!bad.length, bad.slice(0, 4).join(', '))
  })

  it('#17 every tick on the wire as a JSON number (the index bbox) is a safe integer', () => {
    const all: number[] = []
    const scan = (v: unknown): void => {
      if (typeof v === 'number' && !Number.isSafeInteger(v)) all.push(v)
      else if (v && typeof v === 'object') Object.values(v).forEach(scan)
    }
    scan(GOLD.index.units.map((u) => u.bbox))
    ok(!all.length, String(all))
  })
})
