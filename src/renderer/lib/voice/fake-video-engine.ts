// Video in voice rooms: a stand-in for Sew's plugin video API, for vitest and the
// dev video demo. No camera, no network. Tests push prepared frames with
// `push`; the demo feeds each sink from a `FrameSource` (synthetic VP8 from the
// webview's own `VideoEncoder`, `synthetic-video-source.ts`).

import { NativeVoiceError } from './native-voice-engine'
import {
  LOCAL_CAMERA_TRACK,
  LOCAL_SCREEN_TRACK,
  LOCAL_VIDEO_OFF,
  type LocalVideo,
  type ScreenSource,
  type SinkInfo,
  type SinkRequest,
  type SinkUpdate,
  type VideoCaps,
  type VideoDevicesInfo,
  type VideoEngine,
  type VideoEvents,
  type VideoPolicy,
  type VideoSnapshot,
  type VideoTrack,
} from './video-engine'
import { encodeFrame } from './video-frame'

/** Makes the frames for one sink until stopped. `send` takes a whole message
 *  (header and payload, `encodeFrame`). */
export interface FrameSource {
  start(sink: { sinkId: number; track: string; selfView: boolean }, send: (data: ArrayBuffer) => void): void
  /** The plugin's answer to `request_keyframe`. */
  keyframe(): void
  stop(): void
}

export interface FakeVideoOptions {
  caps?: Partial<VideoCaps>
  devices?: VideoDevicesInfo
  screenSources?: ScreenSource[]
  /** Frames for each opened sink (the demo). Tests push them by hand instead. */
  source?: (track: string) => FrameSource | null
  /** `video_sink` answers `decoded` for these tracks. */
  decodedTracks?: string[]
  /** The call's media policy in the snapshot (default: everything allowed). */
  policy?: VideoPolicy | null
}

export const FAKE_POLICY: VideoPolicy = { videoEnabled: true, cameraAllowed: true, screenAllowed: true }

export const FAKE_CAPS: VideoCaps = {
  camera: true,
  screen: { supported: true, systemPicker: false, list: true, windows: true },
  publishCodecs: ['vp8'],
  decodeCodecs: ['vp8', 'h264'],
  backend: 'fake',
}

export const FAKE_DEVICES: VideoDevicesInfo = {
  cameras: [
    { id: 'cam:1', label: 'Acme HD Camera', facing: 'external', isDefault: true },
    { id: 'cam:2', label: 'Globex Desk Camera', facing: 'external', isDefault: false },
  ],
  camera: null,
  permission: { camera: 'granted', screen: 'granted' },
}

interface FakeSink {
  info: SinkInfo
  req: SinkRequest
  onFrame: (data: ArrayBuffer) => void
  source: FrameSource | null
  updates: SinkUpdate[]
  keyframes: number
  seq: number
}

export class FakeVideoEngine implements VideoEngine {
  /** Every command, as `name {json args}`, for assertions. */
  readonly calls: string[] = []
  readonly sinks = new Map<number, FakeSink>()
  local: LocalVideo = LOCAL_VIDEO_OFF
  tracks: VideoTrack[] = []
  /** The next `camera` / `screen` command rejects with this. */
  private refusals: Array<{ command: string; code: string; detail: unknown }> = []
  private events: VideoEvents | null = null
  private nextSink = 1
  private readonly caps_: VideoCaps

  constructor(private readonly opts: FakeVideoOptions = {}) {
    this.caps_ = { ...FAKE_CAPS, ...opts.caps }
  }

  private log(name: string, args?: unknown): void {
    this.calls.push(args === undefined ? name : `${name} ${JSON.stringify(args)}`)
  }

  async start(events: VideoEvents): Promise<void> {
    this.events = events
  }

  stop(): void {
    this.events = null
    for (const s of this.sinks.values()) s.source?.stop()
  }

  async snapshot(): Promise<VideoSnapshot> {
    return { tracks: this.tracks, local: this.local, policy: this.opts.policy === undefined ? FAKE_POLICY : this.opts.policy }
  }

  async caps(): Promise<VideoCaps> {
    return this.caps_
  }

  async devices(): Promise<VideoDevicesInfo> {
    return this.opts.devices ?? FAKE_DEVICES
  }

  async setDevice(camera: string): Promise<void> {
    this.log('set_video_device', { camera })
  }

  /** The next `command` rejects with `{code, detail}`. */
  refuseNext(command: 'camera' | 'screen' | 'screen_sources', code: string, detail: unknown = null): void {
    this.refusals.push({ command, code, detail })
  }

  private refused(command: string): void {
    const i = this.refusals.findIndex((r) => r.command === command)
    if (i < 0) return
    const [r] = this.refusals.splice(i, 1)
    throw new NativeVoiceError(r.code, r.detail)
  }

  async camera(on: boolean, device?: string): Promise<void> {
    this.log('camera', { on, ...(device ? { device } : {}) })
    this.refused('camera')
    this.setLocal({ ...this.local, camera: on ? 'on' : 'off', reason: null })
  }

  async screenSources(): Promise<ScreenSource[]> {
    this.log('screen_sources')
    this.refused('screen_sources')
    return this.opts.screenSources ?? [
      { id: 'display:1', kind: 'display', title: 'Built-in Display', app: null, width: 1512, height: 982, thumbnail: null },
      { id: 'window:42', kind: 'window', title: 'Quarterly plan', app: 'Acme Docs', width: 1200, height: 800, thumbnail: null },
    ]
  }

  async screen(on: boolean, source?: string): Promise<void> {
    this.log('screen', { on, ...(source ? { source } : {}) })
    this.refused('screen')
    this.setLocal({ ...this.local, screen: on ? 'on' : 'off', reason: null })
  }

  async preview(on: boolean, device?: string): Promise<string | null> {
    this.log('preview', { on, ...(device ? { device } : {}) })
    return on ? LOCAL_CAMERA_TRACK : null
  }

  async openSink(req: SinkRequest, onFrame: (data: ArrayBuffer) => void): Promise<SinkInfo> {
    this.log('video_sink', req)
    const sinkId = this.nextSink++
    const decoded = this.opts.decodedTracks?.includes(req.track) === true || req.decode.length === 0
    const track = this.tracks.find((t) => t.track === req.track)
    const info: SinkInfo = { sinkId, codec: decoded ? null : (track?.codec ?? 'vp8'), mode: decoded ? 'decoded' : 'encoded' }
    const source = this.opts.source?.(req.track) ?? null
    const sink: FakeSink = { info, req, onFrame, source, updates: [], keyframes: 0, seq: 0 }
    this.sinks.set(sinkId, sink)
    const selfView = req.track === LOCAL_CAMERA_TRACK || req.track === LOCAL_SCREEN_TRACK
    source?.start({ sinkId, track: req.track, selfView }, (data) => {
      if (this.sinks.get(sinkId) === sink && (sink.updates.at(-1)?.visible ?? req.visible)) onFrame(data)
    })
    return info
  }

  async updateSink(sinkId: number, patch: SinkUpdate): Promise<void> {
    this.log('video_sink_update', { sink_id: sinkId, ...patch })
    this.sinks.get(sinkId)?.updates.push(patch)
  }

  async closeSink(sinkId: number): Promise<void> {
    this.log('video_sink_close', { sink_id: sinkId })
    this.sinks.get(sinkId)?.source?.stop()
    this.sinks.delete(sinkId)
  }

  async requestKeyframe(sinkId: number): Promise<void> {
    this.log('request_keyframe', { sink_id: sinkId })
    const s = this.sinks.get(sinkId)
    if (!s) return
    s.keyframes += 1
    s.source?.keyframe()
  }

  // ── what the call would do ────────────────────────────────────────

  setTracks(tracks: VideoTrack[]): void {
    this.tracks = tracks
    this.events?.onVideo(tracks)
  }

  setLocal(local: LocalVideo): void {
    this.local = local
    this.events?.onLocalVideo(local)
  }

  /** `media_policy_updated`. */
  setPolicy(p: VideoPolicy): void {
    this.events?.onPolicy(p)
  }

  emitError(code: string, detail: unknown = null): void {
    this.events?.onError(code, detail)
  }

  emitDevices(d: VideoDevicesInfo): void {
    this.events?.onDevices(d)
  }

  /** The open sink for a track. */
  sinkFor(track: string): FakeSink | null {
    return [...this.sinks.values()].find((s) => s.req.track === track) ?? null
  }

  /** Send a prepared frame to the sink open on `track`. */
  push(track: string, header: Parameters<typeof encodeFrame>[0], payload: Uint8Array): void {
    const s = this.sinkFor(track)
    if (!s) throw new Error(`fake video: no sink open on ${track}`)
    s.seq += 1
    s.onFrame(encodeFrame({ sinkId: s.info.sinkId, seq: s.seq, ...header }, payload))
  }
}

/** A remote camera track, for tests and the demo. */
export function fakeTrack(memberId: string, over: Partial<VideoTrack> = {}): VideoTrack {
  return {
    memberId,
    isSelf: false,
    source: 'camera',
    track: `TR_${memberId}`,
    codec: 'vp8',
    width: 1280,
    height: 720,
    muted: false,
    paused: null,
    unsupported: false,
    ...over,
  }
}
