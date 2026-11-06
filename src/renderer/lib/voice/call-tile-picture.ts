// The floating call tile's picture: whose video it shows. Pure.
//
// K2 Companion's phone tile shows the active speaker's video (Sew's
// active-speaker sink, which holds a speaker for 1.5 s), else your own
// camera's self-view. The desktop has no active-speaker sink: it has each
// person's `speaking` flag from Sew's `speaking` levels, so the same 1.5 s
// hold runs here (`nextSpeaker`).

import type { CallPerson, VideoTile } from './video-labels'

/** A speaker keeps the picture this long after they stop, before someone
 *  else who is talking takes it (Sew's active-speaker hold). */
export const SPEAKER_HOLD_MS = 1_500

export interface SpeakerHold {
  /** The member whose video the tile follows. Null: nobody has spoken yet. */
  id: string | null
  /** When they were last heard (ms). */
  heardAt: number
  /** They were talking at the last update (so they're heard until now). */
  talking: boolean
}

export const NO_SPEAKER: SpeakerHold = { id: null, heardAt: 0, talking: false }

/** Who is speaking now: other people only (your own voice never takes the
 *  picture), in the call's order. */
export function speakingIds(people: readonly CallPerson[]): string[] {
  return people.filter((p) => p.speaking && !p.isSelf && p.memberId).map((p) => p.memberId as string)
}

/** The active speaker after who is `speaking` at `now`. Call it on every
 *  change of who is speaking, and again when `holdLeft` runs out. The current
 *  speaker stays while they talk and for `holdMs` after they stop; then the
 *  first person talking takes over. Nobody talking keeps the last speaker. */
export function nextSpeaker(prev: SpeakerHold, speaking: readonly string[], now: number, holdMs = SPEAKER_HOLD_MS): SpeakerHold {
  if (prev.id !== null && speaking.includes(prev.id)) return { id: prev.id, heardAt: now, talking: true }
  const lastHeard = prev.talking ? now : prev.heardAt
  if (speaking.length > 0 && (prev.id === null || now - lastHeard >= holdMs)) return { id: speaking[0], heardAt: now, talking: true }
  return { id: prev.id, heardAt: lastHeard, talking: false }
}

/** How long until the hold can pass to someone else (ms); 0: it can now. */
export function holdLeft(prev: SpeakerHold, now: number, holdMs = SPEAKER_HOLD_MS): number {
  if (prev.id === null) return 0
  if (prev.talking) return holdMs
  return Math.max(0, prev.heardAt + holdMs - now)
}

/** A tile with video on it (live, paused, starting, or one this computer
 *  can't decode). A camera that's off has no video. */
export function hasVideo(t: VideoTile): boolean {
  return t.state !== 'off'
}

/** Video is on in the call: anyone's camera or screen share, yours included.
 *  The floating tile shows only then. */
export function videoLive(tiles: readonly VideoTile[]): boolean {
  return tiles.some(hasVideo)
}

const liveFirst = (a: VideoTile, b: VideoTile): number => (a.state === 'live' ? 0 : 1) - (b.state === 'live' ? 0 : 1)

/** The tile the floating card shows: the active speaker's video (their
 *  screen share first, then their camera), else someone else's (screens
 *  first, live ones first), else your own camera's self-view, else your
 *  screen. Null: no video in the call. */
export function tilePicture(tiles: readonly VideoTile[], speaker: string | null): VideoTile | null {
  const withVideo = tiles.filter(hasVideo)
  const remote = withVideo.filter((t) => !t.self)
  if (speaker !== null) {
    const theirs = remote.filter((t) => t.memberId === speaker)
    const pick = theirs.find((t) => t.source === 'screen') ?? theirs.find((t) => t.source === 'camera')
    if (pick) return pick
  }
  const others = [...remote.filter((t) => t.source === 'screen').sort(liveFirst), ...remote.filter((t) => t.source === 'camera').sort(liveFirst)]
  if (others.length > 0) return others[0]
  const own = withVideo.filter((t) => t.self)
  return own.find((t) => t.source === 'camera') ?? own.find((t) => t.source === 'screen') ?? null
}
