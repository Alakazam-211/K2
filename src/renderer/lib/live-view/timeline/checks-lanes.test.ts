// The mock's model checks 18–47: lanes and edits, the wire and the kit
// client, outside saves, wording, and the large timeline.

import { describe, it } from 'vitest'
import { actorOf, EXT_ID } from './actors'
import { bookFromPages, keyOf } from './adapter'
import { entryCount, modelOf, overlaps, visibleItems } from './fold'
import { GOLD } from './samples'
import { LARGE } from './scenarios-large'
import { SAVES, TURN } from './scenarios'
import { MKR, PROPS, TLR, UNIT, type TimelineFrame } from './schema'
import { same, touchedOf, type SimEvent } from './sim'
import { afterOf, beforeOf, ok, raw, sim } from './test-world'
import { pairOf } from './time'
import { copyLabel, framePhrase, saveLine } from './wording'
import { B30, B35, BOOKS, FRAMES, FRT, LAST_REV, MISFITS, SAVED_B } from './world'

const LIM = GOLD.schema.limits

describe('Lanes and edits (plan §1, §3)', () => {
  it('#18 no two enabled items overlap on a track outside a transition, at every rev (live_overlap)', () => {
    const bad: string[] = []
    for (const [rv, B] of Object.entries(BOOKS)) overlaps(modelOf(B)).forEach((o) => bad.push(`rev ${rv} ${o}`))
    ok(!bad.length, bad.join('; '))
  })

  it('#19 the items list’s order means nothing: the lane sorts on read', () => {
    const M = modelOf(BOOKS[LAST_REV - 1])
    const list = M.B.lists.items.map((x) => x[0]).filter((k) => M.items[k].track === '@trk:2')
    const lane = M.byT['@trk:2'].items.map((x) => x.key)
    const sorted = lane.slice().sort((a, b) => (M.items[a].s < M.items[b].s ? -1 : 1))
    ok(!same(list, lane) && same(lane, sorted) && lane.indexOf('@item:15') < lane.indexOf('@item:14'), 'B-roll 05 before B-roll 04 in time, not in the list')
  })

  it('#20 clip move is one field set (geom), no list move', () => {
    const mv = FRAMES.filter((f) => f.verb === 'clip move')
    ok(mv.length === 2 && mv.every((f) => f.ops.length === 1 && f.ops[0].kind === 'map' && f.ops[0].field === 'geom' && !f.ops.some((o) => o.kind === 'list')), 'moves')
  })

  it('#21 clip trim writes geom (and src for an in-trim) only; rev 31 keeps the start and shortens by 1.5s', () => {
    const tr = FRAMES.filter((f) => f.verb === 'clip trim')
    const all = tr.every((f) => f.ops.every((o) => o.kind === 'map' && (o.field === 'geom' || o.field === 'src')))
    const o = tr[0].ops[0]
    const a = pairOf(o.before)!
    const b = pairOf(o.after)!
    ok(all && a[0] === b[0] && b[1] < a[1] && modelOf(B30).dur(Number((a[1] - b[1]) / FRT)) === '1.5s', 'trim')
  })

  it('#22 keys are minted past the mark inside the change', () => {
    const f = FRAMES.find((x) => x.verb === 'marker add')!
    const ins = f.ops.find((o) => o.kind === 'list' && o.root === MKR)
    const mk = f.ops.find((o) => o.root === TLR && o.key === 'mark/k')
    const prev = +(modelOf(BOOKS[f.rev - 1]).T.marks.k || 0)
    ok(ins && keyOf(MKR, ins.key) === 'k' + (prev + 1) && mk && mk.after === String(prev + 1), `${ins?.key}`)
  })

  it('#23 marks are never lowered: the outside save’s copy keeps mark/k; the large timeline’s add mints past the mark', () => {
    const a = B35.maps[TLR]['mark/k']['']
    const b = bookFromPages({ upto: 0, roots: GOLD.saved.roots }).maps[TLR]['mark/k']['']
    const f = LARGE.frames.find((x) => x.verb === 'clip add')!
    const ins = f.ops.find((o) => o.kind === 'list')!
    ok(a === b && ins.key === LARGE.picks.nk && afterOf(f).maps[TLR]['mark/i'][''] === keyOf(UNIT, ins.key)!.slice(1), `${ins.key}`)
  })

  it('#24 clip delete is a lift: one list delete, the gap stays, its neighbours unmoved', () => {
    const f = LARGE.frames.find((x) => x.verb === 'clip delete')!
    const d = f.ops.filter((o) => o.kind === 'list')
    const M0 = modelOf(afterOf(f))
    const gone = LARGE.picks.dl
    ok(d.length === 1 && d[0].op === 'delete' && !M0.items[gone.key] && !overlaps(M0).length, `${M0.dur(gone.f1 - gone.f0)} on ${modelOf(LARGE.B0).byT[gone.track].label}`)
  })

  it('#25 the outside save is change_kind external from kit:external (colour null → K2’s grey, “Saved outside K2”), no turn, no verb', () => {
    const ev = SAVES.find((x) => x.id === 'save-ingested')!.steps[1].events.find((e) => e.kind === 'frame') as Extract<SimEvent, { kind: 'frame' }>
    const f = ev.frame
    ok(f.actor.kind === 'file' && f.actor.colour === null && f.turn === null && f.verb === null && actorOf(f.actor).name === 'Saved outside K2', 'the outside save')
  })

  it('#26 media paths never reach the wire: every media’s file is a last component', () => {
    const ms = Object.values(modelOf(B30).media)
    ok(!ms.filter((m) => /[\\/]/.test(m.file || '')).length && ms.length === 14, `${ms.length} media`)
  })
})

describe('Wire and kit client', () => {
  it('#27 frames numbered without a gap and every op fits the state as KeyedWatch applies it (no resync)', () => {
    const seqs = FRAMES.map((f) => f.seq)
    const revs = FRAMES.map((f) => f.rev)
    ok(seqs.every((s, i) => !i || s === seqs[i - 1] + 1) && revs.every((r, i) => r === GOLD.state.upto + 1 + i) && !MISFITS.length, MISFITS.join('; '))
  })

  it('#28 snapshot pages in the schema’s root order; composite maps named in the head', () => {
    const roots = GOLD.state.roots.map((r) => r.name)
    const KS = GOLD.schema.keyed
    ok(same(roots, KS.roots.map((r) => r.name)) && KS.composite.every((c) => roots.includes(c.map) && roots.includes(c.list)), roots.join(', '))
  })

  it('#29 each UnitDelta move_resize carries the geom before and after, equal to the state on either side', () => {
    const bad: string[] = []
    for (const f of FRAMES)
      for (const u of f.units || []) {
        if (!u.ops.includes('move_resize')) continue
        const a = BOOKS[f.rev - 1].maps[PROPS[UNIT]][u.key]
        const b = BOOKS[f.rev].maps[PROPS[UNIT]][u.key]
        if (u.geom_before !== a.geom || u.geom_after !== b.geom) bad.push(`rev ${f.rev} ${u.key}`)
      }
    ok(!bad.length, bad.join(', '))
  })

  it('#30 the kit’s agent rule: after each verb the agent’s focus is the touched clip’s track and its selection what it touched', () => {
    const S = raw(TURN, TURN.steps.length - 1)
    const bad = S.ruleChecks.filter((rc) => rc.diff.length).map((rc) => `rev ${rc.rev}: ${rc.diff.join(', ')}`)
    ok(!bad.length && S.ruleChecks.length >= 5, `${S.ruleChecks.length} flushes; ${bad.join('; ')}`)
  })

  it('#31 frame.claim names the actor’s own claim exactly when the change touched a clip it held, else null', () => {
    let claims: { claim?: string; actor?: { id: string }; keys?: string[] }[] = []
    const bad: string[] = []
    for (const e of GOLD.turn.timeline) {
      for (const r of e.roster_events || []) {
        if (r.kind === 'claim_started_elems') claims.push(r)
        if (r.kind === 'claim_ended') claims = claims.filter((c) => c.claim !== r.claim)
      }
      if (e.event === 'ops_change') {
        const f = FRAMES.find((x) => x.rev === e.rev)!
        const hit = claims.filter((c) => c.actor?.id === f.actor.id).find((c) => touchedOf(f).some((t) => (c.keys || []).includes(t.key)))
        if ((hit ? hit.claim : null) !== f.claim) bad.push(`rev ${f.rev}`)
      }
    }
    ok(!bad.length, bad.join(', '))
  })

  it('#32 claims and selections are keys of one list root (items), at most 256, a note at most 1,024 bytes', () => {
    const evs = GOLD.turn.timeline.flatMap((e) => e.roster_events || [])
    const cl = evs.filter((r) => r.kind === 'claim_started_elems')
    const sel = evs.filter((r) => r.kind === 'selection') as unknown as { selection: { keys: string[] } }[]
    ok(cl.every((c) => (c.keys || []).length <= 256 && new TextEncoder().encode(c.note || '').length <= 1024 && c.root === UNIT && (c.keys || []).every((k) => keyOf(UNIT, k))) && sel.every((s) => s.selection.keys.length <= 256), 'limits')
  })

  for (const P of GOLD.pc.points)
    it(`#${32 + GOLD.pc.points.indexOf(P) + 1} presence ${P.id} (rev ${P.rev}, ${P._at}): focus, selection, viewport, playhead, mode, isSelf, claims`, () => {
      const i = TURN.steps.findIndex((s) => s.point === P.id)
      const S = sim(TURN, i)
      const bad: string[] = []
      if (S.B.rev !== P.rev) bad.push(`rev ${S.B.rev}≠${P.rev}`)
      for (const q of P.peers) {
        const s = S.peers[q.handle]
        if (!s) {
          bad.push(q.name + ' missing')
          continue
        }
        for (const k of ['focus', 'selection', 'viewport'] as const) if (!same(s[k], q[k])) bad.push(`${q.name} ${k}`)
        const ta = s.timePosition
        const tb = q.timePosition
        if (!!ta !== !!tb || (ta && tb && (ta.at !== tb.at || ta.state !== tb.state))) bad.push(`${q.name} timePosition`)
        if (s.mode !== q.mode_derived) bad.push(`${q.name} mode`)
        if (s.isSelf !== q.isSelf) bad.push(q.name + ' isSelf')
      }
      if (!same(S.claims.map((c) => c.id), P.claims_active.map((c) => c.id))) bad.push('claims')
      ok(i >= 0 && !bad.length, `differ: ${bad.join(', ')}`)
    })
})

describe('Outside saves (K-B3)', () => {
  const j = TURN.steps.findIndex((st) => st.events.some((e) => e.kind === 'outside_save'))
  const L = (B: typeof B30): string => {
    const M = modelOf(B)
    return JSON.stringify({ t: M.tracks.map((t) => [t.key, t.kind, t.title]), i: Object.values(M.items).map((x) => [x.key, x.kind, x.track, x.geom, x.name, x.off, x.src == null ? null : String(x.src)]).sort(), k: M.markers.map((m) => [m.key, m.name, String(m.at), m.owner]).sort() })
  }

  it('#36 kept both: the save’s change frame comes first from kit:external, then {outcome, rev, kept, copy}; what was live is kept and the copy is named', () => {
    const S0 = raw(TURN, j - 1)
    const S1 = raw(TURN, j)
    const kb = S1.banner
    const ext = S1.log.filter((e) => e.kind === 'frame').pop()
    const ln = kb && saveLine(kb)
    ok(
      S0.B.rev === 35 && S1.B.rev === 36 && L(S1.B) === L(SAVED_B) && L(S1.B) !== L(S0.B) && ext && actorOf(ext.actor).id === EXT_ID && kb && kb.outcome === 'kept_both' && kb.rev === 36 && /@35$/.test(String(kb.kept)) && ln && ln.l2.includes(copyLabel(kb.copy.name)),
      `${ln ? ln.l1 + ' ' + ln.l2 : '?'}`,
    )
  })

  it('#37 the kit wrote the rev before, so a differing save keeps both, without copy.doc: Open the copy works from the path', () => {
    const h = GOLD.turn.timeline.find((e) => e.event === 'outside_save')!.frame as { outcome: string; rev: number; copy: { name: string; path: string } }
    const w = GOLD.turn.timeline.find((e) => e.event === 'host' && e.host?.type === 'written')!.host!
    ok(h.outcome === 'kept_both' && w.rev === h.rev - 1 && same(Object.keys(h).filter((k) => !k.startsWith('_')).sort(), ['copy', 'kept', 'outcome', 'rev']) && /^rough-cut \(conflict [^)]+ \d{4}-\d\d-\d\d \d{6}\)\.otio$/.test(h.copy.name) && h.copy.path.endsWith(h.copy.name) && !('doc' in h.copy), h.copy.path)
  })
})

describe('Wording (K2 writes it)', () => {
  FRAMES.forEach((f, n) =>
    it(`#${38 + n} rev ${f.rev}: “${f._view}”`, () => {
      const p = framePhrase(f, BOOKS[f.rev - 1], BOOKS[f.rev])
      ok(p.line === f._view, `“${p.line}” ≠ “${f._view}”`)
    }),
  )

  it('#43 every timecode in the wording is HH:MM:SS:FF at the timeline’s rate', () => {
    const lines = [...FRAMES.map((f) => framePhrase(f, BOOKS[f.rev - 1], BOOKS[f.rev]).line), ...LARGE.frames.map((f: TimelineFrame) => framePhrase(f, beforeOf(f), afterOf(f)).line)]
    const tcs = lines.join(' ').match(/\d\d:\d\d:\d\d[:;]\d\d/g) || []
    ok(tcs.length >= 5 && tcs.every((t) => /^\d\d:\d\d:\d\d:\d\d$/.test(t) && +t.slice(9) < 24), `${tcs.length} seen`)
  })
})

describe('Large timeline (plan §5)', () => {
  const M = modelOf(LARGE.B0)

  it('#44 2,200 items, 120 markers, 10 tracks: within LiveLimits, and the keyed entries under the kit’s limit', () => {
    const n = entryCount(LARGE.B0)
    ok(M.count >= 2000 && M.count <= LIM.max_items && M.markers.length <= LIM.max_markers && M.tracks.length <= LIM.max_tracks && n <= LIM.keyed_max_entries && n === 21130, `${M.count} items, ${n} entries`)
  })

  it('#45 lanes virtualise: for three windows the binary search finds exactly what a full scan finds', () => {
    const wins = [
      [0, 2000],
      [LARGE.P0, LARGE.P0 + 1400],
      [M.endF - 600, M.endF + 10],
    ]
    const bad: string[] = []
    let drawn = 0
    for (const [a, b] of wins)
      for (const t of M.tracks) {
        const v = visibleItems(t, a, b).map((x) => x.key)
        const brute = t.items.filter((x) => x.f0 < b && x.f1 > a).map((x) => x.key)
        drawn += v.length
        if (!same(v, brute)) bad.push(`${t.label} ${a}`)
      }
    ok(!bad.length && drawn < 400, `${drawn} drawn; ${bad.join(', ')}`)
  })

  it('#46 the generated timeline and each of its changes keep the lane rule', () => {
    const bad: string[] = []
    for (const f of LARGE.frames) overlaps(modelOf(afterOf(f))).forEach((o) => bad.push(`rev ${f.rev} ${o}`))
    ok(!bad.length && !overlaps(M).length && LARGE.frames.length === 5, bad.join('; '))
  })

  it('#47 the sample turn’s doc: 33 items, within the limits', () => {
    const n = entryCount(B30)
    ok(n <= LIM.keyed_max_entries && modelOf(B30).count <= LIM.max_items && modelOf(B30).count === 33 && n === 502, `${modelOf(B30).count} items, ${n} entries`)
  })
})
