// The monitor's states (the design section's table, one place at a time),
// preparing video, and the pack's pinned snapshots read exactly as pinned.

import { addActor } from './actors'
import { bookFromPinned, clone } from './adapter'
import { modelOf } from './fold'
import { vclips } from './picture'
import { GOLD } from './samples'
import { CONT, MED, PROPS, UNIT, type Book } from './schema'
import { ensurePeer, step, type Scenario, type SimEvent, type SimState } from './sim'
import { PER_S } from './time'
import { ATLAS, B30, blank, BROOK, FACTS, FRT, pres, ptr, SAM, SELF, vpt } from './world'

const pv = (v: Record<string, unknown>): SimEvent => ({ kind: 'pv', v })
const local = (v: Record<string, unknown>): SimEvent => ({ kind: 'local', v })

/** The sample has no image or drop-frame clip on a video track, so this book adds both in V2's empty stretch. */
export const B30IMG: Book = (() => {
  const B = clone(B30)
  B.lists[MED].push(['@med:90', 'file'])
  B.maps[PROPS[MED]]['@med:90'] = { file: 'Map still.png', name: 'Map still.png', nid: 'm:6d6170' }
  B.lists[MED].push(['@med:91', 'file'])
  B.maps[PROPS[MED]]['@med:91'] = { file: 'Street 2997.mov', name: 'Street 2997.mov', nid: 'm:737472', rate: '30000/1001', drop_frame: '1', start: String((108000n * PER_S) / 30n), origin: 'range' }
  B.lists[UNIT].push(['@item:91', 'clip'])
  B.maps[PROPS[UNIT]]['@item:91'] = { track: 't2', geom: `${String(2336n * FRT)},${String(48n * FRT)}`, name: 'Street, drop-frame', off: '0', edit: '1', src: String(240n * FRT), media: 'm91', rate: '30000/1001', retimed: '0' }
  B.lists[UNIT].push(['@item:90', 'clip'])
  B.maps[PROPS[UNIT]]['@item:90'] = { track: 't2', geom: `${String(2390n * FRT)},${String(72n * FRT)}`, name: 'Map still', off: '0', edit: '1', src: '0', media: 'm90', rate: '24/1' }
  return B
})()

export function buildStates(): Scenario {
  const base = (): SimState => {
    const S = blank(B30IMG)
    Object.assign(ensurePeer(S, SELF), { pointer: ptr(700, null), viewport: vpt(300, 1700, '@trk:2', '@trk:4') })
    Object.assign(ensurePeer(S, SAM), { pointer: ptr(2460, '@trk:1') })
    Object.assign(ensurePeer(S, ATLAS), { pointer: ptr(1030, '@trk:1'), focus: { root: CONT, key: '@trk:1' } })
    ensurePeer(S, BROOK)
    return S
  }
  const at = (f: number, track: string): SimEvent => pres(ATLAS, 'pointer', ptr(f, track))
  return {
    id: 'states',
    group: 'The tab',
    title: 'Monitor states: still, loading, playing, seeking, waiting, stills only, not a video, no preview, offline, nest, other, gap, end',
    base,
    steps: [
      step('still', [at(1065, '@trk:1'), pv({ player: 'still', mode: 'pause' })], 'Paused without the exact frame: the nearest filmstrip tile at or before the playhead.'),
      step('still · head', [at(1005, '@trk:1')], 'Between Interview C’s in-point and its first grid tile: the head tile.'),
      step('still · poster', [pv({ pending: ['@item:5'] })], 'The head tile isn’t there yet, so the source’s poster stands in.'),
      step('loading', [at(1065, '@trk:1'), pv({ player: 'cut', mode: 'first', pending: [] })], 'The first frame is loading.'),
      step('playing', [pv({ mode: 'play', rung: 1920 })], 'Playing, cuts only.'),
      step('quick seek', [at(1100, '@trk:1'), pv({ mode: 'seek', rung: 640, quick: true })], 'A quick seek shows nothing new.'),
      step('seeking', [at(1440, '@trk:2'), pv({ mode: 'seek', rung: 640, quick: false })], 'A longer seek: “Seeking…” after 300 ms.'),
      step('sharp', [pv({ mode: 'pause', rung: 1920 })], 'The exact frame is up.'),
      step('waiting', [pv({ mode: 'stall' })], 'Mid-play, the data is late: “Waiting for video…”.'),
      step('stills only', [pv({ player: 'still', mode: 'pause', why: 'decode' })], 'This computer can’t play the preview: Stills mode.'),
      step('not a video', [at(2420, '@trk:2'), pv({ player: 'cut', why: null })], 'An image on a video track: a card with its name and kind.'),
      step('no preview', [at(1065, '@trk:1'), pv({ nopreview: ['@med:3'] })], 'This clip can’t be previewed.'),
      step('skipped', [local({ follow: false, skipped: true }), pres(SELF, 'pointer', ptr(1344, '@trk:1'))], 'Skip to the next clip.'),
      step('offline', [local({ follow: true }), at(1450, '@trk:2'), pv({ nopreview: [], offline: ['@med:11'] })], 'This clip’s media isn’t available right now.'),
      step('nest', [at(2230, '@trk:2'), pv({ offline: [] })], 'A nested timeline.'),
      step('other', [at(1020, '@trk:2')], 'A title and a turned-off clip are passed over.'),
      step('gap', [at(1656, '@trk:1')], 'Nothing on either video track.'),
      step('end', [at(2855, '@trk:1'), pv({ mode: 'ended' })], 'End of the edit.'),
      step('drop frame', [at(2350, '@trk:2'), pv({ mode: 'pause' })], 'A 29.97 drop-frame source: its timecode as given, in the “;” form.'),
    ],
  }
}

export type PrepScenario = Scenario & { clips: { pool: number; cant: number; first3: string[]; most: string[]; stopAt: string | null } }

/** Preparing video: plain blocks fill in clip by clip; play waits for the next 30 s. */
export function buildPrep(): PrepScenario {
  const M = modelOf(B30)
  const clips = vclips(M, FACTS)
  const isCant = (name: string): boolean => ['B-roll 05', 'B-roll 08'].includes(name)
  const cant = clips.filter((x) => isCant(x.name)).map((x) => x.key)
  const pool = clips.filter((x) => !isCant(x.name))
  const early = pool.slice().sort((a, b) => a.f0 - b.f0)
  const first3 = early.slice(-3).map((x) => x.key) // three clips late in the edit are ready first
  const near = early.filter((x) => x.f0 < 60 * 24).map((x) => x.key)
  const most = [...new Set([...first3, ...near])]
  const stopAt = early.find((x) => !most.includes(x.key) && x.f0 > 40)
  const base = (): SimState => {
    const S = blank(B30)
    Object.assign(ensurePeer(S, SELF), { pointer: ptr(40, null) })
    ensurePeer(S, ATLAS)
    Object.assign(ensurePeer(S, SAM), { pointer: ptr(700, '@trk:1') })
    S.follow = false
    return S
  }
  const lf = local({ follow: false })
  return {
    id: 'prep',
    group: 'The tab',
    title: 'Preparing video: plain blocks fill in clip by clip, play waits for the next 30 s',
    base,
    clips: { pool: pool.length, cant: cant.length, first3, most, stopAt: stopAt ? stopAt.key : null },
    steps: [
      step('preparing', [lf, pv({ player: 'cut', mode: 'pause', prep: { ready: first3, cant } })], `Preparing video · 3 of ${pool.length} clips ready; Play waits.`),
      step('can’t preview', [pv({ prep: { ready: first3, cant, hl: true } }), { kind: 'zoom', fit: true }], 'The clips that can’t be previewed are highlighted.'),
      step('filling in', [pv({ prep: { ready: most, cant, fresh: most.filter((k) => !first3.includes(k)) } })], 'Clips fill in one by one; Play is enabled.'),
      step('playing', [pv({ mode: 'play', prep: { ready: most, cant } }), pres(SELF, 'pointer', ptr(60, null))], 'It plays from the proxies that are ready.'),
      step('preparing this clip', [pv({ mode: 'stall', why: 'prep', prep: { ready: most, cant } }), pres(SELF, 'pointer', ptr(stopAt ? stopAt.f0 : 1400, null))], 'Playback holds at a clip that isn’t ready.'),
      step('video ready', [pv({ mode: 'play', why: null, prep: { ready: 'all', cant, done: true, fresh: pool.map((x) => x.key).filter((k) => !most.includes(k)) } })], 'Video ready.'),
    ],
  }
}

/** The pack's view golden snapshots (k2-mpc-timeline, re-blessed at 82dfb6c0), read exactly as pinned. */
export const GB = { cut: bookFromPinned(GOLD.golden.cut), edited: bookFromPinned(GOLD.golden['cut-edited']), trim: bookFromPinned(GOLD.golden['cut-trim-in']) }
export const GCHANGE = GOLD.golden['cut-trim-in'].change
addActor({ ...GCHANGE.actor, name: 'Atlas' }) // the pinned actor's handle is "atlas"; the view shows its name as Atlas

export function buildGolden(): Scenario {
  const base = (): SimState => {
    const S = blank(GB.cut)
    ensurePeer(S, SELF)
    ensurePeer(S, 'atlas')
    S.peers[SELF].pointer = ptr(30, null)
    S.follow = false
    return S
  }
  return {
    id: 'golden',
    group: 'The tab',
    title: 'Pinned snapshots (the kit’s view golden): the cut, a trim of a clip’s start, then six edits',
    base,
    steps: [
      step('the cut', [local({ follow: false }), { kind: 'zoom', fit: true }], 'The kit’s pinned cut as it opens.'),
      step('trim in', [{ kind: 'frame', frame: GCHANGE }], 'The pinned change frame: atlas trims the start of Shot A.'),
      step('six edits', [{ kind: 'book', B: GB.edited, note: 'A new snapshot came in after six edits: a move, a trim, a lift, an add, an enable and a marker' }], 'The pinned cut after six edits (one snapshot, as pinned).'),
    ],
  }
}
