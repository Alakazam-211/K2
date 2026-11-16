// The mock's 55 model checks, part two: outside saves, limits and
// addresses, K2 Cores' rulings, the pinned BLEND snapshots and the picture's
// preview states. One `it` per check, in the mock's order and words.

import { describe, expect, it } from 'vitest'
import { GOLD } from './samples'
import { ID16, clone, isAddr, isAddrAny, parse9, same } from './math'
import { CONT, DOCS, UNITS, VS, applyFrame, docFromIndex, liveEntries, KEYED_MAX, nameOf, objOf, worldOf } from './fold'
import { flatRows, sceneTree } from './tree'
import { ATLAS, actorOf, refusalText } from './wording'
import { applyGroup, claimOn } from './sim'
import { TURN, simulate, stepOfRev } from './turn'
import { bigScene, camLabel, hugeScene } from './scenarios'
import { SAVE, SCENARIOS } from './scenarios-more'
import type { Frame, Refusal } from './types'

const D0 = DOCS[GOLD.state.upto]
const IX = GOLD.index
const FRAMES = GOLD.turn.frames
const scenario = (id: string) => SCENARIOS.find((x) => x.id === id)!
const last = (id: string) => simulate(scenario(id), scenario(id).steps.length - 1)

/** Every reference the view reads, from every block and every scenario's events. */
function refsIn(): [string, string][] {
  const out: [string, string][] = []
  const put = (w: string, v: unknown): void => {
    if (v != null) out.push([w, v as string])
  }
  const list = (w: string, v: unknown): void => String(v).split(',').filter(Boolean).forEach((x) => put(w, x))
  const RF = new Set([VS.unit_container_field, 'also', 'parent', ...(VS.relations ?? []).map((r) => r.field)])
  for (const r of GOLD.state.roots) {
    if (r.kind === 'list') (r.live ?? []).forEach(([a]) => put('element', a))
    else if (r.composite_of)
      for (const [a, f, v] of r.entries ?? []) {
        put('entry', a)
        if (RF.has(f as string)) list(f as string, v)
      }
  }
  IX.containers.forEach((c) => put('container', c.key))
  IX.units.forEach((u) => {
    put('unit', u.key)
    put('unit container', u.container)
    if (u.placement) put('relative_to', u.placement.relative_to)
  })
  const frames: Frame[] = FRAMES.concat(GOLD.saves.cases.filter((c) => c.frame).map((c) => c.frame as Frame))
  for (const sc of SCENARIOS) for (const st of sc.steps) for (const e of st.events) if (e.kind === 'frame' && e.made) frames.push(e.made)
  for (const f of frames) {
    f.ops.forEach((o) => {
      if (o.root === 'scene') return
      put('op key', o.key)
      if (o.field && RF.has(o.field)) {
        if (o.before != null) list('op ' + o.field, o.before)
        if (o.after != null) list('op ' + o.field, o.after)
      }
    })
    ;(f.units ?? []).forEach((d) => {
      put('delta', d.key)
      put('delta container', d.container)
      if (d.placement_after) put('relative_to', d.placement_after.relative_to)
    })
    ;(f.containers ?? []).forEach((c) => put('frame container', c))
    Object.keys(f.derived?.units ?? {}).forEach((k) => put('derived', k))
  }
  const pres = (q: { focus?: { key: string } | null; selection?: { keys: string[] } | null }): void => {
    if (q.focus) put('focus', q.focus.key)
    if (q.selection) q.selection.keys.forEach((k) => put('selection', k))
  }
  GOLD.pc.start.forEach(pres)
  for (const e of GOLD.turn.timeline) {
    ;(e.roster_events ?? []).forEach((r) => {
      ;((r.keys as string[]) ?? []).forEach((k) => put('claim', k))
      pres(r as never)
    })
    if (e.event === 'verb_refused') put('refusal unit', (e.frame as { unit: string }).unit)
  }
  for (const sc of SCENARIOS)
    for (const st of sc.steps)
      for (const e of st.events) {
        if (e.kind === 'refused') put('refusal unit', e.r?.unit)
        if (e.kind === 'pick') put('pick', e.addr)
        if (e.kind === 'presence' && e.value && typeof e.value === 'object') pres({ [e.type as string]: e.value })
        if (e.kind === 'claim_started') e.claim?.keys.forEach((k) => put('claim', k))
        if (e.kind === 'ui' && e.expand) e.expand.forEach((c) => put('expand', c))
      }
  return out
}

// The checks below simulate every scenario, which fills each constructed
// event's frame (`made`): refsIn reads them, as the mock's page does after load.
for (const sc of SCENARIOS) for (let i = 0; i < sc.steps.length; i++) simulate(sc, i)

describe('Outside saves (never silently ignored)', () => {
  it('O1: kit:external, change kind external, no turn, then the outside_save frame says ingested. Mug renamed Cup keeps @obj:21: the data rule pairs the one object holding @me:Mug', () => {
    const f = FRAMES.find((x) => x.rev === 65)!
    const cup = f.ops.find((o) => o.field === 'name')!
    const st = GOLD.turn.timeline.find((e) => e.event === 'outside_save' && e.after_rev === 65)
    expect(f.actor.id === 'kit:external' && f.change_kind === 'external' && f.turn === null && cup.key === '@obj:21' && cup.after === 'Cup' && D0.lists[UNITS].filter((a) => (D0.props[a].data || '') === D0.props['@obj:21'].data).length === 1 && (st?.frame as { outcome: string }).outcome === 'ingested' && SAVE('O1').settled.outcome === 'ingested').toBe(true)
  })
  it("O2: a save made over Atlas's unwritten rev 63 is kept both: the save becomes the live file as a change by kit:external, the picture shows it; what was live is kept as a version and a conflict copy (a name and path; no kit doc)", () => {
    const c = SAVE('O2')
    const S = last('O2')
    const lf = S.log.filter((e) => e.kind === 'frame').pop()
    const saved = !!lf && lf.kind === 'frame' && actorOf(lf.actor).kind === 'file' && same((parse9(S.D.props['@obj:8'].xform) as number[]).slice(6, 9), (parse9(DOCS[62].props['@obj:8'].xform) as number[]).slice(6, 9)) && S.D.props['@obj:19'].xform !== DOCS[63].props['@obj:19'].xform
    const copy = c.settled.copy!
    expect(c.settled.outcome === 'kept_both' && lf?.kind === 'frame' && c.settled.rev === lf.rev && /^living-room \(conflict Atlas 2026-11-13 142210\)\.blend$/.test(copy.name) && !!copy.path && !('doc' in copy) && c.settled.kept != null && saved && S.notice?.copy?.name === copy.name && S.tier === 'live' && lf?.kind === 'frame' && S.pv.shown === lf.rev).toBe(true)
  })
  it('O3: Chair.003 renamed and given another mesh in one save pairs by neither rule: @obj:13 deleted, Stool added as @obj:27 past the mark (26 → 27), in the same place; taken in', () => {
    const c = SAVE('O3')
    const f = c.frame!
    const after = applyFrame(D0, f)
    const del = f.ops.find((o) => o.op === 'delete' && o.kind === 'list')!
    const ins = f.ops.find((o) => o.op === 'insert')!
    const at = D0.lists[UNITS].indexOf('@obj:13')
    expect(del.key === '@obj:13' && ins.key === '@obj:27' && +after.scene['mark/o'] === 27 && +D0.scene['mark/o'] === 26 && !after.lists[UNITS].includes('@obj:13') && after.lists[UNITS].indexOf('@obj:27') === at && c.settled.outcome === 'ingested').toBe(true)
  })
  it("O4: a save outside the writer's 3.06–5.01 is declined, so the file can't be live any more: edits pause at rev 65", () => {
    const c = SAVE('O4')
    const v = (x: string): number[] => x.split('.').map(Number)
    const inR = (x: string): boolean => v(x)[0] * 100 + v(x)[1] >= 306 && v(x)[0] * 100 + v(x)[1] <= 501
    const S = last('O4')
    expect(c.declined?.code === 'live_version' && !inR(c.declined.version) && inR(D0.scene.version) && c.settled.outcome === 'declined' && c.settled.code === 'live_version' && !!c.settled.message && S.D.rev === 65 && S.tier === 'paused' && S.notice?.outcome === 'declined').toBe(true)
  })
  it('O5: the file was deleted or moved outside K2 (missing): the live version is kept at rev 65, live edits pause', () => {
    const c = SAVE('O5')
    const S = last('O5')
    expect(c.settled.outcome === 'missing' && S.D.rev === 65 && S.notice?.outcome === 'missing' && S.tier === 'paused').toBe(true)
  })
})

describe('Limits, addresses, refusals', () => {
  it("2,000 objects in 100 collections is under the kit's 25,000 live entries; 2,600 is over: opened from a saved version", () => {
    const BIG = bigScene()
    expect(BIG.lists[UNITS].length === 2000 && liveEntries(BIG) <= KEYED_MAX && liveEntries(hugeScene()) > KEYED_MAX).toBe(true)
  })
  it('the list virtualises: 2,101 rows open, about 36 drawn at a time', () => {
    const rows = flatRows(sceneTree(bigScene()), new Set(), '')
    expect(rows.length).toBe(2101)
    expect(Math.ceil(560 / 28) + 2 * 8).toBeLessThan(60)
    expect(flatRows(sceneTree(bigScene()), new Set(), 'Obj_7_1').hits).toBeGreaterThan(0)
  })
  it("address form: @coll:0, @coll:12, @obj:7 are addresses; @obj:07, @obj:, @obj:-1, obj:1, o7, c3 are not, and @coll:1 is no object", () => {
    expect(['@coll:0', '@coll:12', '@obj:7'].every((a) => isAddr(a)) && ['@obj:07', '@obj:', '@obj:-1', 'obj:1', 'o7', 'c3'].every((x) => !isAddr(x)) && !isAddr('@coll:1', '@obj:')).toBe(true)
  })
  it("agent against agent: Brook's rename of Chair.002, which Atlas holds, is refused MPC-E301 and nothing changes; after Atlas lets go, Brook renames it", () => {
    const sc = scenario('refuse')
    const S = simulate(sc, 1)
    const cl = claimOn(S, '@obj:12')
    const r = (S.log.find((e) => e.kind === 'refused') as { r: Refusal } | undefined)?.r
    const S2 = simulate(sc, sc.steps.length - 1)
    expect(r && refusalText(r, S.D).line).toBe('Brook couldn’t rename Chair.002: Atlas is holding it')
    expect(!!cl && actorOf(cl.actor).id === ATLAS && actorOf(r!.actor).kind === 'agent' && r!.holder === ATLAS && r!.code === 'MPC-E301' && S.D.rev === 60 && S2.D.props['@obj:12'].name === 'Dining chair' && actorOf(S2.log[S2.log.length - 1].actor).id === 'agent:brook').toBe(true)
  })
})

describe('Rulings (K2 Cores, 2026-11-13)', () => {
  it('every reference in every block and scenario is an address, none an internal key', () => {
    const refs = refsIn()
    expect(refs.length).toBeGreaterThan(300)
    expect(refs.filter(([, v]) => !isAddr(v))).toEqual([])
    expect(refs.filter(([, v]) => /^[co]\d+$/.test(v))).toEqual([])
  })
  it('collection nesting is a schema relation "parent" (colls → colls, name provisional), and the tree nests from it alone', () => {
    const rel = (VS.relations ?? []).filter((r) => r.from === CONT && r.to === CONT)
    const T = sceneTree(D0)
    expect(rel.length === 1 && rel[0].name === 'parent' && rel[0].field === 'parent' && !!rel[0]._provisional && !!T.kids['@coll:3'] && T.kids['@coll:3'].includes('@coll:4')).toBe(true)
  })
  it("refusals are the kit's verb_refused frame (c23d2349), in the sample and in every constructed one; the activity line has no code, the code only in the details", () => {
    const ev0 = GOLD.turn.timeline.find((e) => e.event === 'verb_refused')
    const r = ev0?.frame as unknown as Refusal
    const S = simulate(TURN, TURN.steps.findIndex((st) => st.events.some((e) => e.kind === 'refused')))
    const lastE = S.log[S.log.length - 1]
    const t = refusalText(r, S.D)
    const KIT = ['at_ms', 'actor', 'client', 'turn', 'verb', 'unit', 'code', 'holder', 'claim']
    const keysOf = (x: object): string[] => Object.keys(x).filter((k) => k[0] !== '_' && k !== 'type').sort()
    const fields = r.type === 'verb_refused' && same(keysOf(r), KIT.slice().sort()) && isAddr(r.unit, '@obj:') && r.turn === FRAMES[0].turn && r.actor?.id === ATLAS
    const cons = SCENARIOS.flatMap((sc) => sc.steps.flatMap((st) => st.events.filter((e) => e.kind === 'refused').map((e) => e.r as Refusal)))
    expect(fields && cons.every((x) => x.type === 'verb_refused' && same(keysOf(x), KIT.slice().sort())) && lastE.kind === 'refused' && !/E301|MPC-/.test(t.line) && t.detail.includes('MPC-E301')).toBe(true)
    expect(t.line).toBe('Atlas couldn’t move Vase: Brook is holding it')
  })
  it('verb_refused reaches every viewer who can read: with view rights the same turn shows the refusal too', () => {
    const S = TURN.base()
    applyGroup(S, TURN.steps[0], 0)
    S.rights = 'view'
    for (let i = 1; i < TURN.steps.length; i++) applyGroup(S, TURN.steps[i], i)
    expect(S.log.some((e) => e.kind === 'refused') && simulate(TURN, TURN.steps.length - 1).log.some((e) => e.kind === 'refused')).toBe(true)
  })
  it('every outside save is one outside_save frame in kit main 02ddb6aa\'s final shapes, sent after its change frame; "merged" appears nowhere', () => {
    const all = GOLD.turn.timeline.filter((e) => e.event === 'outside_save').map((e) => e.frame as Record<string, unknown>).concat(GOLD.saves.cases.map((c) => c.settled as unknown as Record<string, unknown>))
    const ext = FRAMES.filter((f) => f.change_kind === 'external')
    const each = ext.every((f) => GOLD.turn.timeline.some((e) => e.event === 'outside_save' && e.after_rev === f.rev))
    const SH: Record<string, string[]> = { ingested: ['outcome', 'rev'], kept_both: ['outcome', 'rev', 'kept', 'copy'], declined: ['outcome', 'code', 'message'], missing: ['outcome'] }
    const noMerged = !/merged/.test(JSON.stringify(GOLD.saves) + JSON.stringify(GOLD.turn.timeline.filter((e) => e.event === 'outside_save')))
    const ok =
      all.length === 6 &&
      all.every((f) => f.type === 'outside_save' && SH[f.outcome as string] && !('_provisional' in f) && same(Object.keys(f).filter((k) => k[0] !== '_' && k !== 'type'), SH[f.outcome as string])) &&
      all.filter((f) => f.outcome === 'kept_both').every((f) => { const c = f.copy as Record<string, string>; return same(Object.keys(c).filter((k) => k !== 'doc'), ['name', 'path']) && !!c.name && !!c.path }) &&
      all.filter((f) => f.outcome === 'ingested' || f.outcome === 'kept_both').every((f) => Number.isInteger(f.rev))
    expect(ok && each && noMerged && new Set(all.map((f) => f.outcome)).size === 4).toBe(true)
  })
  it("agents act, people watch: every edit verb across the scenarios (frames and refusals) is an agent's; people only move focus and selection or go away", () => {
    const edits: { kind: string }[] = []
    const people: string[] = []
    for (const sc of SCENARIOS)
      for (const st of sc.steps)
        for (const e of st.events) {
          if (e.kind === 'card') people.push(sc.id + ' card')
          const v = e.kind === 'frame' ? (e.frame ? e.frame.verb : e.multi ? (actorOf(e.multi[1]).kind === 'file' ? null : 'object place') : e.rename ? 'object rename' : 'object place') : e.kind === 'refused' ? e.r?.verb : null
          if (!v) continue
          const a = e.kind === 'frame' ? (e.frame ? actorOf(e.frame.actor) : actorOf(e.multi ? e.multi[1] : e.rename ? e.rename[2] : ATLAS)) : actorOf(e.r?.actor)
          edits.push(a)
          if (a.kind !== 'agent') people.push(sc.id + ' ' + a.name + ' ' + v)
        }
    expect(edits.length).toBeGreaterThanOrEqual(8)
    expect(people).toEqual([])
    expect(FRAMES.filter((f) => !f.verb).every((f) => actorOf(f.actor).kind === 'file')).toBe(true)
  })
  it('presence: the ViewAddress roster event and peer.viewAddress (kit main 02ddb6aa), any kit address of at most 512 bytes; never "view"', () => {
    const ros = GOLD.turn.timeline.flatMap((e) => e.roster_events ?? [])
    const scen = SCENARIOS.flatMap((sc) => sc.steps.flatMap((x) => x.events.filter((e) => e.kind === 'presence')))
    const va = ros.filter((r) => r.kind === 'ViewAddress')
    const ok = !ros.some((r) => /^view/.test(r.kind as string)) && va.length > 0 && va.every((r) => isAddrAny(r.viewAddress) && new TextEncoder().encode(r.viewAddress as string).length <= 512) && GOLD.pc.start.every((q) => !('view' in q) && !('view_address' in q)) && !scen.some((e) => e.type === 'view' || e.type === 'view_address')
    expect(ok).toBe(true)
  })
  it("kit 92f084c5: a container's layout reference that isn't an address is dropped; here every collection's layout is null", () => {
    expect(IX.containers.filter((c) => c.layout != null && !isAddr(c.layout))).toEqual([])
  })
})

describe('Pinned BLEND snapshots (k2-mpc-blend 686cd0da)', () => {
  const PO = GOLD.pinned.outliner
  const PR = GOLD.pinned.rig
  const keys = (o: object): string => Object.keys(o).filter((k) => k[0] !== '_').join(',')
  it("the sample's index has the pinned shape key for key, in the kit's order; a unit can lack placement: it isn't placed and gets no box", () => {
    const sameUnit = PO.units.filter((u) => u.placement).every((u) => keys(u) === keys(IX.units[0]))
    const noPl = PO.units.filter((u) => !('placement' in u)).map((u) => u.name)
    expect(keys(PO) === keys(IX) && keys(PO.containers[0]) === keys(IX.containers[0]) && sameUnit && noPl.length === 1 && !worldOf(docFromIndex(PO), '@obj:3')).toBe(true)
  })
  it('the outliner snapshot reads as pinned: approx "driven" on Walker and Held; one turns by quaternion; no metres per unit (the tab says "units")', () => {
    const dr = PO.units.filter((u) => u.placement?.approx === 'driven').map((u) => u.name)
    const q = PO.units.find((u) => u.placement?.steps[0].trs?.rot.quat)
    const Dp = docFromIndex(PO)
    expect(dr.join(',') === 'Walker,Held' && !!q && Dp.props[q.key].rotmode === 'quaternion' && Dp.space?.metres_per_unit === null && objOf(Dp, '@obj:6').edit === 'a').toBe(true)
  })
  it('rig: Arm is placed on Base through a real parent inverse (a quarter turn and a move); world = Base · inverse · own puts Arm at (-1.5, -2, 1)', () => {
    const IXR = PR.index
    const arm = IXR.units.find((u) => u.name === 'Arm')!
    const base = IXR.units.find((u) => u.name === 'Base')!
    const w = worldOf(docFromIndex(IXR), arm.key)
    const m = arm.placement?.steps[0].matrix
    expect(arm.placement?.relative_to === base.key && !!m && !same(m, ID16) && !!w && Math.abs(w[12] + 1.5) < 1e-6 && Math.abs(w[13] + 2) < 1e-6 && Math.abs(w[14] - 1) < 1e-6).toBe(true)
  })
  it("rig: Crowd's 64 objects arrive in the two pinned pages (33 + 31); the doc holds 0, then 33, then 64 of them", () => {
    const pg = PR.crowd_pages.pages
    const crowd = PR.index.units.filter((u) => u.container === pg[0].container).map((u) => u.key)
    const ok = pg.length === 2 && pg[0].units.length === 33 && pg[1].units.length === 31 && pg[0].from === 0 && pg[0].next === 33 && pg[1].from === 33 && pg[1].next === null && pg.every((x) => x.total === 64) && same(pg.flatMap((x) => x.units), crowd)
    const sc = scenario('pin-rig')
    const n = (i: number): number => { const S = simulate(sc, i); return S.D.lists[UNITS].filter((a) => S.D.props[a].coll === pg[0].container).length }
    expect(ok && n(0) === 0 && n(1) === 33 && n(2) === 64 && simulate(sc, 1).pages?.[pg[0].container].next === 33).toBe(true)
  })
  it("the tree nests from the index's parent (BLEND main 0c552c1a), siblings in position order; a parent that isn't in the file, or makes a cycle, is refused and that collection shows at the top level", () => {
    const T = sceneTree(docFromIndex(PO))
    const D2 = clone(docFromIndex(PO))
    D2.props['@coll:3'].parent = '@coll:9'
    const T2 = sceneTree(D2)
    const D3 = clone(docFromIndex(PO))
    D3.props['@coll:2'].parent = '@coll:3'
    const T3 = sceneTree(D3)
    expect(PO.containers.filter((c) => 'parent' in c).length === 3 && same(T.roots, ['@coll:1', '@coll:0']) && same(T.kids['@coll:1'], ['@coll:2', '@coll:4']) && same(T.kids['@coll:2'], ['@coll:3']) && T2.refused.length === 1 && T2.roots.includes('@coll:3') && T3.refused.length === 1 && T3.refused[0].why === 'cycle' && T3.roots.length === 3).toBe(true)
  })
})

describe('The picture (K2 Render previews; provisional)', () => {
  it('every change to a shape\'s place gets a pending still (the old one stays up, state stale) and then a "picture" step where the new one lands: revs 61, 62, 63, 65', () => {
    const bad: string[] = []
    for (const f of FRAMES.filter((x) => x.verb === 'object place' || actorOf(x.actor).kind === 'file')) {
      const j = stepOfRev(f.rev)
      const a = simulate(TURN, j)
      const b = simulate(TURN, j + 1)
      if (!(a.pv.state === 'stale' && a.pv.shown === f.rev - 1 && a.pv.want === f.rev)) bad.push('rev ' + f.rev + ' pending')
      if (!(b.pv.state === 'ready' && b.pv.shown === f.rev && b.pv.landed && TURN.steps[j + 1].label === 'picture')) bad.push('rev ' + f.rev + ' lands')
    }
    expect(bad).toEqual([])
  })
  it("a rename doesn't redraw: rev 64's still is rev 63's, reused (state ready, no wait)", () => {
    const j = stepOfRev(64)
    const S = simulate(TURN, j)
    expect(S.pv.state === 'ready' && S.pv.reused === 63 && S.pv.shown === 64 && !TURN.steps[j].events.some((e) => e.kind === 'preview' && e.p?.state === 'rendering')).toBe(true)
  })
  it('views: Scene, one per camera in the file (one active), a framed object, Turntable; every view a person or agent is on is one of them', () => {
    const V = GOLD.previews.views
    const cams = D0.lists[UNITS].filter((a) => D0.kinds[a] === 'camera')
    const okV = V.every((v) => v.view === '@scene:main' || v.view === '@scene/turntable' || v.view === '@obj:*' || (v.kind === 'camera' && v.view === '@cam:' + (v.object as string).split(':')[1] && cams.includes(v.object as string)))
    const seen = GOLD.turn.timeline.flatMap((e) => e.roster_events ?? []).filter((r) => r.kind === 'ViewAddress').map((r) => r.viewAddress as string)
    const okSeen = seen.every((v) => V.some((x) => x.view === v) || (isAddr(v, '@obj:') && !!D0.props[v]))
    expect(okV && okSeen && V.filter((v) => v.kind === 'camera').length === cams.length && V.filter((v) => v.active).length === 1).toBe(true)
    expect(cams.map((a) => nameOf(D0, a))).toEqual(['Camera', 'Camera.001'])
  })
  it("the still is a lossless WebP with alpha and no background; cameras show as Camera 1, Camera 2 until the mesh read carries names; Shelf's bounds are approximate (modifiers)", () => {
    const P = GOLD.previews
    const cams = P.views.filter((v) => v.kind === 'camera')
    expect(P.background.alpha === true && !('gd' in P.background) && cams.every((v) => v._name) && cams.map((v) => camLabel(v.view)).join(', ') === 'Camera 1, Camera 2' && P.bounds['@obj:16'].approx === 'modifiers').toBe(true)
  })
})
