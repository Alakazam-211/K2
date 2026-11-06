// Video in K2's voice rooms: the seam between the room UI and whatever makes the
// video. The K2 app's real one is Sew's plugin (`native-video-engine.ts`, Sew's
// plugin video API §B7, its open points settled by Sew on 2026-11-06);
// `fake-video-engine.ts` stands in for it in tests and in the dev video demo.
//
// The call itself stays the voice engine's (`native-voice-engine.ts`). Video
// rides on it: no call, no video.

import type { VideoCodec } from './video-frame'

export type { VideoCodec } from './video-frame'

export interface VideoCaps {
  camera: boolean
  screen: {
    supported: boolean
    /** The OS shows its own picker (macOS 14+, Wayland's portal). */
    systemPicker: boolean
    /** `screen_sources` lists what can be shared. */
    list: boolean
    /** Single windows can be shared, not only whole screens. */
    windows: boolean
  }
  publishCodecs: VideoCodec[]
  decodeCodecs: string[]
  /** The engine receives other people's cameras and screens (Sew 0.8.2). */
  receive: boolean
  /** `local:camera` and `local:screen` show this device's own camera and
   *  screen during a call, not only the preview. */
  selfView: boolean
  /** Picking another camera switches the one being sent now. */
  liveSwitch: boolean
  /** `null`: a plugin built without video (everything above is off). */
  backend: string
}

/** The backend a plugin built without video reports. */
export const NO_VIDEO_BACKEND = 'null'

/** This build can show or send video: the engine receives, or it can send a
 *  camera or a screen in a codec it encodes. Otherwise the room stays voice
 *  only (a plugin built without video, or one whose video isn't wired yet).
 *  The backend alone doesn't decide: an engine with no capture backend still
 *  receives (backend `null`, `receive` true; the webview decodes). */
export function videoUsable(caps: VideoCaps): boolean {
  const send = (caps.camera || caps.screen.supported) && caps.publishCodecs.length > 0
  return caps.receive || send
}

export interface CameraDevice {
  id: string
  label: string
  /** `front`, `back`, `external`… (phones mostly). */
  facing: string | null
  isDefault: boolean
}

/** The OS's answer for a capture permission, as the plugin reports it.
 *  `denied` covers the OS's restricted case; `unsupported`: no such capture on
 *  this OS or build. */
export type CapturePermission = 'granted' | 'denied' | 'prompt' | 'unsupported'

export interface VideoDevicesInfo {
  cameras: CameraDevice[]
  /** The camera the next (or live) publish uses. Null: the OS default. */
  camera: string | null
  permission: { camera: CapturePermission; screen: CapturePermission }
}

export type VideoSource = 'camera' | 'screen'

/** Why a track's frames stopped while it's still published. */
export type VideoPause = 'congestion' | 'background' | 'policy'

/** One published video track in the call (`sew-voice://video`). */
export interface VideoTrack {
  memberId: string
  isSelf: boolean
  source: VideoSource
  /** The remote `track_sid`: what a sink opens. */
  track: string
  codec: string
  width: number
  height: number
  /** The sender turned it off (the camera's off, the tile shows their avatar). */
  muted: boolean
  paused: VideoPause | null
  /** This device can't decode it. */
  unsupported: boolean
}

export type LocalVideoState = 'off' | 'asking' | 'on' | 'paused'

/** This person's own camera and screen (`sew-voice://local_video`). */
export interface LocalVideo {
  camera: LocalVideoState
  screen: LocalVideoState
  /** A reject code, `background`, `thermal`, `user_stopped`, `source_closed` or `video_disabled`. */
  reason: string | null
}

export const LOCAL_VIDEO_OFF: LocalVideo = { camera: 'off', screen: 'off', reason: null }

export interface ScreenSource {
  id: string
  kind: 'display' | 'window'
  title: string
  app: string | null
  width: number
  height: number
  /** A PNG (at most 320 px wide), as a data URL ready for an <img>. */
  thumbnail: string | null
}

export type SinkTile = 'large' | 'grid'

export interface SinkRequest {
  /** A remote `track_sid`, `local:camera` or `local:screen`. */
  track: string
  /** Device pixels. */
  width: number
  height: number
  visible: boolean
  tile: SinkTile
  /** What this webview's WebCodecs decodes (the probe). Empty: decoded frames. */
  decode: VideoCodec[]
}

export interface SinkInfo {
  /** u32, the frame header's `sink_id`. */
  sinkId: number
  /** Null in decoded mode, and for a self-view sink opened before the camera
   *  runs (every frame header names the codec too). */
  codec: string | null
  /** `encoded` frames go to WebCodecs; `decoded` frames are I420 or RGBA. */
  mode: 'encoded' | 'decoded'
}

export interface SinkStats {
  decoded: number
  dropped: number
  freezes: number
  freezeMs: number
}

export interface SinkUpdate {
  width?: number
  height?: number
  visible?: boolean
  tile?: SinkTile
  stats?: SinkStats
}

/** What the UI reads of Sew's `MediaPolicy` (sew-proto's JSON). */
export interface VideoPolicy {
  /** `[voice.video] enabled` on the Sew install. */
  videoEnabled: boolean
  /** This session may hold a camera lease (the K2 pass has `voice:video`). */
  cameraAllowed: boolean
  /** This session may hold a screen-share lease. */
  screenAllowed: boolean
}

/** What a window opened mid-call needs (the plugin's `state`). */
export interface VideoSnapshot {
  tracks: VideoTrack[]
  local: LocalVideo
  /** Null: not in a call yet. */
  policy: VideoPolicy | null
}

export interface VideoEvents {
  onVideo(tracks: VideoTrack[]): void
  onLocalVideo(local: LocalVideo): void
  onDevices(devices: VideoDevicesInfo): void
  onPolicy(policy: VideoPolicy): void
  /** Video's own engine errors: `video_stalled`, `encoder_lost`, `camera_lost`. */
  onError(code: string, detail: unknown): void
}

export interface VideoEngine {
  /** Subscribe to the video events, for this window's life. */
  start(events: VideoEvents): Promise<void>
  stop(): void
  snapshot(): Promise<VideoSnapshot>
  /** Rejects when this build's plugin has no video. */
  caps(): Promise<VideoCaps>
  devices(): Promise<VideoDevicesInfo>
  /** `default` follows the OS default. */
  setDevice(camera: string): Promise<void>
  /** The OS camera permission, without asking. */
  cameraPermission(): Promise<CapturePermission>
  /** Asks the OS for the camera (its prompt). Only on the person's own button
   *  press: the plugin never asks by itself. */
  requestCameraPermission(): Promise<CapturePermission>
  /** Resolves once publishing (or off). Rejects with a `NativeVoiceError`. */
  camera(on: boolean, device?: string): Promise<void>
  screenSources(): Promise<ScreenSource[]>
  screen(on: boolean, source?: string): Promise<void>
  /** The camera outside a call: the sink track to attach, or null when off. */
  preview(on: boolean, device?: string): Promise<string | null>
  /** `onFrame` gets each raw Channel message (`video-frame.ts`). */
  openSink(req: SinkRequest, onFrame: (data: ArrayBuffer) => void): Promise<SinkInfo>
  updateSink(sinkId: number, patch: SinkUpdate): Promise<void>
  closeSink(sinkId: number): Promise<void>
  requestKeyframe(sinkId: number): Promise<void>
}

/** The self-view sinks (§B7.5). */
export const LOCAL_CAMERA_TRACK = 'local:camera'
export const LOCAL_SCREEN_TRACK = 'local:screen'

/** Sew's per-room caps (`docs/voice.md`): what a refusal names. */
export const ROOM_CAMERA_LIMIT = 25
export const ROOM_SCREEN_LIMIT = 3
