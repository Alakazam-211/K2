// The mock's model checks 48–59: what K2 Cores ruled on 2026-11-13.

import { describe, it } from 'vitest'
import { actorOf } from './actors'
import { clone, keyOf } from './adapter'
import { lanesOf, modelOf } from './fold'
import { isRT, ptrF, vpOf } from './playheads'
import { GOLD } from './samples'
import { LARGE } from './scenarios-large'
import { OPEN, PRES, REFUSE, SAVES, SCENARIOS, TURN } from './scenarios'
import { CONT, PROPS, TLR, UNIT, type OutsideSaveEvent, type Refusal, type SampleTimePosition, type TimelineFrame, type Viewport } from './schema'
import { type Scenario, type SimEvent } from './sim'
import { ok, raw, sim } from './test-world'
import { copyLabel, framePhrase, frameTc, refusalPhrase, saveLine } from './wording'
import { B30, BOOKS, BROOK, FRAMES, SAM } from './world'

const allEv = SCENARIOS.flatMap((sc) => sc.steps.flatMap((st, i) => st.events.map((e) => ({ sc: sc.id, i, e }))))

describe('Rulings (K2 Cores, 2026-11-13)', () => {
  it('#48 agents act, people watch: no step of any scenario has a person as the actor of an edit, a claim or a verb', () => {
    const bad: string[] = []
    const actorIn = (e: SimEvent): unknown => (e.kind === 'frame' ? e.frame.actor : e.kind === 'claim_started' ? e.claim.actor : e.kind === 'refused' ? e.r.actor : null)
    FRAMES.forEach((f) => actorOf(f.actor).kind === 'person' && bad.push('turn rev ' + f.rev))
    GOLD.turn.timeline.forEach((e) => (e.roster_events || []).forEach((r) => r.kind === 'claim_started_elems' && actorOf(r.actor).kind === 'person' && bad.push('turn ' + r.claim)))
    let n = FRAMES.length
    for (const { sc, i, e } of allEv) {
      const a = actorIn(e)
      if (!a) continue
      n++
      if (actorOf(a as string).kind === 'person') bad.push(`${sc} step ${i + 1}`)
    }
    for (const { sc, i, e } of allEv) if (e.kind === 'presence' && actorOf(e.who).kind === 'person' && !['pointer', 'timePosition', 'viewAddress', 'viewport', 'selection', 'away'].includes(e.type)) bad.push(`${sc} step ${i + 1} ${e.type}`)
    ok(!bad.length && n === 39, `${n} looked at; ${bad.slice(0, 4).join('; ')}`)
  })

  it('#49 the turn’s refused verb reads in plain words, with no code; the code is only in the tooltip', () => {
    const r = GOLD.turn.timeline.find((x) => x.event === 'verb_refused')!.frame as unknown as Refusal
    const j = TURN.steps.findIndex((st) => st.events.some((x) => x.kind === 'refused'))
    const S = sim(TURN, j)
    const ph = S.refused ? refusalPhrase(S.refused, modelOf(S.B), S.claims) : null
    const shape = !!(r && r.actor && r.actor.id && r.verb && keyOf(UNIT, r.unit) && /^MPC-E\d{3}$/.test(r.code) && r.holder && r.holder.id && r.claim)
    ok(shape && ph && !/E\d{3}/.test(ph.line + ' ' + ph.sub) && ph.tip.includes(r.code) && ph.line === r._view, `“${ph?.line}”`)
  })

  it('#50 refusals reach viewers with rights only; the refuse scenario is agent against agent', () => {
    const asView: Scenario = { ...TURN, steps: TURN.steps.map((st) => ({ ...st, events: st.events.map((e) => (e.kind === 'welcome' ? { ...e, frame: { ...e.frame, rights: 'view' } } : e)) })) }
    const n = (sc: Scenario): number => raw(sc, sc.steps.length - 1).log.filter((e) => e.kind === 'refused').length
    const c = raw(REFUSE, REFUSE.steps.length - 1).log.flatMap((e) => (e.kind === 'refused' ? [e] : []))
    ok(n(TURN) === 1 && n(asView) === 0 && c.length === 2 && c.every((x) => actorOf(x.r.actor).kind === 'agent' && actorOf(x.r.holder).kind === 'agent'), 'refusals')
  })

  it('#51 every outside-save outcome is ingested, kept_both, declined or missing, never “merged”, all four shown', () => {
    const OUT = ['ingested', 'kept_both', 'declined', 'missing']
    const saves = [...GOLD.turn.timeline.filter((e) => e.event === 'outside_save').map((e) => e.frame as unknown as OutsideSaveEvent), ...allEv.flatMap((x) => (x.e.kind === 'outside_save' ? [x.e.save] : []))]
    const bad = saves.filter((o) => !OUT.includes(o.outcome) || (o.outcome === 'kept_both' && !(o.copy && typeof o.copy.name === 'string' && /\.otio$/.test(o.copy.name) && o.copy.path && o.kept && o.rev)) || (o.outcome === 'ingested' && !o.rev) || (o.outcome === 'declined' && !(o.code && o.message)))
    const seen = [...new Set(saves.map((o) => o.outcome))]
    ok(!bad.length && OUT.every((o) => seen.includes(o as OutsideSaveEvent['outcome'])) && !saves.some((o) => (o.outcome as string) === 'merged') && saves.length === 7, `${saves.length} frames`)
  })

  it('#52 a declined save pauses edits: the view keeps the live version and says why in plain words', () => {
    const S1 = sim(SAVES.find((x) => x.id === 'save-declined')!, 1)
    const ln = S1.banner && saveLine(S1.banner)
    ok(S1.tier === 'live' && S1.B.rev === 35 && S1.paused && S1.banner && S1.banner.outcome === 'declined' && ln && !/live_overlap/.test(ln.l1 + ln.l2) && /Edits are paused/.test(ln.l2), ln ? ln.l2 : '?')
  })

  it('#53 on kept both the timeline changes to the save, the copy is named, and the next change lands after it', () => {
    const sc = SAVES.find((x) => x.id === 'save-both')!
    const S0 = sim(sc, 0)
    const S1 = sim(sc, 1)
    const S2 = sim(sc, sc.steps.length - 1)
    const b = S1.banner
    ok(S1.B !== S0.B && S1.log.some((e) => e.kind === 'frame' && actorOf(e.actor).kind === 'file') && b && b.outcome === 'kept_both' && saveLine(b).l2.includes(copyLabel(b.copy.name)) && S2.B.rev === S1.B.rev + 1 && S2.banner?.outcome === 'kept_both', `rev ${S0.B.rev} → ${S1.B.rev} → ${S2.B.rev}`)
  })

  it('#54 the rate and start timecode come in the first snapshot page: the ruler reads 01:00:00:00 before any clip', () => {
    const roots = GOLD.state.roots.map((r) => r.name)
    const j = OPEN.steps.findIndex((st) => st.events.some((e) => e.kind === 'loading' && e.v && e.v.phase === 'pages'))
    const M = modelOf(sim(OPEN, j).B)
    ok(roots[0] === TLR && !M.T.provisional && M.tc(0n) === '01:00:00:00' && !M.count && j === 2, `step ${j + 1}`)
  })

  it('#55 every built change frame carries each touched clip’s timecode, and the wording reads it from the frame', () => {
    const seen = new Set<TimelineFrame>()
    const all: TimelineFrame[] = []
    const add = (f: TimelineFrame): void => {
      if (!seen.has(f)) {
        seen.add(f)
        all.push(f)
      }
    }
    FRAMES.forEach(add)
    allEv.forEach(({ e }) => e.kind === 'frame' && add(e.frame))
    LARGE.frames.forEach(add)
    const bad: string[] = []
    for (const f of all) for (const u of f.units || []) if (f.derived !== null && !u.ops.includes('delete') && !frameTc(f, u.key)) bad.push(`rev ${f.rev} ${u.key}`)
    const f0 = FRAMES.find((f) => f.verb === 'clip move')!
    const t = clone(f0)
    t.derived!.units![t.units![0].key].tc = '01:09:09:09'
    const ph = framePhrase(t, BOOKS[f0.rev - 1], BOOKS[f0.rev])
    ok(!bad.length && ph.place === '01:09:09:09' && all.length === 22, `${all.length} frames; ${bad.slice(0, 4).join('; ')}`)
  })

  it('#56 labels and kinds are read, never counted: change them in a copy and the lanes follow', () => {
    const B = clone(B30)
    B.derived.containers['@trk:1'] = { kind_label: 'V9', kind: 'video' }
    B.derived.containers['@trk:4'] = { kind_label: 'V3', kind: 'video' }
    const L = lanesOf(modelOf(B)).map((t) => t.label).join(', ')
    const big = modelOf(LARGE.B0).tracks.every((t) => t.labelFrom === 'derived' && t.kindFrom === 'derived')
    ok(L === 'V3, V2, V9, A1' && big, L)
  })

  it('#57 no title text in v1: titles are named generator clips', () => {
    const ip = B30.maps[PROPS[UNIT]]
    const gens = Object.values(modelOf(B30).items).filter((x) => x.kind === 'generator')
    const withText = Object.keys(ip).filter((k) => 'text' in ip[k])
    const tv = [...FRAMES, ...LARGE.frames].filter((f) => /title|text/.test(f.verb || ''))
    ok(GOLD.schema.view.unit_fields.text === null && GOLD.index.units.every((u) => u.text === null) && !withText.length && !tv.length && gens.length === 3 && gens.every((g) => g.name), gens.map((g) => g.name).join(', '))
  })

  it('#58 presence has the kit’s final shape at every step of every scenario', () => {
    const bad: string[] = []
    const n = { pointer: 0, viewport: 0, track: 0 }
    const okP = (p: SampleTimePosition): boolean =>
      /^(0|[1-9]\d*)$/.test(String(p.at)) && typeof p.at === 'string' && ['playing', 'paused'].includes(p.state) && Number.isFinite(p.at_ms) && (p.rate == null || /^[1-9]\d*(\/[1-9]\d*)?$/.test(p.rate)) && Object.keys(p).every((k) => ['at', 'state', 'at_ms', 'in', 'rate'].includes(k) || k.startsWith('_')) && (!p._track || keyOf(CONT, p._track) !== null)
    const okV = (v: Viewport): boolean => isRT(v.from) && isRT(v.to) && v.from.value <= v.to.value && Array.isArray(v.tracks) && v.tracks.length === 2 && v.tracks.every((k) => keyOf(CONT, k) !== null) && !('x' in v)
    const look = (where: string, kind: 'pointer' | 'viewport', v: unknown): void => {
      if (v == null) return
      n[kind]++
      if (kind === 'pointer' && (v as SampleTimePosition)._track) n.track++
      if (!(kind === 'pointer' ? okP(v as SampleTimePosition) : okV(v as Viewport))) bad.push(where + ' ' + kind)
    }
    GOLD.turn.timeline.forEach((e) =>
      (e.roster_events || []).forEach((r) => {
        if (r.kind === 'TimePosition') look('turn ' + r.who, 'pointer', r.timePosition)
        if (r.kind === 'viewport') look('turn ' + r.who, 'viewport', r.viewport)
      }),
    )
    GOLD.pc.points.forEach((P) => P.peers.forEach((q) => (look(P.id + ' ' + q.name, 'pointer', q.timePosition), look(P.id + ' ' + q.name, 'viewport', q.viewport))))
    for (const sc of SCENARIOS)
      sc.steps.forEach((_st, i) => {
        for (const p of Object.values(raw(sc, i).peers)) {
          look(`${sc.id} ${i + 1} ${p.name}`, 'pointer', p.timePosition)
          look(`${sc.id} ${i + 1} ${p.name}`, 'viewport', p.viewport)
        }
      })
    ok(!bad.length && n.pointer && n.viewport && n.track, `${n.pointer} time positions, ${n.track} with a lane; ${bad.slice(0, 4).join('; ')}`)
  })

  it('#59 the view places them: Sam’s playhead and visible range, and Brook (an agent) with a playhead on V2', () => {
    const S = sim(PRES, 1)
    const M = modelOf(S.B)
    const v = vpOf(M, S.peers[SAM].viewport!)
    const f = ptrF(M, S.peers[SAM].pointer)
    const b = S.peers[BROOK].pointer
    ok(f === 706 && v.f0 === 380 && v.f1 === 1480 && v.i0 === 0 && v.i1 === 2 && b && ptrF(M, b) === 708 && b.track === '@trk:2', `Sam at ${f}, sees ${v.f0}–${v.f1}`)
  })
})
