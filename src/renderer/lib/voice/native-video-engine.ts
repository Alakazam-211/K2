// Video in K2's voice rooms, from Sew's plugin (`tauri-plugin-sew-voice`), against
// the DRAFT of its video API (prd-sew-62 §B7, sent to K2 2026-11-06). Names and
// shapes may change before the plugin lands; `native-video-contract.json` holds
// what this file reads, so a change shows up as a failing test.
//
// Commands are `plugin:sew-voice|<name>` and reject `{code, detail}`. Arguments
// are snake_case, as the plugin's `consent` takes them. Events go to every
// window, like the voice ones. Frames come over one Tauri raw Channel per sink:
// binary, no JSON on the hot path (`video-frame.ts`).

import { NativeVoiceError, PLUGIN_COMMAND_PREFIX, EVENT_PREFIX, toNativeError, type NativeVoiceBridge } from './native-voice-engine'
import type {
  CameraDevice,
  CapturePermission,
  LocalVideo,
  LocalVideoState,
  ScreenSource,
  SinkInfo,
  SinkRequest,
  SinkUpdate,
  VideoCaps,
  VideoDevicesInfo,
  VideoEngine,
  VideoEvents,
  VideoPause,
  VideoSnapshot,
  VideoTrack,
} from './video-engine'
import { LOCAL_VIDEO_OFF } from './video-engine'
import type { VideoCodec } from './video-frame'

/** The voice bridge, plus a Tauri `Channel` for a sink's frames. */
export interface NativeVideoBridge extends NativeVoiceBridge {
  /** A new Channel whose messages go to `onMessage`. Passed as a command argument. */
  channel(onMessage: (data: unknown) => void): unknown
}

/** The engine errors that are video's (the voice engine handles the rest). */
export const VIDEO_ERROR_CODES = new Set(['video_stalled', 'encoder_lost', 'camera_lost'])

function record(v: unknown): Record<string, unknown> | null {
  return v && typeof v === 'object' && !Array.isArray(v) ? (v as Record<string, unknown>) : null
}

function badShape(what: string, v: unknown): Error {
  return new Error(`sew-voice video: unexpected ${what}: ${JSON.stringify(v)}`)
}

function codecList(v: unknown, what: string): VideoCodec[] {
  if (!Array.isArray(v)) throw badShape(what, v)
  return v.filter((c): c is VideoCodec => c === 'vp8' || c === 'h264')
}

export function parseVideoCaps(v: unknown): VideoCaps {
  const r = record(v)
  const s = record(r?.screen)
  if (!r || !s || typeof r.camera !== 'boolean' || typeof s.supported !== 'boolean') throw badShape('video_caps', v)
  return {
    camera: r.camera,
    screen: { supported: s.supported, systemPicker: s.system_picker === true, list: s.list === true, windows: s.windows === true },
    publishCodecs: codecList(r.publish_codecs, 'publish_codecs'),
    decodeCodecs: Array.isArray(r.decode_codecs) ? r.decode_codecs.filter((c): c is string => typeof c === 'string') : [],
    backend: typeof r.backend === 'string' ? r.backend : '',
  }
}

function permission(v: unknown): CapturePermission {
  return v === 'granted' || v === 'denied' || v === 'prompt' ? v : 'unknown'
}

export function parseVideoDevices(v: unknown): VideoDevicesInfo {
  const r = record(v)
  if (!r || !Array.isArray(r.cameras)) throw badShape('video_devices', v)
  const cameras: CameraDevice[] = r.cameras.map((raw) => {
    const c = record(raw)
    if (!c || typeof c.id !== 'string' || typeof c.name !== 'string') throw badShape('camera', raw)
    return { id: c.id, label: c.name, facing: typeof c.facing === 'string' ? c.facing : null, isDefault: c.default === true }
  })
  const p = record(r.permission)
  return {
    cameras,
    camera: typeof r.camera === 'string' ? r.camera : null,
    permission: { camera: permission(p?.camera), screen: permission(p?.screen) },
  }
}

const PAUSES: ReadonlySet<string> = new Set(['congestion', 'background', 'policy'])

export function parseVideoTracks(v: unknown): VideoTrack[] {
  if (!Array.isArray(v)) throw badShape('video', v)
  return v.map((raw) => {
    const r = record(raw)
    if (
      !r ||
      typeof r.member_id !== 'string' ||
      typeof r.track !== 'string' ||
      (r.source !== 'camera' && r.source !== 'screen') ||
      (r.paused !== null && r.paused !== undefined && !(typeof r.paused === 'string' && PAUSES.has(r.paused)))
    ) {
      throw badShape('video track', raw)
    }
    return {
      memberId: r.member_id,
      isSelf: r.is_self === true,
      source: r.source,
      track: r.track,
      codec: typeof r.codec === 'string' ? r.codec : '',
      width: typeof r.width === 'number' ? r.width : 0,
      height: typeof r.height === 'number' ? r.height : 0,
      muted: r.muted === true,
      paused: typeof r.paused === 'string' ? (r.paused as VideoPause) : null,
      unsupported: r.unsupported === true,
    }
  })
}

const LOCAL_STATES: ReadonlySet<string> = new Set(['off', 'asking', 'on', 'paused'])

export function parseLocalVideo(v: unknown): LocalVideo {
  const r = record(v)
  if (!r || typeof r.camera !== 'string' || !LOCAL_STATES.has(r.camera) || typeof r.screen !== 'string' || !LOCAL_STATES.has(r.screen)) {
    throw badShape('local_video', v)
  }
  return { camera: r.camera as LocalVideoState, screen: r.screen as LocalVideoState, reason: typeof r.reason === 'string' ? r.reason : null }
}

export function parseScreenSources(v: unknown): ScreenSource[] {
  if (!Array.isArray(v)) throw badShape('screen_sources', v)
  return v.map((raw) => {
    const r = record(raw)
    if (!r || typeof r.id !== 'string' || typeof r.title !== 'string') throw badShape('screen source', raw)
    const png = typeof r.thumbnail_png === 'string' && r.thumbnail_png !== '' ? r.thumbnail_png : null
    return {
      id: r.id,
      kind: typeof r.kind === 'string' ? r.kind : 'display',
      title: r.title,
      app: typeof r.app === 'string' ? r.app : null,
      width: typeof r.width === 'number' ? r.width : 0,
      height: typeof r.height === 'number' ? r.height : 0,
      thumbnail: png === null ? null : png.startsWith('data:') ? png : `data:image/png;base64,${png}`,
    }
  })
}

export function parseSinkInfo(v: unknown): SinkInfo {
  const r = record(v)
  if (!r || typeof r.sink_id !== 'number' || (r.mode !== 'encoded' && r.mode !== 'decoded')) throw badShape('video_sink answer', v)
  return { sinkId: r.sink_id, codec: typeof r.codec === 'string' ? r.codec : null, mode: r.mode }
}

/** A raw Channel message as bytes. Tauri hands `InvokeResponseBody::Raw` over as
 *  an ArrayBuffer; older transports used a number array. */
export function channelBytes(data: unknown): ArrayBuffer {
  if (data instanceof ArrayBuffer) return data
  if (ArrayBuffer.isView(data)) return data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength) as ArrayBuffer
  if (Array.isArray(data)) return new Uint8Array(data as number[]).buffer
  throw badShape('frame message', typeof data)
}

export class NativeVideoEngine implements VideoEngine {
  private unlisten: Array<() => void> = []

  constructor(private readonly bridge: NativeVideoBridge) {}

  private async command<T>(name: string, args?: Record<string, unknown>): Promise<T> {
    try {
      return await this.bridge.invoke<T>(`${PLUGIN_COMMAND_PREFIX}${name}`, args)
    } catch (err) {
      throw toNativeError(err)
    }
  }

  async start(events: VideoEvents): Promise<void> {
    const on = async (name: string, handle: (payload: unknown) => void): Promise<void> => {
      this.unlisten.push(await this.bridge.listen(`${EVENT_PREFIX}${name}`, handle))
    }
    await Promise.all([
      on('video', (p) => events.onVideo(parseVideoTracks(p))),
      on('local_video', (p) => events.onLocalVideo(parseLocalVideo(p))),
      on('video_devices', (p) => events.onDevices(parseVideoDevices(p))),
      on('policy', (p) => events.onPolicy(p)),
      on('errors', (p) => {
        const r = record(p)
        if (r && typeof r.code === 'string' && VIDEO_ERROR_CODES.has(r.code)) events.onError(r.code, r.detail ?? null)
      }),
    ])
  }

  stop(): void {
    for (const off of this.unlisten) off()
    this.unlisten = []
  }

  /** `state` gains `video`, `local_video` and `policy`. A plugin without video
   *  leaves them out: no tracks, nothing on. */
  async snapshot(): Promise<VideoSnapshot> {
    const r = record(await this.command<unknown>('state'))
    if (!r) throw badShape('state snapshot', r)
    return {
      tracks: r.video === undefined ? [] : parseVideoTracks(r.video),
      local: r.local_video === undefined ? LOCAL_VIDEO_OFF : parseLocalVideo(r.local_video),
      policy: r.policy ?? null,
    }
  }

  async caps(): Promise<VideoCaps> {
    return parseVideoCaps(await this.command<unknown>('video_caps'))
  }

  async devices(): Promise<VideoDevicesInfo> {
    return parseVideoDevices(await this.command<unknown>('video_devices'))
  }

  async setDevice(camera: string): Promise<void> {
    await this.command<null>('set_video_device', { camera })
  }

  async camera(on: boolean, device?: string): Promise<void> {
    await this.command<null>('camera', { on, ...(device ? { device } : {}) })
  }

  async screenSources(): Promise<ScreenSource[]> {
    return parseScreenSources(await this.command<unknown>('screen_sources'))
  }

  async screen(on: boolean, source?: string): Promise<void> {
    await this.command<null>('screen', { on, ...(source ? { source } : {}) })
  }

  async preview(on: boolean, device?: string): Promise<string | null> {
    const r = record(await this.command<unknown>('preview', { on, ...(device ? { device } : {}) }))
    if (!on) return null
    if (!r || typeof r.sink_track !== 'string') throw badShape('preview answer', r)
    return r.sink_track
  }

  async openSink(req: SinkRequest, onFrame: (data: ArrayBuffer) => void): Promise<SinkInfo> {
    const channel = this.bridge.channel((data) => onFrame(channelBytes(data)))
    const answer = await this.command<unknown>('video_sink', {
      track: req.track,
      channel,
      width: req.width,
      height: req.height,
      visible: req.visible,
      tile: req.tile,
      decode: req.decode,
    })
    return parseSinkInfo(answer)
  }

  async updateSink(sinkId: number, patch: SinkUpdate): Promise<void> {
    const stats = patch.stats
      ? { decoded: patch.stats.decoded, dropped: patch.stats.dropped, freezes: patch.stats.freezes, freeze_ms: patch.stats.freezeMs }
      : undefined
    await this.command<null>('video_sink_update', {
      sink_id: sinkId,
      ...(patch.width !== undefined ? { width: patch.width } : {}),
      ...(patch.height !== undefined ? { height: patch.height } : {}),
      ...(patch.visible !== undefined ? { visible: patch.visible } : {}),
      ...(patch.tile !== undefined ? { tile: patch.tile } : {}),
      ...(stats ? { stats } : {}),
    })
  }

  async closeSink(sinkId: number): Promise<void> {
    await this.command<null>('video_sink_close', { sink_id: sinkId })
  }

  async requestKeyframe(sinkId: number): Promise<void> {
    await this.command<null>('request_keyframe', { sink_id: sinkId })
  }
}

export { NativeVoiceError }
