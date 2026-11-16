// The mock's first scenarios beside the turn: opening, presence, refusals,
// the four outside saves, and the empty and error states.

import { actorOf } from './actors'
import { clone } from './adapter'
import { builtOf, geomSet, mkFrame } from './diff'
import { modelOf } from './fold'
import { GOLD } from './samples'
import { CONT, MKR, PROPS, TLR, UNIT, type Claim, type OutsideSaveEvent, type Refusal } from './schema'
import { ensurePeer, simulateRaw, step, type LoadingState, type Scenario, type SimEvent, type SimState } from './sim'
import { pairOf } from './time'
import { ATLAS, B30, blank, BROOK, CTX, EXT, FRT, fromTurn, initTurn, jFirst, jP2, KAI, pres, ptr, SAM, SELF, stepOf, TURN, vpt } from './world'

/** Opening: the welcome, then the snapshot's pages in the schema's root order, then the view index's units pages. */
export function buildOpen(): Scenario {
  const roots = GOLD.state.roots.map((r, i) => ({ name: r.name, n: r.kind === 'list' ? (r.live || []).length : (r.entries || []).length, kind: r.kind, i }))
  const unitPages = GOLD.index.containers.map((c) => ({ key: c.key, title: c.title, n: GOLD.index.units.filter((u) => u.container === c.key).length }))
  const L = (phase: string, upto: number): LoadingState => ({ phase, roots, unitPages, upto })
  const at = (n: string): number => roots.findIndex((r) => r.name === n)
  const ld = (v: LoadingState | null): SimEvent => ({ kind: 'loading', v })
  const s = [
    step('connect', [ld(L('connect', -1))], 'Opening the live watch: the tab shows the file name and a spinner.'),
    step('welcome', [ld(L('welcome', -1))], 'The welcome: who you are (self), rights, features. The snapshot head carries the schema and the view index containers with their derived labels and kinds: lane headers (V2, V1, A1, A2) are laid out, video above audio.'),
  ]
  for (const r of roots) {
    const extra = r.i === at(TLR) ? ' The rate and start timecode come in this first page, so the ruler reads timecode from here on, while the rest loads.' : r.i === at(PROPS[CONT]) ? ' Lane names draw now.' : r.i === at(PROPS[UNIT]) ? ' Every bar is placed as the page lands.' : r.i === at(PROPS[MKR]) ? ' Markers drop in.' : ''
    s.push(step(r.name, [ld(L('pages', r.i))], `keyed_page: ${r.name}, ${r.n} ${r.kind === 'list' ? 'elements' : 'entries'}.${extra}`))
  }
  s.push(step('units', [ld(L('units', roots.length))], `units pages: ${unitPages.map((u) => u.title + ' ' + u.n).join(', ')}. Hover and derived timecodes are ready.`))
  s.push(step('live', [ld(null)], 'Every page is in: presence and frames start.'))
  return {
    id: 'open',
    group: 'States',
    title: 'Opening: loading the live watch, then ready',
    steps: s,
    base: () => {
      const S = initTurn()
      S.base = B30
      return S
    },
  }
}

/** Presence close-up: at P2, plus Kai far along the timeline. */
export function buildPresence(): Scenario {
  const base = (): SimState => {
    const S = simulateRaw(TURN, jP2, CTX)
    S.recent = []
    S.ruleChecks = []
    Object.assign(ensurePeer(S, KAI), { viewport: vpt(2150, 2850, '@trk:1', '@trk:4'), pointer: ptr(2600, null) })
    return S
  }
  return {
    id: 'presence',
    group: 'Presence and claims',
    title: 'Presence: playheads, visible ranges, who holds which clip',
    base,
    steps: [
      step('here', [], 'Presence is in the timeline’s own time: each playhead is a line in its owner’s colour with a flag in the ruler.'),
      step('Brook’s playhead', [pres(BROOK, 'pointer', ptr(708, '@trk:2'))], 'An agent can have a playhead too: Brook parks its playhead on V2 at 01:00:29:12.'),
      step('Sam scrolls', [pres(SAM, 'viewport', vpt(1700, 2800, '@trk:1', '@trk:4')), pres(SAM, 'pointer', ptr(2056, '@trk:1'))], 'Sam scrolls on and down to V1 to A2: Sam’s playhead leaves your screen, so an edge chip shows where.'),
      step('Kai away', [pres(KAI, 'away', true)], 'Kai is away: the playhead goes dashed and faint; the avatar dims.'),
      step('you look away', [{ kind: 'local', v: { follow: false, center: 1900 } }], 'You scroll on your own: following Atlas pauses, and a pill offers to follow again.'),
    ],
  }
}

/** Refused verbs, agent against agent (ruled): who, verb, unit, code and holder. */
export function buildRefuse(): Scenario {
  const refused = (actor: string, verb: string, unit: string, holder: string, claim: string, at_ms: number): SimEvent => ({
    kind: 'refused',
    r: { at_ms, actor: clone(actorOf(actor)), client: 4711, turn: null, verb, unit, code: 'MPC-E301', holder: clone(actorOf(holder)), claim } satisfies Refusal,
  })
  const c42: Claim = { id: 'claim-42', actor: clone(actorOf(BROOK)), root: UNIT, keys: ['@item:6'], note: 'naming the interviews', hold: true, started_ms: 306500, expires_ms: 426500 }
  return {
    id: 'refuse',
    group: 'Presence and claims',
    title: 'Refused verbs: two agents want one clip',
    base: fromTurn(jFirst),
    steps: [
      step('before', [], 'Atlas holds Interview A, B-roll 03 and B-roll 05 (claim-41, “tightening the open”). Brook is here too.'),
      step('Brook refused', [pres(BROOK, 'focus', { root: CONT, key: '@trk:2' }), refused(BROOK, 'clip move', '@item:13', ATLAS, 'claim-41', 306000)], 'Brook tries to move B-roll 03, which Atlas holds. The kit refuses the verb; nothing changes.'),
      step('Brook holds', [pres(BROOK, 'focus', { root: CONT, key: '@trk:1' }), pres(BROOK, 'selection', { root: UNIT, keys: ['@item:6'] }), { kind: 'claim_started', claim: c42 }], 'Brook holds Interview B instead (claim-42).'),
      step('Atlas refused', [refused(ATLAS, 'clip trim', '@item:6', BROOK, 'claim-42', 307200)], 'Atlas tries to trim Interview B and is refused by Brook’s claim.'),
      step('Ada looks', [pres(SELF, 'selection', { root: UNIT, keys: ['@item:6'] })], 'Ada selects Interview B to see who holds it. People watch: selection, playhead and visible range.'),
      step('released', [{ kind: 'claim_ended', id: 'claim-42', how: 'released', at_ms: 309000, at_rev: 31, actor: actorOf(BROOK) }], 'Brook lets go: the outline goes.'),
    ],
  }
}

/** Outside saves (kit c23d2349): taken in, kept both, declined, missing. */
export function buildSaves(): Scenario[] {
  const applied = mkFrame(B30, 31, 304000, EXT, null, (B) => {
    B.maps[PROPS[UNIT]]['@item:17'].off = '0'
    B.lists[MKR].push(['@mk:5', 'marker'])
    B.maps[PROPS[MKR]]['@mk:5'] = { owner: '', at: String(1100n * FRT), dur: '0', name: 'Check colour', note: 'too warm', color: 'CYAN' }
    B.maps[TLR]['mark/k'][''] = '5'
  })
  const jSave = stepOf(TURN, (s) => s.events.some((e) => e.kind === 'outside_save'))
  const save = (o: OutsideSaveEvent): SimEvent => ({ kind: 'outside_save', save: { ...o } })
  const kbEv = TURN.steps[jSave].events.find((e) => e.kind === 'frame') as Extract<SimEvent, { kind: 'frame' }>
  const kb = kbEv.frame
  const KB = builtOf(kb)!.after
  const MK = modelOf(KB)
  const tk = Object.keys(MK.items).find((k) => k === '@item:19') || Object.values(MK.items).find((x) => x.kind === 'clip')!.key
  const after = mkFrame(KB, kb.rev + 1, 330000, ATLAS, 'clip trim', (B) => {
    const g = pairOf(B.maps[PROPS[UNIT]][tk].geom)!
    geomSet(B, tk, String(g[0]), String(g[1] - 12n * FRT))
  }, { turn: 't:e4a07c19b2d56f38.1', client: 4711 })
  const before = (what: string) => step('before', [], what)
  return [
    {
      id: 'save-ingested',
      group: 'Outside saves',
      title: 'Outside save, taken in: nothing live was waiting',
      base: initTurn,
      steps: [
        before('Nobody has edited live since the file was opened: the live file equals the saved one.'),
        step('saved', [{ kind: 'frame', frame: applied }, save({ _at_ms: 304000, outcome: 'ingested', rev: 31, _summary: 'Lower third turned on, a marker “Check colour”' })], 'An editing app saves the file with two changes; the kit takes the save in as one change from the file.'),
      ],
    },
    {
      id: 'save-both',
      group: 'Outside saves',
      title: 'Outside save, kept both: the save becomes the live file',
      base: fromTurn(jSave - 1),
      steps: [
        before('The turn is written back (rev 35). An editing app that opened the file before that is still open.'),
        { ...TURN.steps[jSave], label: 'saved', what: 'It saves over the file without the live edits. The kit keeps both: the save becomes the live file, and what was live is kept as a stored version and as a conflict copy.' },
        step('carries on', [{ kind: 'frame', frame: after }], 'Atlas carries on, now on the saved file.'),
      ],
    },
    {
      id: 'save-declined',
      group: 'Outside saves',
      title: 'Outside save, declined: the file can’t be live any more, edits pause',
      base: fromTurn(jSave - 1),
      steps: [
        before('The turn is written back (rev 35).'),
        step('declined', [save({ _at_ms: 321000, outcome: 'declined', code: 'live_overlap', message: 'Two clips overlap on V2 in that save.', detail: { track: '@trk:2', a: 'B-roll 06', b: 'Lower third · Interview C' } })], 'The app saves a file in which two clips overlap on V2: the kit declines it and edits pause.'),
      ],
    },
    {
      id: 'save-missing',
      group: 'Outside saves',
      title: 'Outside save, missing: the file was deleted or moved',
      base: fromTurn(jSave - 1),
      steps: [before('The turn is written back (rev 35).'), step('missing', [save({ _at_ms: 321000, outcome: 'missing' })], 'The file is deleted or moved outside K2. The live version is kept.')],
    },
  ]
}

/** Empty: a timeline with tracks and no clips; errors: files that can't be watched live. */
export function buildEmpty(): Scenario[] {
  const E = clone(B30)
  E.lists.items = []
  E.lists.markers = []
  E.maps[PROPS[UNIT]] = {}
  E.maps[PROPS[MKR]] = {}
  E.maps[TLR].name = { '': 'New edit' }
  E.rev = 3
  const add = mkFrame(E, 4, 500000, ATLAS, 'clip add', (X) => {
    X.lists.items.push(['@item:34', 'clip'])
    X.maps[PROPS[UNIT]]['@item:34'] = { track: 't1', geom: `0,${String(240n * FRT)}`, name: 'Interview A', off: '0', edit: '1', src: String(1214458n * FRT), media: 'm1', rate: '24/1' }
    X.maps[TLR]['mark/i'] = { '': '34' }
  }, { turn: 't:77aa01c3e5f20b94.1', client: 4711 })
  const base = (): SimState => {
    const S = blank(E)
    ensurePeer(S, SELF)
    ensurePeer(S, ATLAS)
    return S
  }
  const decl = (code: string, title: string, text: string): SimEvent => ({ kind: 'tier', tier: 'declined', decline: { code, title, text, detail: code } })
  return [
    {
      id: 'empty',
      group: 'Empty and error states',
      title: 'Empty: a timeline with tracks and no clips',
      base,
      steps: [step('no clips', [], 'Lanes and the ruler draw; a quiet line says there are no clips yet.'), step('first clip', [{ kind: 'frame', frame: add }], 'Atlas adds the first clip: it grows in, and the empty line goes.')],
    },
    {
      id: 'errors',
      group: 'Empty and error states',
      title: 'Errors: files that can’t be watched live',
      base: initTurn,
      steps: [
        step('many timelines', [decl('live_many_timelines', 'This file holds 3 timelines', 'K2 watches one timeline per file live. You can still open the saved file.')]),
        step('overlap', [decl('live_overlap', 'Two clips overlap on V2', 'K2 can only watch a timeline live when the clips on a track don’t overlap (except across a transition). Fix it in the file and K2 tries again on the next save.')]),
        step('between frames', [decl('live_time_off_tick', 'Some times fall between frames', 'A few clips start or end at times K2 can’t count exactly, so it won’t watch this file live. You can still open the saved file.')]),
        step('too large', [decl('live_too_large', 'Too large to watch live, for now', 'This timeline has 2,612 items. K2 watches up to 2,500 live today. You can still open the saved file.')]),
      ],
    },
  ]
}
