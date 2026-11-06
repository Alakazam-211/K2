// The adapter over the plugin's video API (Sew 0.8.2), held to native-video-contract.json
// (which src-tauri's native_voice tests hold to the plugin's real types).

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import { NativeVoiceError, PLUGIN_COMMAND_PREFIX } from './native-voice-engine'
import {
  channelBytes,
  NativeVideoEngine,
  parseLocalVideo,
  parseMediaPolicy,
  parseSinkInfo,
  parseScreenSources,
  parseVideoCaps,
  parseVideoDevices,
  parseVideoTracks,
  type NativeVideoBridge,
} from './native-video-engine'
import { videoUsable, type LocalVideo, type VideoTrack } from './video-engine'
import { localVideoReasonText, videoEngineErrorText, videoRefusalText } from './video-labels'

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

describe('parsing the plugin shapes', () => {
  it('reads video_caps and video_devices', () => {
    expect(parseVideoCaps(contract.videoCaps)).toEqual({
      camera: true,
      screen: { supported: true, systemPicker: true, list: true, windows: true },
      publishCodecs: ['vp8'],
      decodeCodecs: ['vp8', 'h264'],
      receive: true,
      selfView: true,
      liveSwitch: true,
      backend: 'apple',
    })
    expect(parseVideoDevices(contract.videoDevices)).toEqual({
      cameras: [
        { id: 'cam:1', label: 'FaceTime HD Camera', facing: 'front', isDefault: true },
        { id: 'cam:2', label: 'Acme USB Camera', facing: 'external', isDefault: false },
      ],
      camera: null,
      permission: { camera: 'granted', screen: 'unsupported' },
    })
    for (const p of ['granted', 'denied', 'prompt', 'unsupported']) {
      expect(parseVideoDevices({ ...contract.videoDevices, permission: { camera: p, screen: p } }).permission).toEqual({ camera: p, screen: p })
    }
    // Sew's four values only: the OS's restricted case comes as `denied`.
    expect(() => parseVideoDevices({ ...contract.videoDevices, permission: { camera: 'restricted', screen: 'granted' } })).toThrow(/capture permission/)
  })

  it("reads a build without video: everything off, backend 'null'", () => {
    expect(parseVideoCaps(contract.videoCapsWithoutVideo)).toEqual({
      camera: false,
      screen: { supported: false, systemPicker: false, list: false, windows: false },
      publishCodecs: [],
      decodeCodecs: [],
      receive: false,
      selfView: false,
      liveSwitch: false,
      backend: 'null',
    })
  })

  it('reads an older video_caps without receive, self_view and live_switch as false', () => {
    expect(parseVideoCaps(contract.videoCapsOld)).toMatchObject({ receive: false, selfView: false, liveSwitch: false, backend: 'apple' })
    expect(() => parseVideoCaps({ ...contract.videoCaps, receive: 'yes' })).toThrow(/video_caps.receive/)
  })

  it('uses video when the engine receives or can send, and not otherwise', () => {
    const caps = parseVideoCaps(contract.videoCaps)
    expect(videoUsable(caps)).toBe(true)
    expect(videoUsable(parseVideoCaps(contract.videoCapsWithoutVideo))).toBe(false)
    // Receive only (no camera, no screen).
    expect(videoUsable({ ...caps, camera: false, screen: { ...caps.screen, supported: false }, publishCodecs: [] })).toBe(true)
    // Send only, from an older payload.
    expect(videoUsable(parseVideoCaps(contract.videoCapsOld))).toBe(true)
    // A backend whose receive and send aren't wired yet: every other command would answer video_unsupported.
    expect(videoUsable({ ...caps, receive: false, publishCodecs: [] })).toBe(false)
    expect(videoUsable({ ...caps, receive: false, camera: false, screen: { ...caps.screen, supported: false } })).toBe(false)
    // No capture backend on this machine: receive only, backend null (Sew's real engine).
    expect(videoUsable({ ...parseVideoCaps(contract.videoCapsWithoutVideo), receive: true })).toBe(true)
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
    expect(parseLocalVideo(contract.localVideo.hidden)).toEqual({ camera: 'off', screen: 'paused', reason: 'hidden' })
  })

  it('has words for every refusal code and stop reason the plugin names', () => {
    const generic = new Set(["K2 couldn't turn on your camera.", "K2 couldn't share your screen."])
    // Codes that never reach camera or screen: sinks, and the engine errors.
    const notRefusals = new Set(['unknown_track', 'unknown_sink', 'decoded_unsupported', 'decoded_tiles_full', 'video_stalled', 'encoder_lost', 'camera_lost'])
    for (const code of contract.codes as string[]) {
      if (code === 'picker_cancelled') {
        expect(videoRefusalText(code, null, 'screen')).toBeNull()
        continue
      }
      if (notRefusals.has(code)) continue
      for (const what of ['camera', 'screen'] as const) {
        const t = videoRefusalText(code, null, what)
        expect(t, code).not.toBeNull()
        expect(generic.has(t?.line ?? ''), `${code} falls back to the generic line`).toBe(false)
      }
    }
    for (const code of ['video_stalled', 'encoder_lost', 'camera_lost']) expect(videoEngineErrorText(code), code).not.toBeNull()
    for (const reason of [...(contract.reasons as string[]), ...(contract.engineReasons as string[])]) {
      for (const what of ['camera', 'screen'] as const) {
        const t = localVideoReasonText(reason, what)
        if (t !== null) expect(generic.has(t.line), `${reason} falls back to the generic line`).toBe(false)
      }
    }
    // The person's own off says nothing; a hidden window says why.
    expect(localVideoReasonText('unpublished', 'camera')).toBeNull()
    expect(localVideoReasonText('user_stopped', 'screen')).toBeNull()
    expect(localVideoReasonText('hidden', 'screen')?.line).toMatch(/minimised or hidden/)
  })

  it('turns the standard base64 thumbnail into a data URL, and takes only display or window', () => {
    const s = parseScreenSources(contract.screenSources)
    expect(s[0]).toMatchObject({ kind: 'display', thumbnail: 'data:image/png;base64,iVBORw0KGgo=' })
    expect(s[1]).toMatchObject({ kind: 'window', app: 'Acme Docs', thumbnail: null })
    expect(() => parseScreenSources([{ ...contract.screenSources[0], kind: 'tab' }])).toThrow(/screen source/)
  })

  it('reads the sink answer: a u32 id, and no codec in decoded mode', () => {
    expect(parseSinkInfo(contract.videoSinkResultDecoded)).toEqual({ sinkId: 4, codec: null, mode: 'decoded' })
    expect(parseSinkInfo(contract.videoSinkResult)).toEqual({ sinkId: 3, codec: 'vp8', mode: 'encoded' })
    // A self-view sink opened before the camera runs: encoded, codec not known yet.
    expect(parseSinkInfo(contract.videoSinkResultSelfViewEarly)).toEqual({ sinkId: 5, codec: null, mode: 'encoded' })
    expect(() => parseSinkInfo({ sink_id: 1, codec: 7, mode: 'encoded' })).toThrow(/video_sink answer/)
    expect(() => parseSinkInfo({ sink_id: 1, codec: 'vp8', mode: 'decoded' })).toThrow(/video_sink answer/)
    expect(() => parseSinkInfo({ sink_id: 2 ** 32, codec: 'vp8', mode: 'encoded' })).toThrow(/video_sink answer/)
    expect(() => parseSinkInfo({ sink_id: '3', codec: 'vp8', mode: 'encoded' })).toThrow(/video_sink answer/)
  })

  it("reads video_enabled, camera_allowed and screen_allowed from sew-proto's MediaPolicy", () => {
    expect(parseMediaPolicy(contract.policy.joinAnswerNoVideoCap)).toEqual({ videoEnabled: true, cameraAllowed: false, screenAllowed: true })
    expect(parseMediaPolicy(contract.policy.videoOff)).toEqual({ videoEnabled: false, cameraAllowed: true, screenAllowed: true })
    // Older payloads without the two leases read as allowed.
    expect(parseMediaPolicy(contract.policy.old)).toEqual({ videoEnabled: true, cameraAllowed: true, screenAllowed: true })
    expect(() => parseMediaPolicy({ tier: 'full' })).toThrow(/policy/)
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
  it('sends the plugin command names with snake_case arguments', async () => {
    const { b, calls, channels } = bridge({
      video_sink: contract.videoSinkResult,
      video_sink_update: null,
      video_sink_close: null,
      request_keyframe: null,
      camera: null,
      screen: null,
      set_video_device: null,
      preview: contract.previewResult,
      camera_permission: 'prompt',
      request_camera_permission: 'granted',
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
    expect(await e.cameraPermission()).toBe('prompt')
    expect(await e.requestCameraPermission()).toBe('granted')
    expect(calls.slice(2)).toEqual([
      [`${P}request_keyframe`, { sink_id: 3 }],
      [`${P}video_sink_close`, { sink_id: 3 }],
      [`${P}camera`, { on: true, device: 'cam:2' }],
      [`${P}screen`, { on: true, source: 'display:1' }],
      [`${P}set_video_device`, { camera: 'default' }],
      [`${P}preview`, { on: true }],
      [`${P}camera_permission`, undefined],
      [`${P}request_camera_permission`, undefined],
    ])
    // Every command the adapter sends is one of the plugin's video commands.
    const sent = new Set(calls.map(([c]) => c.slice(P.length)))
    for (const c of sent) expect(contract.commands, c).toContain(c)
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
    expect(snap.policy).toEqual({ videoEnabled: true, cameraAllowed: false, screenAllowed: true })
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
      onPolicy: (p) => seen.push(`policy ${p.videoEnabled} ${p.cameraAllowed}`),
      onError: (code) => seen.push(`error ${code}`),
    })
    expect([...listeners.keys()].sort()).toEqual(contract.events.map((n: string) => `sew-voice://${n}`).sort())
    listeners.get('sew-voice://video')?.(contract.video)
    listeners.get('sew-voice://local_video')?.(contract.localVideo.paused)
    listeners.get('sew-voice://video_devices')?.(contract.videoDevices)
    listeners.get('sew-voice://policy')?.(contract.policy.videoOff)
    listeners.get('sew-voice://errors')?.(contract.errors.mic_denied)
    listeners.get('sew-voice://errors')?.(contract.errors.camera_lost)
    expect(seen).toEqual(['video 3', 'local paused', 'devices 2', 'policy false true', 'error camera_lost'])
    e.stop()
    expect(listeners.size).toBe(0)
  })
})
