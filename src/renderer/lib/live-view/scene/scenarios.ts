// The sample's scenarios, part one: the picture's states (first open, a
// failed still, no still, no picture, without modifiers, turntable, large,
// lost, empty, too large) and people and views (who's where, choosing a view
// by keys, follow and say, two views, a refusal, what can't change live).
// Part two (outside saves, the pinned snapshots, the full list) is in
// scenarios-more.ts. The mock's words, step for step.

import { GOLD } from './samples'
import { CONT } from './fold'
import { SCENE_VIEW } from './view-address'
import { bigDoc, emptyDoc, REF } from './frames'
import { ACTORS, ATLAS, BROOK, SAM } from './wording'
import { blankState, ensurePeer, pvBlank, SELF, type PreviewEvent, type SimEvent, type SimState, type SimStep } from './sim'
import { TURN, fromTurn, initTurn, simulateRaw, stepOfLabelAfter, stepOfRev, type Scenario } from './turn'
import type { Doc } from './types'

export const ev = (kind: string, o: Partial<SimEvent> = {}): SimEvent => ({ kind, ...o })
export const step = (label: string, events: SimEvent[] = [], what = '', extra: Partial<SimStep> = {}): SimStep => ({ at: null, label, events, what, ...extra })
export const PV = (rev: number, state: PreviewEvent['state'], extra: Partial<PreviewEvent> = {}): SimEvent =>
  ev('preview', { p: { _constructed: true, type: 'preview', rev, view: SCENE_VIEW, state, width: 1920, ...extra } as PreviewEvent })

/** Until K2 Cores' mesh read carries camera names and lenses, cameras show as Camera 1, Camera 2 (view-list order). */
export function camLabel(view: string): string {
  const cams = GOLD.previews.views.filter((v) => v.kind === 'camera')
  const k = cams.findIndex((v) => v.view === view)
  return k >= 0 ? 'Camera ' + (k + 1) : 'Camera'
}

const FRAMES = GOLD.turn.frames
let BIG: Doc | null = null
let HUGE: Doc | null = null
/** 2,000 objects in 100 collections (under the kit's keyed limit). */
export const bigScene = (): Doc => (BIG = BIG ?? bigDoc(100, 20, 'live'))
/** 2,600 objects: past the limit, opened from a saved version. */
export const hugeScene = (): Doc => (HUGE = HUGE ?? bigDoc(130, 20, 'blob'))

const bigBase = (): SimState => {
  const D = bigScene()
  const S = blankState(D, SELF)
  S.big = 'live'
  ensurePeer(S, SELF)
  ensurePeer(S, ATLAS)
  S.follow = false
  S.ui.collapsed = D.lists[CONT].filter((c) => c !== '@coll:1')
  S.pv.ms = 286
  return S
}

export function buildStates(): Scenario[] {
  const G = 'The picture'
  return [
    { id: 'first', group: G, title: 'Opening the tab: the list first, then the first picture',
      base: () => { const S = initTurn(); S.peers = {}; ensurePeer(S, SELF); S.pv = pvBlank(null); S.pv.state = 'rendering'; S.follow = false; return S },
      steps: [
        step('connecting', [ev('loading', { v: { phase: 'connect' } })], 'The tab opens. The live watch connects and the first picture starts drawing. The viewport holds a quiet placeholder the size of the picture, so nothing jumps when it lands.'),
        step('list in', [ev('loading', { v: null }), ev('presence', { who: ATLAS, type: 'focus', value: { root: 'colls', key: '@coll:3' } })], 'The snapshot is in: the outliner and numbers are live. The picture is still being drawn the first time (a 1920 px still takes 60 to 400 ms, longer on a cold file).'),
        step('first picture', [PV(60, 'ready', { ms: 212 })], 'The first still lands and fades in. From here a new still replaces the old one; the old one stays up meanwhile.'),
      ] },
    { id: 'failed', group: G, title: 'A still that fails: keep the old one, say so, offer Retry', base: () => simulateRaw(TURN, stepOfRev(61)),
      steps: [
        step('updating', [ev('follow', { v: false })], 'Atlas moved Table; the new still is pending, the old one stays up under the live box.'),
        step('couldn\'t update', [PV(61, 'failed', { reason: 'timeout' })], 'The renderer gives up on rev 61\'s still. The rev 60 picture stays, marked as out of date, with Retry. The boxes still show where things are now.'),
        step('retry', [PV(61, 'rendering')], 'Retry asks for the still again.'),
        step('fails again', [PV(61, 'failed', { reason: 'timeout' })], 'The retry fails too: the chip says so again and keeps Retry; the old picture and the live boxes stay.'),
        step('retry', [PV(61, 'rendering')], 'Retry once more.'),
        step('drawn', [PV(61, 'ready', { ms: 362 })], 'This time it lands: the chip goes, the box settles on the new picture.'),
      ] },
    { id: 'failfirst', group: G, title: 'No still at all: "Couldn\'t draw this view"',
      base: () => { const S = initTurn(); S.pv = pvBlank(null); S.pv.state = 'rendering'; S.follow = false; return S },
      steps: [
        step('drawing', [], 'The first still is being drawn.'),
        step('failed', [PV(60, 'failed', { reason: 'render_error' })], 'It fails and there is no older still. The viewport says so plainly and offers Retry; the outliner and numbers stay live beside it.'),
        step('other view', [ev('view', { v: '@cam:1' }), PV(60, 'ready', { ms: 188, view: '@cam:1' })], 'Another view can still work: Camera draws fine.'),
      ] },
    { id: 'nopic', group: G, title: 'No picture for this file yet (the mesh read isn\'t built)',
      base: () => { const S = initTurn(); S.pv = pvBlank(null); S.pv.state = 'nopic'; S.pv.reason = 'no_mesh_read'; S.follow = false; return S },
      steps: [
        step('no picture', [], 'The renderer can\'t read this file\'s meshes yet. The tab says so once, calmly, and shows what it does know: where each object sits (its origin, from the kit\'s placements) on a ground grid. The outliner and numbers are live.'),
        step('a move', [ev('frame', { frame: FRAMES[0] }), ev('presence', { who: ATLAS, type: 'selection', value: { root: 'objs', keys: ['@obj:10'] } })], 'Atlas moves Table: its origin marker slides, Vase\'s rides along. No picture to wait for.'),
      ] },
    { id: 'nomod', group: G, title: 'Shown without modifiers; an approximate (driven) object', base: fromTurn(0),
      steps: [
        step('without modifiers', [ev('follow', { v: false }), PV(60, 'ready', { ms: 164, without_modifiers: true })], 'The still was drawn from base meshes: a quiet note says "Shown without modifiers", with the why on hover. Nothing else changes.'),
        step('driven', [ev('pick', { addr: '@obj:23' })], 'Clock hand is driven (animation sets it): its box is dashed and says "approximate". The kit marks its placement approx: driven.'),
        step('approximate box', [ev('pick', { addr: '@obj:16' })], 'Shelf\'s local bounds are approximate: an array modifier adds boards the mesh read\'s bounds leave out. Its box uses the same dashed style as a driven object; the picture\'s details say why.'),
        step('a driven move', [ev('pick', { addr: null }), ev('frame', { frame: null, made: null, multi: [[['@obj:22', [-0.2, 2.44, 0.86], null]], ATLAS, ['@obj:23']] }), ev('presence', { who: ATLAS, type: 'selection', value: { root: 'objs', keys: ['@obj:22'] } }), PV(61, 'rendering')], 'Atlas moves Clock along the wall. Clock hand rides on it; the place op marks the hand approx: driven (a real field now), so its box is dashed from the first frame of the move.'),
        step('picture', [PV(61, 'ready', { ms: 240 })], 'The new still lands; the hand\'s box stays dashed.'),
      ] },
    { id: 'turntable', group: G, title: 'Turntable: 8 stills, drag to scrub', base: fromTurn(stepOfLabelAfter(63)),
      steps: [
        step('turntable', [ev('follow', { v: false }), ev('view', { v: '@scene/turntable' })], 'Turntable: 8 stills around the scene. It plays as a slow loop; drag across it to scrub. They are 8 still pictures, not a 3D view you can turn, and the strip says so.'),
        step('scrubbed', [ev('ui', { tt: 3 })], 'Dragged to frame 4 of 8. Boxes and tags follow the frame.'),
      ] },
    { id: 'large', group: G, title: 'Large scene: 2,000 objects in 100 collections', base: bigBase,
      steps: [
        step('picture', [], '100 collections of 20 objects (the provisional live limit). The picture renders like any other (about 24,000 faces); collections start collapsed and the outliner draws only the rows on screen.'),
        step('expand', [ev('ui', { expand: ['@coll:44'] })], 'Coll_42 opened: 20 rows, still only what fits is drawn.'),
        step('search', [ev('ui', { q: 'Obj_7_1' })], 'Search by name: matches with their collection, flat.'),
        step('no match', [ev('ui', { q: 'Lamp' })], 'A name nothing has: the list says so, names what was searched, and offers Clear the search.'),
        step('cleared', [ev('ui', { q: '' })], 'Clear the search: the whole list is back, as it was before the search.'),
        step('a move', [ev('ui', { q: '' }), ev('frame', { frame: null, big: ['@obj:154', [7.5, 7.25, 0.125]] }), ev('presence', { who: ATLAS, type: 'focus', value: { root: 'colls', key: '@coll:9' } }), ev('presence', { who: ATLAS, type: 'selection', value: { root: 'objs', keys: ['@obj:154'] } }), PV(13, 'rendering')], 'Atlas moves Obj_7_13 (a constructed frame): its box moves now, tagged; the picture updates.'),
        step('picture', [PV(13, 'ready', { ms: 318 })], 'The new still lands; the box settles to an outline.'),
      ] },
    { id: 'lost', group: G, title: 'The connection drops, then comes back', base: fromTurn(stepOfLabelAfter(63)),
      steps: [
        step('rev 63', [ev('follow', { v: false })], 'Live at rev 63.'),
        step('lost', [ev('conn', { v: 'lost' })], 'The socket closed: the picture and list keep rev 63, dimmed, and say they are reconnecting. No presence while away.'),
        step('gave up', [ev('conn', { v: 'gaveup' })], 'K2 stops retrying after a while: it says so, keeps what you see, and offers Try again.'),
        step('back', [ev('conn', { v: 'ok' }), ev('toast', { tone: 'ok', text: 'Back live. Nothing changed while you were away.' })], 'Resumed: no changes were missed.'),
      ] },
    { id: 'empty', group: G, title: 'An empty scene',
      base: () => { const S = blankState(emptyDoc(), SELF); ensurePeer(S, SELF); ensurePeer(S, ATLAS); S.follow = false; S.pv.ms = 4; return S },
      steps: [step('no objects', [], 'A live scene with one collection and no objects: the still is just the background. Adding objects isn\'t live in v1, so the empty state says where new objects come from.')] },
    { id: 'blob', group: G, title: 'Too large to watch live (2,600 objects)',
      base: () => { const D = hugeScene(); const S = blankState(D, SELF); S.big = 'blob'; S.tier = 'blob'; ensurePeer(S, SELF); S.follow = false; S.ui.collapsed = D.lists[CONT].filter((c) => c !== '@coll:1'); S.pv.ms = 392; return S },
      steps: [step('saved version', [], 'Past the keyed limit: the scene opens from its last saved version. The picture is of that version; names and collections only, read-only, no presence or live boxes.')] },
  ]
}

export function buildPresence(): Scenario[] {
  const G = 'People and views'
  const j1 = TURN.steps.findIndex((s) => s.point === 'P1')
  const jAfter63 = stepOfLabelAfter(63)
  return [
    { id: 'who', group: G, title: 'Who is on what, and who looks through which view', base: fromTurn(jAfter63), steps: [
      step('on the render', [ev('follow', { v: false }), ev('pick', { addr: '@obj:15' })], 'Holds and selections drawn on the picture: Atlas holds the dining set, Brook holds Vase, each in their colour with a name tag. Sam has Floor selected (a quiet outline). Ada picked Sofa: her own selection is orange.'),
      step('view switcher', [ev('menu', { v: true })], 'The view switcher, top-left: Scene, each camera in the file by name, the framed object, Turntable. Brook\'s icon sits on Camera.001; Atlas and Sam are on Scene with Ada.'),
      step('Camera.001', [ev('view', { v: '@cam:26' })], 'Ada looks through Camera.001, where Brook is. The boxes and tags redraw for this view.'),
      step('follow Atlas', [ev('watch', { v: true }), ev('presence', { who: ATLAS, type: 'viewAddress', value: '@obj:10' })], 'Follow Atlas: the view goes where Atlas looks. Atlas asked for Table, framed.'),
      step('Atlas switches', [ev('presence', { who: ATLAS, type: 'viewAddress', value: '@cam:1' })], 'Atlas switches to Camera; Ada\'s view switches along.'),
      step('Home', [ev('view', { v: SCENE_VIEW })], 'Ada presses Home (Scene). Choosing a view ends the follow; the one way back is the header button, now "Back to Atlas".'),
      step('Sam away', [ev('presence', { who: SAM, type: 'away', value: true })], 'Sam goes away: the avatar and the outline dim; the selection stays.'),
    ] },
    { id: 'keys', group: G, title: 'Choosing a view with the keyboard', base: fromTurn(jAfter63), steps: [
      step('menu open', [ev('follow', { v: false }), ev('menu', { v: true }), ev('ui', { menuCur: SCENE_VIEW })], 'The views menu is open from the keyboard: the highlight starts on the view you have (Scene).'),
      step('down twice', [ev('menu', { v: true }), ev('ui', { menuCur: '@cam:26' })], 'Down, Down: the highlight moves to Camera 2, on the row itself.'),
      step('Enter', [ev('view', { v: '@cam:26' }), ev('toast', { tone: 'ok', text: 'Now looking through Camera 2', ms: 2500 })], 'Enter picks it: the menu closes, the picture is Camera 2, and a quiet line confirms it for 2.5 s.'),
    ] },
    { id: 'follow', group: G, title: 'Follow anyone, and say something while you watch', base: fromTurn(jAfter63), steps: [
      step('follow Atlas', [ev('follow', { v: false }), ev('watch', { v: true }), ev('presence', { who: ATLAS, type: 'viewAddress', value: '@obj:10' })], 'Ada follows Atlas (the header button). Her tab shows what Atlas looks at: Table, framed. The button reads "Following Atlas · Stop".'),
      step('say something', [ev('say', { to: ATLAS, text: 'Could the table sit a little closer to the window?', ref: '@obj:10' })], 'While following, a small composer sits on the picture: "Say something to Atlas…". What she writes carries a chip naming exactly what she is looking at: Table (its address, @obj:10, goes with the message).'),
      step('not sent', [ev('sendfail', { to: ATLAS, text: 'Could the table sit a little closer to the window?', ref: '@obj:10' })], 'Send fails (the connection blinked): the composer keeps the words, says it couldn\'t send, and offers Try again. Nothing reaches Activity yet.'),
      step('sent on retry', [ev('sent', { to: ATLAS, text: 'Could the table sit a little closer to the window?', ref: '@obj:10' })], 'Sent: Activity shows "You → Atlas: …" with the Table chip. Talking isn\'t editing: Atlas decides what to do.'),
      step('reply', [ev('reply', { from: ATLAS, text: 'On it.', ref: '@obj:10' })], 'Atlas answers in Activity: "On it."'),
      step('look away', [ev('view', { v: SCENE_VIEW })], 'Ada picks Scene herself. Picking a view leaves Follow; the header button becomes the one way back: "Back to Atlas".'),
      step('follow Sam', [ev('watch', { who: SAM }), ev('presence', { who: SAM, type: 'viewAddress', value: '@cam:26' })], 'Ada clicks Sam\'s avatar to follow a person, not an agent. Sam is looking through Camera 2, so Ada\'s tab goes there: "Following Sam · Stop".'),
      step('Sam leaves', [ev('leave', { who: SAM })], 'Sam closes the file. Follow ends quietly: "Sam left the file" for 4 s, and Ada keeps the view she had.'),
    ] },
    { id: 'two', group: G, title: 'Two views: Scene beside Atlas\'s view', base: () => simulateRaw(TURN, 2), steps: [
      step('two views', [ev('follow', { v: false }), ev('layout', { v: 2 })], 'Two views side by side, each its own still with its own picker: Scene on the left, Atlas\'s view on the right (it follows the view Atlas last asked for: Table, framed). Both are stills: no free orbit.'),
      { ...TURN.steps[stepOfRev(61)], label: 'rev 61', what: 'Atlas moves Table: both panes move its box at once, with a display-only arrow for the move, while both pictures update.' },
      { ...TURN.steps[stepOfRev(61) + 1], label: 'picture', what: 'The new stills land in both panes; the arrows fade.' },
      step('Atlas switches', [ev('presence', { who: ATLAS, type: 'viewAddress', value: '@cam:1' })], 'Atlas switches to Camera: the right pane switches along; yours stays on Scene.'),
    ] },
    { id: 'refuse', group: G, title: 'Refused on the render: Brook tries to rename a chair Atlas holds', base: fromTurn(j1), steps: [
      step('held', [ev('follow', { v: false })], 'Atlas holds the dining set ("arranging the dining set").'),
      step('refused', [ev('presence', { who: BROOK, type: 'focus', value: { root: 'colls', key: '@coll:3' } }), ev('presence', { who: BROOK, type: 'selection', value: { root: 'objs', keys: ['@obj:12'] } }), ev('refused', { r: REF(BROOK, 'object rename', '@obj:12', 'MPC-E301', ATLAS, 'claim-12', 't:2c91d0e4b7a35f18.3') })], 'Brook tries to rename Chair.002; Atlas holds it, so the verb is refused (MPC-E301). The callout sits by the chair on the picture; the code only in its tooltip.'),
      step('released', [ev('claim_ended', { id: 'claim-12', how: 'released', at_ms: null, at_rev: null, actor: ACTORS[ATLAS] }), ev('presence', { who: ATLAS, type: 'selection', value: null })], 'Atlas lets go of the dining set: the holds leave the picture, Activity says "Atlas let go of the dining set", and People no longer lists Atlas on the chairs. The refusal in Activity now says Atlas was holding it.'),
      step('renamed', [ev('frame', { frame: null, rename: ['@obj:12', 'Dining chair', BROOK, 't:2c91d0e4b7a35f18.3'] }), PV(61, 'ready', { reused_from: 60, ms: 0 })], 'Brook renames Chair.002 to Dining chair. A rename doesn\'t redraw the picture; the tag says what changed, then fades.'),
    ] },
    { id: 'limits', group: G, title: 'What can\'t change live: driven, quaternion, linked', base: fromTurn(stepOfLabelAfter(63)), steps: [
      step('driven', [ev('follow', { v: false }), ev('pick', { addr: '@obj:23' }), ev('refused', { r: REF(ATLAS, 'object place', '@obj:23', 'blend_driven', null, null, FRAMES[0].turn) })], 'Clock hand: animation drives it. Its box is dashed (approximate). Atlas tries to move it anyway and is refused: the callout says why in plain words.'),
      step('quaternion', [ev('pick', { addr: '@obj:9' })], 'Ceiling fan turns by quaternion: location, scale and name can change live; rotation can\'t.'),
      step('linked', [ev('pick', { addr: '@obj:24' })], 'Plant is linked from another file: read-only here, and not in the clay picture (only an origin marker shows where it sits).'),
    ] },
  ]
}
