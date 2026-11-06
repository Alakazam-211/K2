// Video in voice rooms: what the room UI says, and which tiles it draws. Pure.

import {
  LOCAL_CAMERA_TRACK,
  LOCAL_SCREEN_TRACK,
  ROOM_CAMERA_LIMIT,
  ROOM_SCREEN_LIMIT,
  type LocalVideo,
  type VideoPause,
  type VideoSource,
  type VideoTrack,
} from './video-engine'

/** Which OS privacy pane a refusal's button opens. */
export type PrivacyPane = 'camera' | 'screen'

export interface VideoNoticeText {
  line: string
  kind: 'error' | 'info'
  settings?: PrivacyPane
}

function retryAfter(detail: unknown): number | null {
  const r = detail && typeof detail === 'object' ? (detail as Record<string, unknown>) : null
  const n = r?.retry_after
  return typeof n === 'number' && n > 0 ? Math.ceil(n) : null
}

/** A `camera` or `screen` command's refusal, in plain words. Null: say nothing
 *  (the person cancelled the picker). */
export function videoRefusalText(code: string, detail: unknown, what: VideoSource): VideoNoticeText | null {
  const thing = what === 'camera' ? 'your camera' : 'screen sharing'
  switch (code) {
    case 'picker_cancelled':
      return null
    case 'not_in_call':
      return { line: `Join a call first, then turn on ${thing}.`, kind: 'error' }
    case 'video_not_allowed':
      return { line: "Video isn't allowed for you in this room. A room admin can change that.", kind: 'error' }
    case 'video_disabled':
      return { line: 'Video is turned off for this room.', kind: 'error' }
    case 'camera_denied':
      return { line: "K2 isn't allowed to use your camera. Allow K2 in your system's privacy settings.", kind: 'error', settings: 'camera' }
    case 'screen_denied':
      return { line: "K2 isn't allowed to record your screen. Allow K2 in your system's privacy settings, then restart K2.", kind: 'error', settings: 'screen' }
    case 'camera_busy':
      return { line: 'Another app is using your camera. Close it, then try again.', kind: 'error' }
    case 'camera_not_found':
      return { line: "K2 can't find that camera. Plug it in, or pick another camera.", kind: 'error' }
    case 'video_slots_full':
      return { line: `${ROOM_CAMERA_LIMIT} cameras are already on in this room, the most it allows. Try again when someone turns theirs off.`, kind: 'error' }
    case 'screen_slots_full':
      return { line: `${ROOM_SCREEN_LIMIT} people are already sharing their screens in this room, the most it allows. Try again when someone stops.`, kind: 'error' }
    case 'media_busy': {
      const s = retryAfter(detail)
      return { line: s ? `The call server is busy. Try again in ${s} ${s === 1 ? 'second' : 'seconds'}.` : 'The call server is busy. Try again in a moment.', kind: 'error' }
    }
    case 'media_capacity':
      return { line: 'The call server has no room for more video right now.', kind: 'error' }
    case 'codec_unavailable':
      return { line: "This computer can't send video in a format this room uses.", kind: 'error' }
    case 'screen_unsupported':
      return { line: "Screen sharing doesn't work on this system yet.", kind: 'error' }
    default:
      return { line: what === 'camera' ? "K2 couldn't turn on your camera." : "K2 couldn't share your screen.", kind: 'error' }
  }
}

/** Why the camera or screen stopped or paused by itself (`local_video.reason`).
 *  Null: nothing to say (the person turned it off). */
export function localVideoReasonText(reason: string | null, what: VideoSource): VideoNoticeText | null {
  switch (reason) {
    case null:
    case 'user_stopped':
      return null
    case 'background':
      return { line: 'Your camera is paused while K2 is in the background.', kind: 'info' }
    case 'thermal':
      return { line: 'Your camera is paused to let this computer cool down.', kind: 'info' }
    case 'source_closed':
      return { line: 'Screen sharing stopped: the window you shared closed.', kind: 'info' }
    case 'camera_lost':
      return { line: 'Your camera was disconnected.', kind: 'error' }
    default:
      return videoRefusalText(reason, null, what)
  }
}

/** The plugin's video engine errors. */
export function videoEngineErrorText(code: string): VideoNoticeText | null {
  switch (code) {
    case 'camera_lost':
      return { line: 'Your camera was disconnected.', kind: 'error' }
    case 'encoder_lost':
      return { line: 'Your video stopped: this computer could not encode it.', kind: 'error' }
    case 'video_stalled':
      return { line: 'Video stalled. K2 is trying again.', kind: 'info' }
    default:
      return null
  }
}

/** What a tile says over a paused track. */
export function pausedText(p: VideoPause | 'background' | 'thermal'): string {
  switch (p) {
    case 'background':
      return 'Camera paused (in the background)'
    case 'thermal':
      return 'Camera paused (cooling down)'
    case 'congestion':
      return 'Video paused (slow connection)'
    case 'policy':
      return 'Video paused by the room'
  }
}

/** Someone in the call, as the voice store draws them. */
export interface CallPerson {
  memberId?: string
  isSelf?: boolean
  user: string
  speaking: boolean
  muted: boolean
}

export type TileState = 'live' | 'off' | 'paused' | 'unsupported' | 'asking'

export interface VideoTile {
  /** The sink track: a remote `track_sid`, `local:camera` or `local:screen`. */
  key: string
  memberId: string | null
  self: boolean
  source: VideoSource
  name: string
  speaking: boolean
  /** Their microphone is off. */
  audioMuted: boolean
  state: TileState
  /** Set while `state` is `paused`. */
  paused: VideoPause | 'background' | 'thermal' | null
  /** Self-view is mirrored, in CSS only. */
  mirrored: boolean
  /** Screens draw large. */
  large: boolean
}

/** The room's tiles: screens first (large), then cameras in the order the
 *  plugin lists them, own tiles last. */
export function videoTiles(tracks: readonly VideoTrack[], local: LocalVideo, people: readonly CallPerson[]): VideoTile[] {
  const byMember = new Map(people.filter((p) => p.memberId).map((p) => [p.memberId as string, p]))
  const self = people.find((p) => p.isSelf)
  const remote: VideoTile[] = tracks
    .filter((t) => !t.isSelf)
    .map((t) => {
      const who = byMember.get(t.memberId)
      const state: TileState = t.unsupported ? 'unsupported' : t.muted ? 'off' : t.paused ? 'paused' : 'live'
      const name = who?.user ?? 'Someone'
      return {
        key: t.track,
        memberId: t.memberId,
        self: false,
        source: t.source,
        name: t.source === 'screen' ? `${name}'s screen` : name,
        speaking: t.source === 'camera' && (who?.speaking ?? false),
        audioMuted: who?.muted ?? false,
        state,
        paused: state === 'paused' ? t.paused : null,
        mirrored: false,
        large: t.source === 'screen',
      }
    })
  const own: VideoTile[] = []
  if (local.camera !== 'off') {
    const paused = local.camera === 'paused'
    const reason = local.reason === 'thermal' ? 'thermal' : 'background'
    own.push({
      key: LOCAL_CAMERA_TRACK,
      memberId: self?.memberId ?? null,
      self: true,
      source: 'camera',
      name: 'You',
      speaking: self?.speaking ?? false,
      audioMuted: self?.muted ?? false,
      state: local.camera === 'asking' ? 'asking' : paused ? 'paused' : 'live',
      paused: paused ? reason : null,
      mirrored: true,
      large: false,
    })
  }
  if (local.screen === 'on' || local.screen === 'paused') {
    own.push({
      key: LOCAL_SCREEN_TRACK,
      memberId: self?.memberId ?? null,
      self: true,
      source: 'screen',
      name: 'Your screen',
      speaking: false,
      audioMuted: false,
      state: local.screen === 'paused' ? 'paused' : 'live',
      paused: local.screen === 'paused' ? 'background' : null,
      mirrored: false,
      large: false,
    })
  }
  const screens = remote.filter((t) => t.source === 'screen')
  const cameras = remote.filter((t) => t.source === 'camera')
  return [...screens, ...cameras, ...own]
}

/** The tile that draws large when no screen is shared: the last person who
 *  spoke, kept until someone else speaks (so tiles don't jump on every breath).
 *  Only with 3 or more tiles. */
export function withSpeakerEmphasis(tiles: readonly VideoTile[], lastSpeaker: string | null): VideoTile[] {
  if (tiles.some((t) => t.large) || tiles.length < 3 || lastSpeaker === null) return [...tiles]
  const i = tiles.findIndex((t) => !t.self && t.source === 'camera' && t.memberId === lastSpeaker && t.state === 'live')
  if (i < 0) return [...tiles]
  const speaker = { ...tiles[i], large: true }
  return [speaker, ...tiles.slice(0, i), ...tiles.slice(i + 1)]
}
