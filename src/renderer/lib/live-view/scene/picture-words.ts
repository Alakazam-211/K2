// What the picture says, and what it offers, in each of its states: the
// card over the stage (opening, no still at all, no picture for this file
// yet, an empty scene), the state chips along its foot (updating, couldn't
// update with Retry, picture updated, without modifiers, approximate, the
// saved version), and the "older picture" mark. Pure: the words are the
// mock's (live-scene-v2-notes.md, v2.4 and v2.5), tested without a browser.

import { failureWords } from './left-out'
import type { PictureStatus } from './picture-source'

export type CardAction = 'retry' | 'try-camera' | 'show-list' | 'ask'
export interface PictureCard {
  kind: 'opening' | 'nostill' | 'nopicture' | 'empties' | 'empty' | 'declined'
  title: string
  body?: string
  /** "Opening…": what's done, what's under way, what's next. */
  steps?: { label: string; state: 'done' | 'now' | 'next' }[]
  actions: { id: CardAction; label: string; primary: boolean }[]
  /** Sits high on the stage, so the origin dots under it show. */
  atTop?: boolean
}

export interface PictureChip {
  id: 'updating' | 'failed' | 'updated' | 'markers' | 'nomod' | 'approx' | 'saved' | 'paused'
  tone: 'busy' | 'bad' | 'ok' | 'info'
  text: string
  /** The why, on hover. */
  title?: string
  action?: { id: 'retry'; label: string }
}

export interface PictureWords {
  card: PictureCard | null
  chips: PictureChip[]
  /** Marked on the picture while an older still stands in for a failed one. */
  older: string | null
  /** A still is on the stage. */
  still: boolean
  /** No picture: draw the origin dots on a ground grid. */
  ground: boolean
  /** The connection dropped: the picture dims. */
  dim: boolean
}

export interface PictureContext {
  /** The live watch is still connecting (the list isn't in yet). */
  loading: boolean
  /** live, blob (too large: the saved version), paused (edits paused). */
  tier: string
  /** ok, lost, gaveup. */
  conn: string
  /** The scene has objects. */
  objects: boolean
  /** Pinned test scenes draw shape-less objects as markers. */
  markers: boolean
  /** The selected object's box is approximate: driven, or a modifier adds geometry. */
  approx: { name: string; driven: boolean } | null
  /** The camera "Try …" offers, in words ("Camera 1"), when the file has one. */
  camera: string | null
  /** The agent here to ask, when the scene is empty. */
  askAgent: string | null
}

export const OPENING_STEPS = ['Connecting', 'The list of objects', 'The first picture'] as const

/** The picture's words for its status and the tab's context. */
export function pictureWords(pv: PictureStatus, c: PictureContext): PictureWords {
  const chips: PictureChip[] = []
  const shown = pv.shown != null
  const dim = c.conn !== 'ok'

  // The card: one at a time, the opening first.
  let card: PictureCard | null = null
  if (c.loading || (pv.state === 'rendering' && !shown)) {
    const at = c.loading ? 0 : 2
    card = { kind: 'opening', title: 'Opening…', steps: OPENING_STEPS.map((label, i) => ({ label, state: i < at ? 'done' : i === at ? 'now' : 'next' })), actions: [] }
  } else if (pv.state === 'failed' && !shown) {
    card = {
      kind: 'nostill',
      title: 'Couldn’t draw this view',
      body: `${failureWords(pv.reason)} The outliner and numbers are still live.`,
      actions: [{ id: 'retry', label: 'Retry', primary: true }, ...(c.camera ? [{ id: 'try-camera' as const, label: `Try ${c.camera}`, primary: false }] : [])],
    }
  } else if (pv.state === 'nopic' && pv.reason === 'empties') {
    card = { kind: 'empties', title: 'Nothing to draw', body: 'This scene holds only empties, which have no shape. The dots show where they sit; the list is live.', actions: [{ id: 'show-list', label: 'Show the list', primary: false }], atTop: true }
  } else if (pv.state === 'nopic') {
    card = {
      kind: 'nopicture',
      title: 'No picture for this file yet',
      body: 'K2 can’t read this file’s shapes yet. The list still updates live; the dots show where objects sit.',
      actions: [{ id: 'retry', label: 'Retry', primary: true }, { id: 'show-list', label: 'Show the list', primary: false }],
      atTop: true,
    }
  } else if (!c.objects) {
    card = {
      kind: 'empty',
      title: 'No objects in this scene',
      body: 'Objects can’t be added live yet. An agent can add them in the file and save; they show up here.',
      actions: c.askAgent ? [{ id: 'ask', label: `Ask ${c.askAgent}`, primary: true }] : [],
    }
  }

  // The chips: what the picture on screen is, in words.
  const behind = pv.want != null && shown ? pv.want - (pv.shown as number) : 0
  const lag = behind > 1 ? `${behind} changes behind` : 'one change behind'
  if (pv.state === 'stale' && shown)
    chips.push(
      pv.retrying
        ? { id: 'updating', tone: 'busy', text: 'Trying again to update the picture…' }
        : { id: 'updating', tone: 'busy', text: 'Updating picture…', title: `The picture is ${lag} while the new one is drawn; the boxes are already live.` },
    )
  if (pv.state === 'failed' && shown) chips.push({ id: 'failed', tone: 'bad', text: pv.again ? 'Tried again, still couldn’t update it' : 'Couldn’t update the picture', action: { id: 'retry', label: 'Retry' } })
  if (pv.state === 'ready' && pv.recovered && pv.landed) chips.push({ id: 'updated', tone: 'ok', text: 'Picture updated' })
  if (c.markers && shown) chips.push({ id: 'markers', tone: 'info', text: 'Objects without a shape are shown as small markers', title: 'These objects have no shape; the picture draws each as a small marker so you can see where it sits.' })
  if (pv.withoutModifiers && shown) chips.push({ id: 'nomod', tone: 'info', text: 'Shown without modifiers', title: 'Drawn from the base shapes: modifiers (bevels, arrays, subdivision…) aren’t applied in this picture.' })
  if (c.approx && shown)
    chips.push(
      c.approx.driven
        ? { id: 'approx', tone: 'info', text: `${c.approx.name} is approximate`, title: 'Animation, a driver or a constraint sets it: its box shows the stored place, which may differ from the picture.' }
        : { id: 'approx', tone: 'info', text: `${c.approx.name}’s box is approximate`, title: 'A modifier adds geometry its box leaves out.' },
    )
  if (c.tier === 'blob') chips.push({ id: 'saved', tone: 'info', text: 'Picture of the saved version' })
  if (c.tier === 'paused') chips.push({ id: 'paused', tone: 'info', text: 'Picture of the last live version' })

  return {
    card,
    chips,
    older: pv.state === 'failed' && shown ? 'Older picture: one change behind' : null,
    still: shown && !c.loading,
    ground: pv.state === 'nopic' && !c.loading,
    dim,
  }
}

/** The details popover's first row: how current the picture is. */
export function pictureFreshness(pv: PictureStatus): string | null {
  if (pv.shown == null) return null
  if (pv.reusedFrom != null) return 'Up to date (a rename isn’t redrawn)'
  if (pv.state === 'stale') return 'One change behind; redrawing'
  if (pv.state === 'failed') return 'One change behind; the redraw failed'
  return 'Up to date'
}
