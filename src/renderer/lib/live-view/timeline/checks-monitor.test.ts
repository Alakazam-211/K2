// The mock's model checks 60–89: the monitor and media, the timeline
// display, follow and saves, the pinned snapshots, the agent's playhead,
// timecodes as given, and a hold that ends outside K2.

import { describe, it } from 'vitest'
import { actorOf, EXT_ID } from './actors'
import { applyFrame, keyOf } from './adapter'
import { lanesOf, modelOf, overlaps, type TimelineModel } from './fold'
import { cutsIn, DEFAULT_PV, dfSrcTc, gridF, LANE_SIZES, laneTiles, loopRange, mediaInfo, picModeFor, pictureAt, pictureTiles, playReady, prepOf, sameSpot, srcFrameAt, srcTc, stillAt, stillNote, WAVE_MIN, type Picture, type Still } from './picture'
import { headOf, isRT, ptrF, rtF, verbLanding } from './playheads'
import { GOLD } from './samples'
import { LARGE } from './scenarios-large'
import { GB, GCHANGE } from './scenarios-monitor'
import { CONN, DISPLAY, HOLDGONE, PREP, SCENARIOS, STATES, WATCH } from './scenarios'
import { type Book } from './schema'
import { same, type SimState } from './sim'
import { afterOf, at, beforeOf, ok, sim, X } from './test-world'
import { dfTc, scrollFor, rulerSteps } from './time'
import { framePhrase, frameTc, refusalPhrase } from './wording'
import { ATLAS, B30, BOOKS, FACTS, FRAMES, SAM, SELF } from './world'

const M30 = modelOf(B30)
const pvOf = (S: SimState) => S.pv || DEFAULT_PV
const W = WATCH.frames

describe('Monitor and media (v2)', () => {
  it('#60 every video clip’s source has a poster, a filmstrip and the 640/1280/1920 proxy ladder, found by file name', () => {
    const bad: string[] = []
    for (const M of [M30, modelOf(LARGE.B0)])
      for (const it of Object.values(M.items)) {
        if (it.kind !== 'clip') continue
        const mi = mediaInfo(M, it.media, FACTS)
        if (!mi) {
          bad.push(it.key)
          continue
        }
        const t = M.byT[it.track]
        if (t.kind === 'video' && !(mi.kind === 'video' && mi.poster?.px === 640 && mi.filmstrip?.tile_px === 160 && mi.filmstrip.every_s === 2 && same(mi.proxy?.ladder_px, [640, 1280, 1920]) && /^b3:/.test(mi.hash || ''))) bad.push(it.key)
        if (t.kind === 'audio' && mi.kind === 'video' && mi.poster == null) bad.push(it.key)
      }
    ok(!bad.length && Object.keys(FACTS).length === 80, `${Object.keys(FACTS).length} sources; ${bad.slice(0, 4).join(', ')}`)
  })

  it('#61 cut-only shows the top enabled video track: titles and turned-off clips passed over, a nest’s card, a gap', () => {
    const P = (f: number): Picture => pictureAt(M30, f, null, FACTS)
    const [a, b, c, d, e] = [P(48), P(130), P(1100), P(2200), P(1650)]
    ok(a.kind === 'clip' && a.it?.name === 'Interview A' && a.skipped.some((x) => x.why === 'title') && b.it?.name === 'B-roll 01' && c.it?.name === 'Interview C' && c.skipped.some((x) => x.why === 'off' && x.it.name === 'B-roll 06') && d.kind === 'nest' && e.kind === 'gap', 'pictures')
  })

  it('#62 the player switches source only where the picture changes clip; the title’s end isn’t a switch', () => {
    const cuts = cutsIn(M30, 0, 600, null, FACTS)
    const vb = new Set<number>()
    M30.tracks.filter((t) => t.kind === 'video').forEach((t) => t.items.forEach((x) => (vb.add(x.f0), vb.add(x.f1))))
    ok(cuts.every((c) => vb.has(c)) && !cuts.includes(96) && cuts.includes(120) && cuts.includes(312) && cuts.length === 7, cuts.map((c) => M30.tcF(c).slice(6)).join(', '))
  })

  it('#63 the source timecode comes from the clip’s src', () => {
    const it = M30.items['@item:1']
    ok(srcTc(M30, srcFrameAt(M30, it, 0)) === '14:03:22:10' && srcTc(M30, srcFrameAt(M30, it, 48)) === '14:03:24:10', 'source timecode')
  })

  it('#64 tiles are keyed by media hash, source time and width: a move keeps every tile; an in-trim needs one new head; an end trim none', () => {
    const T = (B: Book, key: string) => {
      const M = modelOf(B)
      return laneTiles(M, M.items[key], 48, 66, FACTS)
    }
    const grid = (M: TimelineModel, x: { head?: boolean; srcF: number; key: string }): boolean => !!x.head || (x.srcF % gridF(M) === 0 && x.key.endsWith('|160'))
    const mv0 = T(beforeOf(W.move), '@item:13').map((x) => x.key)
    const mv1 = T(afterOf(W.move), '@item:13').map((x) => x.key)
    const tr0 = T(beforeOf(W.trim), '@item:2')
    const tr1 = T(afterOf(W.trim), '@item:2')
    const end = FRAMES.find((f) => f.verb === 'clip trim')!
    const e0 = T(BOOKS[end.rev - 1], '@item:1')
    const e1 = T(BOOKS[end.rev], '@item:1')
    ok(same(mv0, mv1) && mv1.length === 3 && tr0[0].key !== tr1[0].key && tr1.every((x) => grid(M30, x)) && e0[0].key === e1[0].key && e1.length < e0.length, 'tile keys')
  })

  it('#65 still at playhead never shows a later frame or one outside the clip', () => {
    let n = 0
    const bad: number[] = []
    for (let f = 0; f < M30.endF; f += 7) {
      const s = stillAt(M30, f, null, FACTS)
      if (s.kind !== 'clip') continue
      n++
      if (s.from === 'tile' && (s.stillF! > s.srcF! || s.stillF! < s.inF! || s.stillF! % gridF(M30))) bad.push(f)
    }
    ok(!bad.length && n > 100, `${n} places; ${bad.slice(0, 4).join(', ')}`)
  })

  it('#66 the monitor reads playheads in rational time: following, Atlas’s; after you scrub, yours', () => {
    const S = X(WATCH, 'follow Atlas')
    const S2 = X(WATCH, 'you scrub')
    ok(ptrF(modelOf(S.B), S.peers[ATLAS].pointer) === 312 && S.local?.follow === true && S2.local?.follow === false && ptrF(modelOf(S2.B), S2.peers[SELF].pointer) === 1010, 'playheads')
  })

  it('#67 who’s watching the same spot: at Atlas’s playhead the monitor’s edge shows Sam', () => {
    const S = X(WATCH, 'sharp')
    const M = modelOf(S.B)
    const f = ptrF(M, S.peers[ATLAS].pointer)!
    const who = sameSpot(M, f, Object.values(S.peers), [ATLAS, S.self], null, FACTS)
    ok(who.length === 1 && who[0].handle === SAM, who.map((p) => p.name).join(', ') || 'nobody')
  })

  it('#68 the watched refusal reads “Atlas couldn’t trim Interview B: Brook is holding it”', () => {
    const S = X(WATCH, 'refused')
    const ph = S.refused && refusalPhrase(S.refused, modelOf(S.B), S.claims)
    ok(ph && ph.line === 'Atlas couldn’t trim Interview B: Brook is holding it' && !/E\d{3}/.test(ph.line), ph ? ph.line : '?')
  })

  it('#69 the watched edits are plain verbs: the in-trim writes geom and src; all keep the lane rule and carry each clip’s timecode', () => {
    const t = W.trim
    const ops = t.ops.map((o) => (o.kind === 'map' ? o.field : '')).sort()
    const ph = framePhrase(t, beforeOf(t), afterOf(t))
    const all = Object.values(W)
    const bad: string[] = []
    all.forEach((f) => overlaps(modelOf(afterOf(f))).forEach((o) => bad.push(`rev ${f.rev} ${o}`)))
    const mk = W.mark.ops.find((o) => o.kind === 'list' && o.root === 'markers')
    ok(same(ops, ['geom', 'src']) && ph.line === 'Atlas trimmed the start of Interview B by 1s · 01:00:14:00' && !bad.length && mk?.key === '@mk:5' && all.every((f) => (f.units || []).every((u) => u.ops.includes('delete') || frameTc(f, u.key))), ph.line)
  })

  it('#70 every monitor state is reached by the states scenario', () => {
    const P = (label: string): Picture => {
      const Y = X(STATES, label)
      const M = modelOf(Y.B)
      return pictureAt(M, ptrF(M, Y.peers[ATLAS].pointer)!, pvOf(Y), FACTS)
    }
    ok(P('no preview').kind === 'nopreview' && P('offline').kind === 'offline' && P('nest').kind === 'nest' && P('gap').kind === 'gap' && P('playing').kind === 'clip' && pvOf(X(STATES, 'still')).player === 'still', 'states')
  })
})

describe('Timeline display (v2)', () => {
  const P = (label: string) => pvOf(X(DISPLAY, label))

  it('#71 auto picks clip pictures by lane height', () => {
    const v = (sz: 'S' | 'M' | 'L') => picModeFor(LANE_SIZES[sz].video, 'auto')
    const a = (sz: 'S' | 'M' | 'L') => LANE_SIZES[sz].audio >= WAVE_MIN
    ok(v('S') === 'head' && v('M') === 'strip' && v('L') === 'strip' && !a('S') && a('M') && a('L') && picModeFor(26, 'auto') === 'off' && picModeFor(26, 'strip') === 'off' && picModeFor(30, 'strip') === 'head' && picModeFor(92, 'off') === 'off' && P('small').lanes === 'S' && P('large').lanes === 'L' && P('head and tail').pic === 'headtail', 'picture modes')
  })

  it('#72 head and tail: an end trim needs a new tail only, an in-trim a new head only, a move none', () => {
    const T = (B: Book, key: string, mode: string) => {
      const M = modelOf(B)
      return pictureTiles(M, M.items[key], mode, 48, 66, FACTS)
    }
    const end = FRAMES.find((f) => f.verb === 'clip trim')!
    const e0 = T(BOOKS[end.rev - 1], '@item:1', 'headtail')
    const e1 = T(BOOKS[end.rev], '@item:1', 'headtail')
    const i0 = T(beforeOf(W.trim), '@item:2', 'headtail')
    const i1 = T(afterOf(W.trim), '@item:2', 'headtail')
    const m0 = T(beforeOf(W.move), '@item:13', 'headtail')
    const m1 = T(afterOf(W.move), '@item:13', 'headtail')
    const nc = Object.values(M30.items).find((x) => x.kind === 'clip' && M30.byT[x.track].kind === 'video' && x.f1 - x.f0 < 96 && x.media)
    const ncT = nc && pictureTiles(M30, nc, 'headtail', 48, 66, FACTS)
    ok(e0.length === 2 && e0[0].key === e1[0].key && e0[1].key !== e1[1].key && i0[0].key !== i1[0].key && i0[1].key === i1[1].key && same(m0, m1) && ncT && ncT.length === 1 && pictureTiles(M30, M30.items['@item:1'], 'head', 48, 66, FACTS).length === 1 && !pictureTiles(M30, M30.items['@item:1'], 'off', 48, 66, FACTS).length, nc ? nc.name : '?')
  })

  it('#73 ruler density: labels at least 110 px apart, minor ticks at least 12 px, the minor step dividing the label step', () => {
    const bad = [0.004, 0.05, 0.42, 0.85, 1.1, 4, 16].filter((p) => {
      const r = rulerSteps(p, 24)
      return (r.label * p < 110 && r.label < 86400) || (r.minor * p < 12 && r.minor !== r.label) || r.label % r.minor
    })
    const r = rulerSteps(0.85, 24)
    ok(!bad.length && r.label / 24 === 10, `at 0.85 px a frame: a label every ${r.label / 24} s, a tick every ${r.minor / 24} s`)
  })

  it('#74 scrolling while playing: page jumps at 95% to land at 5%; centred keeps 50%; paused, nothing scrolls', () => {
    const tw = 1000
    ok(scrollFor('page', 960, 0, tw) === 910 && scrollFor('page', 500, 0, tw) === null && scrollFor('centre', 600, 0, tw) === 100 && scrollFor('centre', 300, 0, tw) === null && scrollFor('page', 190, 300, tw) === 140 && P('playing · page').scroll === 'page' && P('playing · centred').scroll === 'centre' && DEFAULT_PV.scroll === undefined, 'scrolling')
  })
})

describe('Monitor, follow and saves (v2.2)', () => {
  it('#75 the design section’s states are reached: still, head, poster, not a video, other, waiting, stills only, end', () => {
    const L = (label: string) => {
      const Y = X(STATES, label)
      const M = modelOf(Y.B)
      const pv = pvOf(Y)
      const f = ptrF(M, Y.peers[ATLAS].pointer)!
      return { M, pv, p: (pv.player === 'still' ? stillAt(M, f, pv, FACTS) : pictureAt(M, f, pv, FACTS)) as Still }
    }
    const [st, hd, po, im, ot, en, so, wt] = ['still', 'still · head', 'still · poster', 'not a video', 'other', 'end', 'stills only', 'waiting'].map(L)
    ok(st.p.from === 'tile' && /^Nearest picture: \d+\.\d s before$/.test(stillNote(st.M, st.p)) && hd.p.from === 'head' && hd.p.stillF === hd.p.inF && po.p.from === 'poster' && stillNote(po.M, po.p) === 'Clip’s poster' && im.p.kind === 'nomedia' && im.p.mkind === 'Image' && ot.p.kind === 'clip' && ot.p.skipped.length >= 1 && en.pv.mode === 'ended' && so.pv.player === 'still' && so.pv.why === 'decode' && wt.pv.mode === 'stall', stillNote(st.M, st.p))
  })

  it('#76 a loop runs from 1 s before the change to 1 s after, leaves Follow, and offers Back to Atlas', () => {
    const Y = X(WATCH, 'loop it')
    const lp = loopRange(modelOf(Y.B), Y.local!.loop!)
    ok(lp.f0 === 312 - 24 && lp.f1 === 576 + 24 && lp.s0 === 312 && Y.local?.follow === false && Y.local.back === ATLAS, `${lp.f0}–${lp.f1}`)
  })

  it('#77 preparing video: “3 of 15 clips ready”, the ones that can’t be previewed left out; play waits for the next 30 s', () => {
    const L = (label: string) => {
      const Y = X(PREP, label)
      const M = modelOf(Y.B)
      const pv = pvOf(Y)
      const f = ptrF(M, Y.peers[SELF].pointer)!
      return { M, pv, f, P: prepOf(M, pv, FACTS)!, ok: playReady(M, f, pv, FACTS) }
    }
    const [a, b, c, d] = ['preparing', 'filling in', 'preparing this clip', 'video ready'].map(L)
    ok(a.P.n === 3 && a.P.total === PREP.clips.pool && a.P.total === 15 && a.P.cant.length === 2 && !a.ok && b.ok && b.P.n > a.P.n && c.pv.mode === 'stall' && c.pv.why === 'prep' && !c.P.isReady(pictureAt(c.M, c.f, c.pv, FACTS).it!.key) && d.P.done && d.P.total === PREP.clips.pool, `${a.P.n} of ${a.P.total}`)
  })

  it('#78 follow anyone: following Sam reads Sam’s time position and extrapolates from at_ms; when Sam leaves, Follow ends', () => {
    const fs = X(WATCH, 'follow Sam')
    const M = modelOf(fs.B)
    const h = headOf(M, fs.peers[SAM], 730000)
    const h2 = headOf(M, fs.peers[SAM], 731000)
    const sl = X(WATCH, 'Sam left')
    const ph = fs.peers[SAM].timePosition!
    ok(fs.local?.follow === SAM && h && h.playing && h.f === 318 && h2 && Math.round(h2.f) === 342 && same(Object.keys(ph).filter((k) => !k.startsWith('_')).sort(), ['at', 'at_ms', 'rate', 'state']) && typeof ph.at === 'string' && ph.rate === '1' && !sl.peers[SAM] && sl.left?.name === 'Sam' && sl.local?.follow === false, 'follow Sam')
  })

  it('#79 say something carries a reference as an address, and lists as “You → Atlas” with Atlas’s reply', () => {
    const Y = X(WATCH, 'Atlas answers')
    const says = Y.log.flatMap((e) => (e.kind === 'say' ? [e] : []))
    const q = says[0]
    ok(says.length === 2 && q.actor === SELF && q.to === ATLAS && q.ref && keyOf('items', q.ref.item) && isRT(q.ref.at) && rtF(q.ref.at, modelOf(Y.B)) === 336 && says[1].reply && actorOf(says[1].actor).kind === 'agent', 'say')
  })

  it('#80 kept both, as the kit does it: the timeline changes to the save, then {outcome, rev, kept, copy: {name, path}}', () => {
    const j = at(WATCH, 'outside save')
    const S0 = sim(WATCH, j - 1)
    const S1 = sim(WATCH, j)
    const e = S1.log.filter((x) => x.kind === 'frame').pop()!
    const M0 = modelOf(S0.B)
    const M1 = modelOf(S1.B)
    const b = S1.banner
    ok(actorOf(e.actor).id === EXT_ID && b?.outcome === 'kept_both' && M1.items['@item:13'].name === 'B-roll 03' && M0.items['@item:13'].name === 'Harbour at dawn' && M1.items['@item:2'].f0 !== M0.items['@item:2'].f0 && b.rev === S1.B.rev && String(b.kept).endsWith('@' + S0.B.rev) && b.copy.path && !('doc' in b.copy), 'kept both')
  })

  it('#81 a lost connection keeps the view: Watching, Changes and your playhead stay; it then catches up or says it can’t', () => {
    const [a, b, c, d] = ['live', 'reconnecting', 'caught up', 'can’t reconnect'].map((l) => X(CONN, l))
    ok(b.conn === 'reconnecting' && Object.keys(b.peers).length === Object.keys(a.peers).length && b.log.length === a.log.length && same(b.peers[SELF].pointer, a.peers[SELF].pointer) && c.conn === null && c.B.rev === a.B.rev + 1 && d.conn === 'failed', `rev ${c.B.rev}`)
  })

  it('#82 the refusal frame has the kit’s real fields', () => {
    const ev = WATCH.steps.find((s) => s.label === 'refused')!.events[0]
    const keys = ev.kind === 'refused' ? Object.keys(ev.r).filter((k) => !k.startsWith('_')).sort() : []
    ok(same(keys, ['actor', 'at_ms', 'claim', 'client', 'code', 'holder', 'turn', 'unit', 'verb']), keys.join(', '))
  })
})

describe('Pinned snapshots (v2.3)', () => {
  const L = (B: Book): string => {
    const M = modelOf(B)
    return JSON.stringify({ t: M.tracks.map((t) => [t.key, t.kind, t.title, t.enabled, t.muted]), i: Object.values(M.items).map((x) => [x.key, x.kind, x.track, x.geom, x.name, x.off, x.src == null ? null : String(x.src), x.retimed]).sort(), k: M.markers.map((m) => [m.key, m.name, String(m.at), m.owner]).sort() })
  }

  it('#83 the pinned change frame takes the pinned cut to the pinned trim-in state, read exactly as pinned', () => {
    ok(L(applyFrame(GB.cut, GCHANGE)) === L(GB.trim) && same(GCHANGE.ops.map((o) => (o.kind === 'map' ? o.field : null)).sort(), ['geom', 'src']), `rev ${GCHANGE.rev}`)
  })

  it('#84 the player’s fields read as pinned: last on top, enabled, not muted, the end is the latest item end, origins a range, nothing retimed', () => {
    const M = modelOf(GB.cut)
    const L2 = lanesOf(M)
    const E = modelOf(GB.edited)
    const end = Math.max(...Object.values(M.items).map((x) => x.f1))
    ok(M.T.stacking === 'last_on_top' && M.T.endFrom === 'items' && L2[0].title === 'V2' && L2[1].title === 'V1' && M.tracks.every((t) => t.enabled && !t.muted) && M.endF === end && Object.values(M.media).every((m) => m.origin === 'range') && Object.values(M.items).every((x) => !x.retimed) && E.count === M.count && !!E.items['@item:7'] && !E.items['@item:6'], L2.map((t) => t.title).join(', '))
  })

  it('#85 the sample carries the same fields: A2 (Music) muted and flagged, Interview B.mov of unknown origin', () => {
    const a2 = M30.tracks.find((t) => t.title === 'Music')
    const ib = Object.values(M30.media).find((m) => m.file === 'Interview B.mov')
    ok(a2 && a2.muted && a2.enabled && ib && ib.origin === 'unknown' && M30.tracks.every((t) => typeof t.enabled === 'boolean'), 'sample fields')
  })
})

describe('The agent’s playhead after its verbs (0634ee32)', () => {
  const seen: Record<string, number> = {}
  const bad: string[] = []
  const refusedBad: string[] = []
  for (const sc of SCENARIOS)
    sc.steps.forEach((st, i) => {
      for (const e of st.events) {
        if (e.kind !== 'frame' || !e.frame.verb || actorOf(e.frame.actor).kind !== 'agent') continue
        const S = sim(sc, i)
        const lg = S.log.filter((x) => x.kind === 'frame' && x.frame === e.frame).pop()
        if (!lg || lg.kind !== 'frame') continue
        const L = verbLanding(e.frame, lg.before, lg.after)
        const p = S.peers[actorOf(e.frame.actor).id]
        const tp = p && p.timePosition
        const M1 = modelOf(lg.after)
        const it1 = e.frame.verb === 'clip trim' ? M1.items[e.frame.units![0].key] : undefined
        const kind = e.frame.verb === 'clip trim' ? (L && it1 && L.at === String(BigInt(Math.round(it1.f1)) * M1.FR) ? 'trim --out' : 'trim --in') : e.frame.verb
        seen[kind] = (seen[kind] || 0) + 1
        if (!L || !tp || tp.at !== L.at || tp.state !== 'paused' || 'rate' in tp || (tp._track || null) !== (L.track || null)) bad.push(`${sc.id}:${i} ${kind}`)
      }
      const r = st.events.find((e) => e.kind === 'refused')
      if (r && r.kind === 'refused') {
        const h = actorOf(r.r.actor).id
        const a = i ? sim(sc, i - 1).peers[h] : null
        const b = sim(sc, i).peers[h]
        if (a && b && !same(a.timePosition, b.timePosition)) refusedBad.push(`${sc.id}:${i}`)
        seen.refused = (seen.refused || 0) + 1
      }
    })

  it('#86 every agent verb in every scenario puts its playhead where verbs.rs says: paused, in ticks, no rate, in the verb’s track', () => {
    ok(!bad.length && seen['trim --out'] && seen['trim --in'] && seen['clip move'] && seen['marker add'], `${JSON.stringify(seen)}; ${bad.slice(0, 4).join('; ')}`)
  })

  it('#87 a refused verb moves nothing', () => ok(!refusedBad.length && seen.refused === 4, `${seen.refused} refusals; ${refusedBad.join('; ')}`))
})

describe('Timecodes as given (v2.5)', () => {
  it('#88 drop-frame: a 29.97 DF source shows its timecode in the “;” form; 10 min of DF frames is 00:10:00;00', () => {
    const Y = X(STATES, 'drop frame')
    const M = modelOf(Y.B)
    const p = pictureAt(M, ptrF(M, Y.peers[ATLAS].pointer)!, pvOf(Y), FACTS)
    const t = p.mi ? dfSrcTc(M, p) : null
    ok(p.kind === 'clip' && p.mi?.dropFrame && /^\d\d:\d\d:\d\d;\d\d$/.test(t || '') && t === '01:00:10;17' && dfTc(17982) === '00:10:00;00' && dfTc(1800) === '00:01:00;02' && M.T.rate && GOLD.golden.cut.state.timeline.drop_frame === '0', String(t))
  })
})

describe('Holds (v2.6, kit 678ea114)', () => {
  it('#89 a hold on a track renamed outside K2 ends as the kit sends it, and the view names the track as it was', () => {
    const S = sim(HOLDGONE, 1)
    const e = S.log.filter((x) => x.kind === 'claim_end').pop()
    const M = modelOf(S.B)
    ok(e && e.kind === 'claim_end' && e.how === 'gone_outside_save' && same(e.keys, ['@trk:4']) && e.was === 'Music' && M.byT['@trk:4'].title === 'Score' && !S.claims.some((c) => c.id === 'claim-60'), `was ${e && e.kind === 'claim_end' ? e.was : '?'}`)
  })
})
