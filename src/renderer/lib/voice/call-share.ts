// A screen share in the middle (Rosson 2026-11-10): while someone shares, in
// the expanded view and the call's own window, the share takes the middle and
// the people become a column of smaller tiles on the left. Pure.
//
// Which share is in the middle, with several:
//  1. the one this person picked (a click on it in the column), while it lasts;
//  2. else the first share someone else is showing, in the room's order;
//  3. else your own (it's the only one).
// The others sit at the top of the column, each a click away from the middle.
// It never follows the active speaker: a share jumping away mid-sentence is
// worse than one that stays put.
//
// The column has everyone in the call: a camera tile for each person with
// one, and a tile with their picture (Camera off) for each person without,
// so a share with nobody's camera on still shows who is there, who is
// talking and who is muted. You come last.

import { hasVideo } from './call-tile-picture'
import type { CallPerson, VideoTile } from './video-labels'

export interface ShareLayout {
  /** The share in the middle. */
  main: VideoTile
  /** The other shares, at the top of the column. */
  others: VideoTile[]
  /** Everyone in the call, as tiles. */
  people: VideoTile[]
}

/** A share with something on it (not stopped). */
export function isShare(t: VideoTile): boolean {
  return t.source === 'screen' && hasVideo(t)
}

/** The share for the middle: the pick while it's there, else the first of
 *  someone else's, else your own. Null: nobody is sharing. */
export function mainShare(tiles: readonly VideoTile[], picked: string | null): VideoTile | null {
  const shares = tiles.filter(isShare)
  if (shares.length === 0) return null
  return shares.find((t) => t.key === picked) ?? shares.find((t) => !t.self) ?? shares[0]
}

/** A tile for someone with no camera tile: their picture and Camera off. */
function personTile(p: CallPerson): VideoTile {
  return {
    key: `person:${p.memberId ?? p.user}`,
    memberId: p.memberId ?? null,
    self: p.isSelf === true,
    source: 'camera',
    name: p.isSelf ? 'You' : p.user,
    speaking: p.speaking,
    audioMuted: p.muted,
    state: 'off',
    paused: null,
    mirrored: false,
    large: false,
  }
}

/** The people column: in the call's order, you last; a camera tile where
 *  there is one, else a picture tile. Camera tiles for someone the call
 *  doesn't list (yet) follow. */
export function peopleTiles(tiles: readonly VideoTile[], people: readonly CallPerson[]): VideoTile[] {
  const cameras = tiles.filter((t) => t.source === 'camera')
  const used = new Set<string>()
  const out: VideoTile[] = []
  const ordered = [...people.filter((p) => !p.isSelf), ...people.filter((p) => p.isSelf)]
  for (const p of ordered) {
    const cam = cameras.find((t) => !used.has(t.key) && (p.isSelf ? t.self : !t.self && t.memberId !== null && t.memberId === p.memberId))
    if (cam) {
      used.add(cam.key)
      out.push({ ...cam, large: false })
    } else {
      out.push(personTile(p))
    }
  }
  for (const t of cameras) if (!used.has(t.key)) out.push({ ...t, large: false })
  return out
}

/** The share layout, or null when nobody is sharing (the grid as before). */
export function shareLayout(tiles: readonly VideoTile[], people: readonly CallPerson[], picked: string | null): ShareLayout | null {
  const main = mainShare(tiles, picked)
  if (!main) return null
  const others = tiles.filter((t) => isShare(t) && t.key !== main.key).map((t) => ({ ...t, large: false }))
  return { main: { ...main, large: true }, others, people: peopleTiles(tiles, people) }
}
