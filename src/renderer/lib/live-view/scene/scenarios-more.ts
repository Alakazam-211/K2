// The sample's scenarios, part two: the outside saves (O1 to O5, the kit's
// outside_save frame) and the pinned BLEND snapshots (k2-mpc-blend
// 686cd0da), read exactly as pinned; then the one list of all 24, in the
// mock's order: the turn, people and views, the picture, the saves, pinned.

import { GOLD } from './samples'
import { parse9 } from './math'
import { DOCS, docFromIndex } from './fold'
import { SCENE_VIEW } from './view-address'
import { blankState, ensurePeer, SELF, type SimState } from './sim'
import { TURN, fromTurn, initTurn, stepOfLabelAfter, stepOfRev, type Scenario } from './turn'
import { buildPresence, buildStates, ev, PV, step } from './scenarios'
import type { Doc } from './types'
import { PRODUCT_NAME } from '@/lib/brand'

export const SAVE = (id: string): (typeof GOLD.saves.cases)[number] => GOLD.saves.cases.find((c) => c.id === id) as (typeof GOLD.saves.cases)[number]

function buildSaves(): Scenario[] {
  const G = 'Outside saves'
  const j65 = stepOfRev(65)
  const o1 = SAVE('O1')
  const o2 = SAVE('O2')
  const o3 = SAVE('O3')
  const o4 = SAVE('O4')
  const o5 = SAVE('O5')
  // kept_both: the save becomes the live file. Its content arrives as a change by the file (kit:external):
  // Rug back to its rev 62 size (Atlas's resize is in the copy), Floor lamp moved.
  const rug62 = (parse9(DOCS[62].props['@obj:8'].xform) as number[]).slice(6, 9)
  return [
    { id: 'O1', c: o1, group: G, title: 'O1 · a save on a synced file: taken in', base: fromTurn(j65 - 1), steps: [
      step('rev 64', [ev('follow', { v: false })], 'Synced: the file holds rev 64.'),
      { ...TURN.steps[j65], label: 'taken in', what: o1.means },
      { ...TURN.steps[j65 + 1], label: 'picture', what: 'The new still lands.' },
    ] },
    { id: 'O2', c: o2, group: G, title: 'O2 · a save made over unwritten edits: kept both, the save is now live', base: fromTurn(stepOfLabelAfter(63)), steps: [
      step('rev 63, unwritten', [ev('follow', { v: false })], `The file holds rev ${o2.synced_rev}; Atlas's rev 63 (Rug resized) is still unwritten.`),
      step('kept both', [ev('frame', { frame: null, multi: [[['@obj:8', null, rug62], ['@obj:19', [-1.25, 1.95, 0], null]], 'kit:external'] }), ev('settled', { v: o2.settled }), PV(64, 'rendering')], o2.means),
      step('picture', [PV(64, 'ready', { ms: 330 })], 'The new still shows the saved file: the rug is back to its old size and the floor lamp has moved. Atlas\'s resize lives on in the conflict copy and in History.'),
    ] },
    { id: 'O3', c: o3, group: G, title: 'O3 · a save that can\'t pair an object: taken in as a new object', base: () => { const S = initTurn(); S.follow = false; return S }, steps: [
      step('rev 60', [], 'Synced at rev 60.'),
      step('taken in', [ev('frame', { frame: o3.frame }), ev('settled', { v: o3.settled }), ev('pick', { addr: '@obj:27' }), PV(61, 'rendering')], o3.means),
      step('picture', [PV(61, 'ready', { ms: 296 })], 'The new still shows the stool.'),
    ] },
    { id: 'O4', c: o4, group: G, title: `O4 · a save ${PRODUCT_NAME} can't keep live: declined, edits pause`, base: fromTurn(j65 + 1), steps: [
      step('rev 65', [ev('follow', { v: false })], 'Live at rev 65.'),
      step('declined', [ev('settled', { v: o4.settled })], o4.means),
    ] },
    { id: 'O5', c: o5, group: G, title: `O5 · the file is moved or deleted outside ${PRODUCT_NAME}`, base: fromTurn(j65 + 1), steps: [
      step('rev 65', [ev('follow', { v: false })], 'Live at rev 65.'),
      step('missing', [ev('settled', { v: o5.settled })], o5.means),
    ] },
  ]
}

function buildPinned(): Scenario[] {
  const G = 'Pinned BLEND snapshots (k2-mpc-blend 686cd0da)'
  const PO = GOLD.pinned.outliner
  const PR = GOLD.pinned.rig
  const IXR = PR.index
  const pages = PR.crowd_pages.pages
  const pinnedBase = (D: Doc, extra: Partial<SimState> = {}) => (): SimState => {
    const S = blankState(D, SELF)
    ensurePeer(S, SELF).view = SCENE_VIEW
    S.follow = false
    S.pinned = true
    Object.assign(S, extra)
    return S
  }
  const head = new Set(IXR.units.filter((u) => u.container !== pages[0].container).map((u) => u.key))
  const rigBase = (): SimState => {
    const S = pinnedBase(docFromIndex(IXR, head), { pinnedName: 'rig' })()
    S.pageIx = IXR
    S.loaded = [...head]
    S.pages = { [pages[0].container]: { loaded: 0, total: pages[0].total, next: 0 } }
    S.ui.collapsed = []
    return S
  }
  return [
    { id: 'pin-outliner', group: G, title: 'Pinned: the outliner scene, as the kit prints it', base: pinnedBase(docFromIndex(PO), { pinnedName: 'outliner' }), steps: [
      step('index', [], 'The kit\'s view index of the outliner sample, read exactly as pinned (BLEND main 0c552c1a): Set and Props sit in Scene, Inner sits in Set, and the objects not in a collection come last. The tree nests from each collection\'s parent, siblings in position order. Child has no placement (it isn\'t placed), so it has no box.'),
      step('a driven object', [ev('pick', { addr: '@obj:6' })], 'Walker is driven: its pinned placement says approx: "driven", so its box is dashed and called approximate.'),
      step('a collection moves', [ev('pick', { addr: null }), ev('collmove', { delta: { key: '@coll:4', parent_after: '@coll:2' } })], 'Props moves into Set. The change arrives as Props\'s delta with parent_after (no fresh snapshot), and the tree updates in place.'),
      step('a parent it can\'t use', [ev('collmove', { delta: { key: '@coll:3', parent_after: '@coll:9' } })], 'A parent that isn\'t in the file (@coll:9) is refused: Inner shows at the top level, and the refusal is in the picture\'s details only.'),
    ] },
    { id: 'pin-rig', group: G, title: 'Pinned: a rig and a 64-object Crowd arriving in two pages', base: rigBase, steps: [
      step('index head', [], `The rig sample's index: Rig holds Base and Arm; Crowd says it holds ${pages[0].total} objects, and its units come in pages (budget ${PR.crowd_pages.budget} bytes). Everything here is an empty (no shape): the picture draws each as a small marker, a stand-in, so you can see where they sit.`),
      step('page 1', [ev('page', { page: pages[0] })], `The first page arrives: ${pages[0].units.length} of ${pages[0].total} Crowd objects (from ${pages[0].from}, next ${pages[0].next}). The outliner shows them and says ${pages[0].total - pages[0].units.length} more are coming.`),
      step('page 2', [ev('page', { page: pages[1] })], `The second page: ${pages[1].units.length} more (from ${pages[1].from}, next null). All ${pages[1].total} are in.`),
      step('Arm', [ev('pick', { addr: '@obj:2' })], 'Arm is placed on Base through a parent inverse that turns it a quarter turn and moves it (not the identity): its own location reads (0, 2, 0) while it sits at (-1.5, -2, 1) in the world, as the kit\'s placement rules give.'),
    ] },
  ]
}

export const STATES = buildStates()
export const PRES = buildPresence()
export const SAVES = buildSaves()
export const PINNED = buildPinned()
/** The mock's 24 in its order, with S4's "on the picture" among People and views. */
export const SCENARIOS: Scenario[] = [TURN, ...PRES, ...STATES, ...SAVES, ...PINNED]
