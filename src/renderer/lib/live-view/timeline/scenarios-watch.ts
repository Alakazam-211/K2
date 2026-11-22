// The tab, watched: one agent's edits seen through the monitor (built from
// rev 30), then the connection, the timeline display and a hold that ends
// outside K2, each starting from a step of the watch.

import { actorOf } from './actors'
import { clone } from './adapter'
import { builtOf, geomSet, mkFrame } from './diff'
import { CONT, MKR, PROPS, TLR, UNIT, type Book, type Claim, type Selection, type TimelineFrame } from './schema'
import { ensurePeer, simulateRaw, step, type Scenario, type SimEvent, type SimState } from './sim'
import { big } from './time'
import { ATLAS, B30, blank, BROOK, CTX, EXT, FRT, pres, ptr, rt, SAM, SELF, vpt } from './world'
import { PRODUCT_NAME } from '@/lib/brand'

export type WatchScenario = Scenario & { frames: Record<string, TimelineFrame>; claims: { c51: Claim; c52: Claim } }

const fr = (f: TimelineFrame): SimEvent => ({ kind: 'frame', frame: f })
const pv = (v: Record<string, unknown>): SimEvent => ({ kind: 'pv', v })
const local = (v: Record<string, unknown>): SimEvent => ({ kind: 'local', v })

function buildWatch(): WatchScenario {
  const TURNID = 't:5b1e07c2d94a6f30.1'
  const IP = PROPS[UNIT]
  const tk = (f: number): string => String(BigInt(f) * FRT)
  let B: Book = B30
  let rev = B30.rev
  let at = 400000 // changes 45 s apart, so Changes can say when each happened
  const F: Record<string, TimelineFrame> = {}
  const push = (id: string, actor: string, verb: string, mut: (X: Book) => void, claim?: string): void => {
    at += 45000
    const f = mkFrame(B, ++rev, at, actor, verb, mut, { turn: TURNID, client: 4711, claim: claim || null })
    B = builtOf(f)!.after
    F[id] = f
  }
  push('trim', ATLAS, 'clip trim', (X) => {
    const p = X.maps[IP]['@item:2']
    p.geom = `${tk(336)},${tk(240)}`
    p.src = String(big(p.src)! + 24n * FRT)
  }, 'claim-51')
  push('move', ATLAS, 'clip move', (X) => geomSet(X, '@item:13', tk(336), tk(120)), 'claim-51')
  push('rename', ATLAS, 'clip rename', (X) => {
    X.maps[IP]['@item:13'].name = 'Harbour at dawn'
  }, 'claim-51')
  push('off', ATLAS, 'clip enable', (X) => {
    X.maps[IP]['@item:13'].off = '1'
  }, 'claim-51')
  push('mark', ATLAS, 'marker add', (X) => {
    X.lists[MKR].push(['@mk:5', 'marker'])
    X.maps[PROPS[MKR]]['@mk:5'] = { owner: '', at: tk(336), dur: '0', name: 'Check the cut', note: 'B-roll or interview here?', color: 'GREEN' }
    X.maps[TLR]['mark/k'][''] = '5'
  })
  // what an outside app saves later: the file as it was before Atlas's edits
  F.saved = mkFrame(B, rev + 1, 790000, EXT, null, (X) => {
    X.lists = clone(B30.lists)
    X.maps = clone(B30.maps)
    X.hidden = clone(B30.hidden || {})
  })
  const atl = (f: number, track: string, sel: Selection): SimEvent[] => [pres(ATLAS, 'pointer', ptr(f, track)), pres(ATLAS, 'focus', { root: CONT, key: track }), pres(ATLAS, 'selection', sel)]
  const c51: Claim = { id: 'claim-51', actor: clone(actorOf(ATLAS)), root: UNIT, keys: ['@item:2', '@item:13'], note: 'tightening the interview open', hold: true, started_ms: 420000, expires_ms: 720000 }
  const c52: Claim = { id: 'claim-52', actor: clone(actorOf(BROOK)), root: UNIT, keys: ['@item:2'], note: 'checking the funding line', hold: true, started_ms: 640000, expires_ms: 940000 }
  const refused: SimEvent = { kind: 'refused', r: { at_ms: 660000, actor: clone(actorOf(ATLAS)), client: 4711, turn: TURNID, verb: 'clip trim', unit: '@item:2', code: 'MPC-E301', holder: clone(actorOf(BROOK)), claim: 'claim-52' } }
  const base = (): SimState => {
    const S = blank(B30)
    Object.assign(ensurePeer(S, SELF), { pointer: ptr(0, null), viewport: vpt(0, 1400, '@trk:2', '@trk:4') })
    Object.assign(ensurePeer(S, SAM), { pointer: ptr(1500, '@trk:1'), viewport: vpt(1100, 2400, '@trk:2', '@trk:3') })
    ensurePeer(S, ATLAS)
    ensurePeer(S, BROOK)
    return S
  }
  const A = (ms: number) => ({ at: ms })
  const sel = (keys: string[]): Selection => ({ root: UNIT, keys })
  const say = (o: Omit<Extract<SimEvent, { kind: 'say' }>, 'kind'>): SimEvent => ({ kind: 'say', ...o })
  const breath = 'Can you keep the cut on the breath?'
  return {
    id: 'watch',
    group: 'The tab',
    title: 'Watching an edit: follow Atlas, a trim, a loop, a refusal, say something, follow Sam, an outside save',
    base,
    frames: F,
    claims: { c51, c52 },
    steps: [
      step('play', [local({ follow: false }), pv({ mode: 'play' }), pres(SELF, 'pointer', ptr(40, null))], 'You open rough-cut and press Space: the monitor plays the top enabled video track, cuts only.', A(380000)),
      step('cut', [pres(SELF, 'pointer', ptr(128, null))], 'At 01:00:05:00 B-roll 01 starts on V2: the player switches to its proxy at the cut.', A(390000)),
      step('Atlas holds', [{ kind: 'claim_started', claim: c51 }, ...atl(312, '@trk:1', sel(['@item:2', '@item:13']))], 'Atlas holds Interview B and B-roll 03 and parks its playhead at Interview B.', A(421000)),
      step('follow Atlas', [local({ follow: true }), pv({ mode: 'seek', rung: 640 })], 'You press Follow Atlas: the monitor jumps to Atlas’s playhead.', A(430000)),
      step('sharp', [pv({ mode: 'pause', rung: 1920 }), pres(SAM, 'pointer', ptr(318, '@trk:1')), pres(SAM, 'viewport', vpt(0, 1300, '@trk:2', '@trk:4'))], 'The exact frame is up. Sam comes to the same spot.', A(440000)),
      step('trim', [fr(F.trim), ...atl(336, '@trk:1', sel(['@item:2'])), pv({ pending: ['@item:2'] })], 'Atlas trims the start of Interview B by 1s.', A(447000)),
      step('tile in', [pv({ pending: [] })], 'The head tile lands.', A(452000)),
      step('loop it', [local({ follow: false, back: ATLAS, loop: { f0: 312, f1: 576, rev: F.trim.rev, who: ATLAS, verb: 'clip trim' } }), pv({ mode: 'play' })], 'You click the trim’s bracket: the loop runs from 1 s before the change to 1 s after.', A(470000)),
      step('move', [local({ follow: true }), pv({ mode: 'pause' }), fr(F.move), ...atl(336, '@trk:2', sel(['@item:13']))], 'Back to Atlas: Atlas moves B-roll 03 to 01:00:14:00.', A(493000)),
      step('rename', [fr(F.rename), ...atl(336, '@trk:2', sel(['@item:13']))], 'Atlas renames B-roll 03 to “Harbour at dawn”.', A(538000)),
      step('turn off', [fr(F.off), ...atl(336, '@trk:2', sel(['@item:13']))], 'Atlas turns off Harbour at dawn: the monitor falls through to Interview B on V1.', A(583000)),
      step('marker', [fr(F.mark), pres(ATLAS, 'pointer', ptr(336, null)), pres(ATLAS, 'selection', { root: MKR, keys: ['@mk:5'] })], 'Atlas drops a marker, “Check the cut”, at its playhead.', A(628000)),
      step('Atlas lets go', [{ kind: 'claim_ended', id: 'claim-51', how: 'released', at_ms: 635000, at_rev: F.mark.rev, actor: actorOf(ATLAS) }], 'Atlas lets go of its hold.', A(636000)),
      step('Brook holds', [{ kind: 'claim_started', claim: c52 }, pres(BROOK, 'focus', { root: CONT, key: '@trk:1' }), pres(BROOK, 'selection', sel(['@item:2']))], 'Brook holds Interview B to check the funding line.', A(642000)),
      step('refused', [refused], 'Atlas tries to trim Interview B again and is refused: Brook is holding it.', A(661000)),
      step('you scrub', [local({ follow: false, back: ATLAS }), pres(SELF, 'pointer', ptr(1010, '@trk:1')), pv({ mode: 'seek', rung: 640 })], 'You drag the playhead to Interview C: following stops.', A(680000)),
      step('back to Atlas', [local({ follow: true }), pv({ mode: 'pause', rung: 1920 })], 'You press Back to Atlas.', A(690000)),
      step('say something', [say({ from: SELF, to: ATLAS, at_ms: 700000, text: 'Is B-roll 03 better over this line?', ref: { at: rt(336), item: '@item:2', track: '@trk:1' } })], 'You say something to Atlas, pointing at the time and the clip.', A(701000)),
      step('Atlas answers', [say({ from: ATLAS, to: SELF, at_ms: 712000, text: 'On it: I’ll try it there once Brook lets go of Interview B.', reply: true })], 'Atlas answers.', A(713000)),
      step('not sent', [say({ from: SELF, to: ATLAS, at_ms: 720000, text: breath, failed: true, why: `it didn’t reach ${PRODUCT_NAME}` })], 'A second message doesn’t go through; the words are kept.', A(721000)),
      step('not sent again', [say({ from: SELF, to: ATLAS, at_ms: 722000, text: breath, failed: true, why: `it still didn’t reach ${PRODUCT_NAME}`, tries: 2 })], 'Send again fails too.', A(723000)),
      step('sent again', [say({ from: SELF, to: ATLAS, at_ms: 724000, text: breath })], 'Send again goes through.', A(725000)),
      step('follow Sam', [local({ follow: SAM }), pres(SAM, 'timePosition', { at: String(318n * FRT), state: 'playing', at_ms: 730000, rate: '1' }), pv({ mode: 'play' })], 'You follow Sam, who is playing.', A(730000)),
      step('Sam left', [{ kind: 'leave', who: SAM }, local({ follow: false, left: 'Sam' }), pres(SELF, 'pointer', ptr(560, '@trk:1')), pv({ mode: 'pause' })], 'Sam closes the file: Follow ends quietly.', A(760000)),
      step('outside save', [fr(F.saved), { kind: 'outside_save', save: { _at_ms: 790500, outcome: 'kept_both', rev: F.saved.rev, kept: `sv:5d1c0a7e93b24f68@${F.mark.rev}`, copy: { name: 'rough-cut (conflict Atlas 2026-11-13 142210).otio', path: 'edits/rough-cut (conflict Atlas 2026-11-13 142210).otio' } } }], 'An editing app saves rough-cut over the file: the kit keeps both.', A(791000)),
    ],
  }
}

export const WATCH = buildWatch()
const jTileIn = WATCH.steps.findIndex((s) => s.label === 'tile in')
const fromWatch = (): SimState => {
  const S = simulateRaw(WATCH, jTileIn, CTX)
  S.recent = []
  S.ruleChecks = []
  return S
}

/** Connection lost: reconnecting keeps what the view shows, then catches up or says it can't. */
export function buildConn(): Scenario {
  const F = WATCH.frames
  const conn = (state: string | null): SimEvent => ({ kind: 'conn', state })
  return {
    id: 'conn',
    group: 'States',
    title: 'Connection lost: reconnecting keeps the view, then catches up or gives up',
    base: fromWatch,
    steps: [
      step('live', [pv({ mode: 'pause' })], 'Following Atlas after its trim.', { at: 455000 }),
      step('reconnecting', [conn('reconnecting')], 'The connection drops. Nothing resets.', { at: 470000 }),
      step('retry now', [local({ follow: true, retry: 'trying' })], 'You press Retry now: it says it’s trying again.', { at: 472000 }),
      step('caught up', [conn(null), fr(F.move), local({ follow: true, caught: 1 })], 'Back: the change that landed meanwhile comes in.', { at: 494000 }),
      step('lost again', [conn('reconnecting')], 'It drops again.', { at: 520000 }),
      step('can’t reconnect', [conn('failed')], `${PRODUCT_NAME} stops trying on its own and says so.`, { at: 640000 }),
      step('retry fails', [local({ follow: true, retry: 'failed' })], 'You press Retry and it still can’t reach the server.', { at: 652000 }),
      step('retry works', [conn(null), local({ follow: true, caught: 0 })], 'The next Retry gets through; nothing changed while you were away.', { at: 700000 }),
    ],
  }
}

/** Timeline display: lane heights, clip pictures, scrolling while playing. */
export function buildDisplay(): Scenario {
  return {
    id: 'display',
    group: 'The tab',
    title: 'Timeline display: lane heights, clip pictures, scrolling while playing',
    base: fromWatch,
    steps: [
      step('medium · auto', [pv({ lanes: 'M', pic: 'auto' })], 'Medium lanes, pictures on auto.'),
      step('large', [pv({ lanes: 'L' })], 'Large lanes.'),
      step('small', [pv({ lanes: 'S' })], 'Small lanes: auto drops to the head picture.'),
      step('head and tail', [pv({ lanes: 'M', pic: 'headtail' })], 'The first and last frame of each clip.'),
      step('head', [pv({ pic: 'head' })], 'Head only.'),
      step('off', [pv({ pic: 'off' })], 'Pictures off.'),
      step('playing · page', [pv({ pic: 'auto', mode: 'play', scroll: 'page' })], 'Playing with “page the lanes” (the default).'),
      step('playing · centred', [pv({ scroll: 'centre' })], 'Playing with the playhead centred.'),
    ],
  }
}

/** A hold ends outside K2 (kit 678ea114): Brook holds the Music track; another app renames it Score. */
export function buildHoldGone(): Scenario {
  const B0 = builtOf(WATCH.frames.trim)!.after
  const ren = mkFrame(B0, B0.rev + 1, 470000, EXT, null, (X) => {
    X.maps[PROPS[CONT]]['@trk:4'].name = 'Score'
  })
  const c: Claim = { id: 'claim-60', actor: clone(actorOf(BROOK)), root: CONT, keys: ['@trk:4'], note: 'levelling the music', hold: true, started_ms: 455000, expires_ms: 755000 }
  return {
    id: 'holdgone',
    group: 'Presence and claims',
    title: `A hold ends outside ${PRODUCT_NAME}: the held track is renamed in another app`,
    base: fromWatch,
    steps: [
      step('Brook holds Music', [{ kind: 'claim_started', claim: c }], 'Brook holds the Music track (A2) to level it.', { at: 456000 }),
      step('renamed outside', [fr(ren), { kind: 'claim_ended', id: 'claim-60', how: 'gone_outside_save', keys: ['@trk:4'], at_ms: 470000, at_rev: ren.rev, actor: actorOf(BROOK) }], 'Another app renames Music to Score: the kit ends Brook’s hold and says why.', { at: 471000 }),
    ],
  }
}
