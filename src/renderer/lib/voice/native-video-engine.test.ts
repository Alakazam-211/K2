// The adapter over the plugin's DRAFT video API, held to native-video-contract.json.

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import { NativeVoiceError, PLUGIN_COMMAND_PREFIX } from './native-voice-engine'
import {
  channelBytes,
  NativeVideoEngine,
  parseLocalVideo,
  parseScreenSources,
  parseVideoCaps,
  parseVideoDevices,
  parseVideoTracks,
  type NativeVideoBridge,
} from './native-video-engine'
import type { LocalVideo, VideoTrack } from './video-engine'

const contract = JSON.parse(readFileSync(resolve(__dirname, 'native-video-contract.json'), 'utf8'))
const P = PLUGIN_COMMAND_PREFIX

function bridge(answers: Record<string, unknown> = {}, rejects: Record<string, unknown> = {}) {
  const calls: Array<[string, Record<string, unknown> | undefined]> = []
  const listeners = new Map<string, (p: unknown) => void>()
  const channels: Array<(data: unknown) => void> = []
  const b: NativeVideoBridge = {
    invoke: async <T,>(command: string, args?: Record<string, unknown>): Promise<T> => {
      calls.push([command, args])
      const name = command.slice(P.length)
      if (name in rejects) throw rejects[name]
      if (!(name in answers)) throw new Error(`unexpected ${command}`)
      return answers[name] as T
    },
    listen: async (event, handler) => {
      listeners.set(event, handler)
      return () => listeners.delete(event)
    },
    channel: (onMessage) => {
      channels.push(onMessage)
      return { id: channels.length }
    },
  }
  return { b, calls, listeners, channels }
}

describe('parsing the draft shapes', () => {
  it('reads video_caps and video_devices', () => {
    expect(parseVideoCaps(contract.videoCaps)).toEqual({
      camera: true,
      screen: { supported: true, systemPicker: false, list: true, windows: true },
      publishCodecs: ['vp8'],
      decodeCodecs: ['vp8', 'h264'],
      backend: 'avfoundation',
    })
    expect(parseVideoDevices(contract.videoDevices)).toEqual({
      cameras: [
        { id: 'cam:1', label: 'FaceTime HD Camera', facing: 'front', isDefault: true },
        { id: 'cam:2', label: 'Acme USB Camera', facing: 'external', isDefault: false },
      ],
      camera: null,
      permission: { camera: 'granted', screen: 'denied' },
    })
  })

  it('reads the video list, paused and muted tracks included', () => {
    const tracks: VideoTrack[] = parseVideoTracks(contract.video)
    expect(tracks.map((t) => [t.memberId, t.source, t.track, t.muted, t.paused, t.isSelf])).toEqual([
      ['7', 'camera', 'TR_VCa', false, null, false],
      ['7', 'screen', 'TR_VSb', false, 'congestion', false],
      ['48831765147549699', 'camera', 'TR_VCc', true, null, true],
    ])
  })

  it('reads local_video with and without a reason', () => {
    const on: LocalVideo = parseLocalVideo(contract.localVideo.on)
    expect(on).toEqual({ camera: 'on', screen: 'off', reason: null })
    expect(parseLocalVideo(contract.localVideo.paused)).toEqual({ camera: 'paused', screen: 'off', reason: 'background' })
  })

  it('turns a base64 thumbnail into a data URL', () => {
    const s = parseScreenSources(contract.screenSources)
    expect(s[0].thumbnail).toBe('data:image/png;base64,iVBORw0KGgo=')
    expect(s[1]).toMatchObject({ kind: 'window', app: 'Acme Docs', thumbnail: null })
  })

  it('throws on a shape it does not know', () => {
    expect(() => parseVideoTracks([{ member_id: '7', track: 't', source: 'hologram' }])).toThrow(/unexpected video track/)
    expect(() => parseLocalVideo({ camera: 'sideways', screen: 'off' })).toThrow(/local_video/)
    expect(() => parseVideoCaps({ screen: {} })).toThrow(/video_caps/)
  })

  it('takes a raw Channel message as an ArrayBuffer, a typed array or a number array', () => {
    const buf = new Uint8Array([1, 2, 3]).buffer
    expect(new Uint8Array(channelBytes(buf))).toEqual(new Uint8Array([1, 2, 3]))
    expect(new Uint8Array(channelBytes(new Uint8Array([9, 1, 2, 3]).subarray(1)))).toEqual(new Uint8Array([1, 2, 3]))
    expect(new Uint8Array(channelBytes([1, 2, 3]))).toEqual(new Uint8Array([1, 2, 3]))
    expect(() => channelBytes('nope')).toThrow(/frame message/)
  })
})

describe('commands', () => {
  it('sends the draft command names with snake_case arguments', async () => {
    const { b, calls, channels } = bridge({
      video_sink: contract.videoSinkResult,
      video_sink_update: null,
      video_sink_close: null,
      request_keyframe: null,
      camera: null,
      screen: null,
      set_video_device: null,
      preview: contract.previewResult,
    })
    const e = new NativeVideoEngine(b)
    const frames: ArrayBuffer[] = []
    const info = await e.openSink({ ...contract.videoSinkArgs }, (f) => frames.push(f))
    expect(info).toEqual({ sinkId: 3, codec: 'vp8', mode: 'encoded' })
    const [cmd, args] = calls[0]
    expect(cmd).toBe(`${P}video_sink`)
    expect({ ...args, channel: undefined }).toEqual({ ...contract.videoSinkArgs, channel: undefined })
    expect(args?.channel).toEqual({ id: 1 })
    channels[0](new Uint8Array([7]).buffer)
    expect(frames.map((f) => new Uint8Array(f)[0])).toEqual([7])

    await e.updateSink(3, { visible: false, stats: { decoded: 10, dropped: 1, freezes: 0, freezeMs: 0 } })
    expect(calls[1]).toEqual([`${P}video_sink_update`, contract.videoSinkUpdateArgs])
    await e.requestKeyframe(3)
    await e.closeSink(3)
    await e.camera(true, 'cam:2')
    await e.screen(true, 'display:1')
    await e.setDevice('default')
    expect(await e.preview(true)).toBe('local:camera')
    expect(calls.slice(2)).toEqual([
      [`${P}request_keyframe`, { sink_id: 3 }],
      [`${P}video_sink_close`, { sink_id: 3 }],
      [`${P}camera`, { on: true, device: 'cam:2' }],
      [`${P}screen`, { on: true, source: 'display:1' }],
      [`${P}set_video_device`, { camera: 'default' }],
      [`${P}preview`, { on: true }],
    ])
  })

  it("rejects with the plugin's code and detail", async () => {
    const { b } = bridge({}, { camera: contract.rejects.media_busy, screen: 'command screen not allowed' })
    const e = new NativeVideoEngine(b)
    const err = await e.camera(true).catch((x: unknown) => x)
    expect(err).toBeInstanceOf(NativeVoiceError)
    expect(err).toMatchObject({ code: 'media_busy', detail: { retry_after: 5 } })
    // Tauri's own refusal (no such command, no permission) is a plain string.
    await expect(e.screen(true)).rejects.toMatchObject({ code: 'failed', detail: 'command screen not allowed' })
  })

  it('reads the snapshot, with or without video in it', async () => {
    const withVideo = new NativeVideoEngine(bridge({ state: contract.snapshotWithVideo }).b)
    const snap = await withVideo.snapshot()
    expect(snap.tracks.map((t) => t.track)).toEqual(['TR_VCa'])
    expect(snap.local).toEqual({ camera: 'on', screen: 'off', reason: null })
    expect(snap.policy).toEqual({ video: true })
    const without = new NativeVideoEngine(bridge({ state: contract.snapshotWithoutVideo }).b)
    expect(await without.snapshot()).toEqual({ tracks: [], local: { camera: 'off', screen: 'off', reason: null }, policy: null })
  })
})

describe('events', () => {
  it('listens to the draft events and passes on only the video errors', async () => {
    const { b, listeners } = bridge()
    const e = new NativeVideoEngine(b)
    const seen: string[] = []
    await e.start({
      onVideo: (t) => seen.push(`video ${t.length}`),
      onLocalVideo: (l) => seen.push(`local ${l.camera}`),
      onDevices: (d) => seen.push(`devices ${d.cameras.length}`),
      onPolicy: () => seen.push('policy'),
      onError: (code) => seen.push(`error ${code}`),
    })
    expect([...listeners.keys()].sort()).toEqual(contract.events.map((n: string) => `sew-voice://${n}`).sort())
    listeners.get('sew-voice://video')?.(contract.video)
    listeners.get('sew-voice://local_video')?.(contract.localVideo.paused)
    listeners.get('sew-voice://video_devices')?.(contract.videoDevices)
    listeners.get('sew-voice://policy')?.({ video: false })
    listeners.get('sew-voice://errors')?.(contract.errors.mic_denied)
    listeners.get('sew-voice://errors')?.(contract.errors.camera_lost)
    expect(seen).toEqual(['video 3', 'local paused', 'devices 2', 'policy', 'error camera_lost'])
    e.stop()
    expect(listeners.size).toBe(0)
  })
})
