// The room's video controller over the fake engine and fake WebCodecs: the
// setting and the call gate it, events open and close sinks, frames reach the
// decoder, errors ask for keyframes, and refusals are explained.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { FakeVideoEngine, fakeTrack } from '@/lib/voice/fake-video-engine'
import { FakeVideoDecoder, fakeWebCodecs, RecordingSurface } from '@/lib/voice/fake-webcodecs'
import { LOCAL_CAMERA_TRACK } from '@/lib/voice/video-engine'
import { createVideoController, SINK_CLOSE_GRACE_MS, SINK_STATS_MS, SINK_UPDATE_DEBOUNCE_MS, type TileBox, type VideoDeps } from './video-core'

const key = { kind: 'encoded' as const, codec: 'vp8' as const, keyframe: true, width: 640, height: 360 }
const delta = { kind: 'encoded' as const, codec: 'vp8' as const }
const bytes = new Uint8Array([0x9d, 0x01, 0x2a])

/** The tile's box, as the observers would report it. */
function boxes() {
  const watched = new Map<HTMLElement, (b: TileBox) => void>()
  return {
    watched,
    watchBox: (el: HTMLElement, onBox: (b: TileBox) => void) => {
      watched.set(el, onBox)
      onBox({ width: 320, height: 180, visible: true })
      return () => watched.delete(el)
    },
  }
}

function setup(engine: FakeVideoEngine | null = new FakeVideoEngine(), over: Partial<VideoDeps> = {}) {
  const b = boxes()
  const surface = new RecordingSurface()
  const toasts: string[] = []
  const saved: unknown[] = []
  const ctl = createVideoController({
    loadEngine: async () => engine,
    webCodecs: fakeWebCodecs,
    watchBox: b.watchBox,
    devicePixelRatio: () => 2,
    surfaceFor: () => surface,
    loadPrefs: () => ({ cameraId: null }),
    savePrefs: (p) => saved.push(p),
    toast: (t) => toasts.push(t),
    openPrivacySettings: async () => true,
    now: () => Date.now(),
    ...over,
  })
  const canvas = (): HTMLCanvasElement => {
    const parent = { tagName: 'DIV' } as unknown as HTMLElement
    return { parentElement: parent } as unknown as HTMLCanvasElement
  }
  return { ctl, engine: engine as FakeVideoEngine, b, surface, toasts, saved, canvas }
}

/** Let the async steps (load, probe, open) finish. */
async function settle(): Promise<void> {
  for (let i = 0; i < 10; i++) await Promise.resolve()
  await vi.advanceTimersByTimeAsync(0)
}

beforeEach(() => {
  vi.useFakeTimers()
  FakeVideoDecoder.reset()
})
afterEach(() => vi.useRealTimers())

describe('the gate', () => {
  it('loads nothing while the setting is off', async () => {
    const s = setup()
    s.ctl.setActive({ enabled: false, inCall: true })
    await settle()
    expect(s.ctl.store.getState().availability).toBe('off')
    expect(s.engine.calls).toEqual([])
    expect(s.ctl.attach('TR_x', s.canvas(), { tile: 'grid' })).toBeTypeOf('function')
    expect(s.engine.sinks.size).toBe(0)
  })

  it('is ready with the setting on, and turning it off closes every sink', async () => {
    const s = setup()
    s.engine.tracks = [fakeTrack('7')]
    s.ctl.setActive({ enabled: true, inCall: true })
    await settle()
    expect(s.ctl.store.getState()).toMatchObject({ availability: 'ready', tracks: [{ track: 'TR_7' }] })
    s.ctl.attach('TR_7', s.canvas(), { tile: 'grid' })
    await settle()
    expect(s.engine.sinks.size).toBe(1)
    s.ctl.setActive({ enabled: false, inCall: true })
    await settle()
    expect(s.engine.sinks.size).toBe(0)
    expect(s.ctl.store.getState()).toMatchObject({ availability: 'off', tracks: [] })
  })

  it("says unavailable for a build without video (backend 'null'), and sends nothing else", async () => {
    // Sew's answer without video: everything off.
    const engine = new FakeVideoEngine({
      caps: {
        camera: false,
        screen: { supported: false, systemPicker: false, list: false, windows: false },
        publishCodecs: [],
        decodeCodecs: [],
        receive: false,
        selfView: false,
        liveSwitch: false,
        backend: 'null',
      },
    })
    const s = setup(engine)
    s.ctl.setActive({ enabled: true, inCall: true })
    await settle()
    expect(s.ctl.store.getState().availability).toBe('unavailable')
    await s.ctl.setCamera(true)
    expect(engine.calls).toEqual([])
  })

  it("says unavailable when video_caps can neither receive nor send (the plugin answers video_unsupported to the rest)", async () => {
    const engine = new FakeVideoEngine({ caps: { receive: false, publishCodecs: [] } })
    const s = setup(engine)
    s.ctl.setActive({ enabled: true, inCall: true })
    await settle()
    expect(s.ctl.store.getState().availability).toBe('unavailable')
    await s.ctl.setCamera(true)
    expect(engine.calls).toEqual([])
    // Receive alone is enough: other people's video shows.
    const viewer = setup(new FakeVideoEngine({ caps: { camera: false, screen: { supported: false, systemPicker: false, list: false, windows: false }, publishCodecs: [] } }))
    viewer.ctl.setActive({ enabled: true, inCall: true })
    await settle()
    expect(viewer.ctl.store.getState().availability).toBe('ready')
  })

  it('says unavailable when an older plugin has no video_caps', async () => {
    const engine = new FakeVideoEngine()
    engine.caps = async () => {
      throw new Error('command video_caps not found')
    }
    const s = setup(engine)
    s.ctl.setActive({ enabled: true, inCall: true })
    await settle()
    expect(s.ctl.store.getState().availability).toBe('unavailable')
    const none = setup(null)
    none.ctl.setActive({ enabled: true, inCall: false })
    await settle()
    expect(none.ctl.store.getState().availability).toBe('unavailable')
  })

  it('a call that ends takes its video with it', async () => {
    const s = setup()
    s.engine.tracks = [fakeTrack('7')]
    s.ctl.setActive({ enabled: true, inCall: true })
    await settle()
    s.ctl.attach('TR_7', s.canvas(), { tile: 'grid' })
    await settle()
    s.ctl.setActive({ enabled: true, inCall: false })
    expect(s.engine.sinks.size).toBe(0)
    expect(s.ctl.store.getState().tracks).toEqual([])
  })
})

describe('sinks', () => {
  async function live(over: Partial<VideoDeps> = {}) {
    const s = setup(new FakeVideoEngine(), over)
    s.engine.tracks = [fakeTrack('7'), fakeTrack('8')]
    s.ctl.setActive({ enabled: true, inCall: true })
    await settle()
    return s
  }

  it('opens a sink at the tile size in device pixels with the probed codecs, and its frames draw', async () => {
    const s = await live()
    s.ctl.attach('TR_7', s.canvas(), { tile: 'grid' })
    await settle()
    expect(s.engine.calls).toContain('video_sink {"track":"TR_7","width":640,"height":360,"visible":true,"tile":"grid","decode":["vp8","h264"]}')
    s.engine.push('TR_7', key, bytes)
    s.engine.push('TR_7', delta, bytes)
    expect(s.surface.drawn).toHaveLength(2)
    expect(FakeVideoDecoder.instances[0].configs).toEqual([{ codec: 'vp8', optimizeForLatency: true }])
  })

  it('a decoder error asks the plugin for a keyframe on that sink', async () => {
    const s = await live()
    s.ctl.attach('TR_7', s.canvas(), { tile: 'grid' })
    await settle()
    s.engine.push('TR_7', key, bytes)
    FakeVideoDecoder.instances[0].fail()
    await settle()
    const sinkId = s.engine.sinkFor('TR_7')?.info.sinkId
    expect(s.engine.calls).toContain(`request_keyframe {"sink_id":${sinkId}}`)
  })

  it('a bad frame is logged and the picture starts over at a keyframe', async () => {
    const s = await live()
    const err = vi.spyOn(console, 'error').mockImplementation(() => undefined)
    s.ctl.attach('TR_7', s.canvas(), { tile: 'grid' })
    await settle()
    s.engine.sinkFor('TR_7')?.onFrame(new Uint8Array(4).buffer)
    expect(err).toHaveBeenCalledWith('[video] bad frame:', expect.any(Error))
    expect(s.engine.sinkFor('TR_7')?.keyframes).toBe(1)
    err.mockRestore()
  })

  it('sends size and visibility changes, debounced', async () => {
    const s = await live()
    const c = s.canvas()
    s.ctl.attach('TR_7', c, { tile: 'grid' })
    await settle()
    const tell = s.b.watched.get(c.parentElement as HTMLElement)
    tell?.({ width: 400, height: 225, visible: true })
    tell?.({ width: 480, height: 270, visible: false })
    await vi.advanceTimersByTimeAsync(SINK_UPDATE_DEBOUNCE_MS - 1)
    expect(s.engine.calls.filter((c) => c.startsWith('video_sink_update'))).toEqual([])
    await vi.advanceTimersByTimeAsync(1)
    const id = s.engine.sinkFor('TR_7')?.info.sinkId
    expect(s.engine.calls.filter((c) => c.startsWith('video_sink_update'))).toEqual([
      `video_sink_update {"sink_id":${id},"width":960,"height":540,"visible":false,"tile":"grid"}`,
    ])
  })

  it('sends decode stats for the beacon', async () => {
    const s = await live()
    s.ctl.attach('TR_7', s.canvas(), { tile: 'grid' })
    await settle()
    s.engine.push('TR_7', key, bytes)
    await vi.advanceTimersByTimeAsync(SINK_STATS_MS)
    const last = s.engine.sinkFor('TR_7')?.updates.at(-1)
    expect(last?.stats).toEqual({ decoded: 1, dropped: 0, freezes: 0, freezeMs: 0 })
  })

  it('a detached tile closes its sink after the grace, and a quick re-attach keeps it', async () => {
    const s = await live()
    const detach = s.ctl.attach('TR_7', s.canvas(), { tile: 'grid' })
    await settle()
    const id = s.engine.sinkFor('TR_7')?.info.sinkId
    detach()
    await vi.advanceTimersByTimeAsync(SINK_CLOSE_GRACE_MS - 10)
    const again = s.ctl.attach('TR_7', s.canvas(), { tile: 'large' })
    await vi.advanceTimersByTimeAsync(SINK_CLOSE_GRACE_MS)
    expect(s.engine.sinkFor('TR_7')?.info.sinkId).toBe(id)
    expect(s.engine.calls.filter((c) => c.startsWith('video_sink {'))).toHaveLength(1)
    expect(s.engine.sinkFor('TR_7')?.updates.at(-1)).toMatchObject({ tile: 'large' })
    again()
    await vi.advanceTimersByTimeAsync(SINK_CLOSE_GRACE_MS)
    expect(s.engine.calls).toContain(`video_sink_close {"sink_id":${id}}`)
    expect(s.ctl.sinks()).toEqual([])
    expect(FakeVideoDecoder.instances.every((d) => d.state === 'closed' || d.chunks.length === 0)).toBe(true)
  })

  it('a track that leaves the list closes its sink at once', async () => {
    const s = await live()
    s.ctl.attach('TR_7', s.canvas(), { tile: 'grid' })
    s.ctl.attach('TR_8', s.canvas(), { tile: 'grid' })
    await settle()
    s.engine.setTracks([fakeTrack('8')])
    expect(s.ctl.sinks().map((x) => x.track)).toEqual(['TR_8'])
    expect(s.engine.sinkFor('TR_7')).toBeNull()
  })

  it('a muted track is held (not visible), and unmuted it asks for a keyframe', async () => {
    const s = await live()
    s.ctl.attach('TR_7', s.canvas(), { tile: 'grid' })
    await settle()
    s.engine.push('TR_7', key, bytes)
    s.engine.setTracks([fakeTrack('7', { muted: true }), fakeTrack('8')])
    await vi.advanceTimersByTimeAsync(SINK_UPDATE_DEBOUNCE_MS)
    expect(s.engine.sinkFor('TR_7')?.updates.at(-1)).toMatchObject({ visible: false })
    s.engine.setTracks([fakeTrack('7', { paused: 'background' }), fakeTrack('8')])
    expect(s.engine.sinkFor('TR_7')?.keyframes).toBe(0)
    s.engine.setTracks([fakeTrack('7'), fakeTrack('8')])
    await vi.advanceTimersByTimeAsync(SINK_UPDATE_DEBOUNCE_MS)
    expect(s.engine.sinkFor('TR_7')?.keyframes).toBe(1)
    expect(s.engine.sinkFor('TR_7')?.updates.at(-1)).toMatchObject({ visible: true })
  })

  it('a webview without WebCodecs asks for decoded frames', async () => {
    const s = await live({ webCodecs: () => null })
    s.ctl.attach('TR_7', s.canvas(), { tile: 'grid' })
    await settle()
    expect(s.engine.sinkFor('TR_7')?.req.decode).toEqual([])
    expect(s.engine.sinkFor('TR_7')?.info.mode).toBe('decoded')
    s.engine.push('TR_7', { kind: 'rgba', width: 2, height: 1 }, new Uint8Array(8))
    expect(s.surface.drawn).toEqual([{ kind: 'rgba', width: 2, height: 1 }])
  })
})

describe("the room's media policy", () => {
  it("keeps the join answer's camera_allowed and screen_allowed for the whole join; later copies change only video_enabled", async () => {
    // The join answer: this person's pass has no voice:video.
    const s = setup(new FakeVideoEngine({ policy: { videoEnabled: true, cameraAllowed: false, screenAllowed: false } }))
    s.ctl.setActive({ enabled: true, inCall: true })
    await settle()
    expect(s.ctl.store.getState().policy).toEqual({ videoEnabled: true, cameraAllowed: false, screenAllowed: false })
    // READY and media_policy_updated always say true (sew-proto): the join's no stands.
    s.engine.setPolicy({ videoEnabled: true, cameraAllowed: true, screenAllowed: true })
    expect(s.ctl.store.getState().policy).toEqual({ videoEnabled: true, cameraAllowed: false, screenAllowed: false })
    s.engine.setPolicy({ videoEnabled: false, cameraAllowed: true, screenAllowed: true })
    expect(s.ctl.store.getState().policy).toEqual({ videoEnabled: false, cameraAllowed: false, screenAllowed: false })
    await s.ctl.setCamera(true)
    await s.ctl.setScreen(true)
    expect(s.engine.calls.filter((c) => c.startsWith('camera') || c.startsWith('screen'))).toEqual([])
    // The call ends: the policy goes with it.
    s.ctl.setActive({ enabled: true, inCall: false })
    expect(s.ctl.store.getState().policy).toBeNull()
  })

  it("a new call in the same window reads its own join answer, and an update before it can't open the camera", async () => {
    const s = setup(new FakeVideoEngine({ policy: null }))
    s.ctl.setActive({ enabled: true, inCall: false })
    await settle()
    expect(s.ctl.store.getState().policy).toBeNull()
    // The next call's join answer says no to the camera.
    s.engine.joinPolicy = { videoEnabled: true, cameraAllowed: false, screenAllowed: true }
    // A copy arrives first (it says true), then the call starts.
    s.engine.setPolicy({ videoEnabled: true, cameraAllowed: true, screenAllowed: true })
    s.ctl.setActive({ enabled: true, inCall: true })
    await settle()
    expect(s.ctl.store.getState().policy).toEqual({ videoEnabled: true, cameraAllowed: false, screenAllowed: true })
    s.engine.setPolicy({ videoEnabled: true, cameraAllowed: true, screenAllowed: true })
    expect(s.ctl.store.getState().policy?.cameraAllowed).toBe(false)
  })

  it('does not ask the plugin for what the policy forbids, and says why', async () => {
    const s = setup(new FakeVideoEngine({ policy: { videoEnabled: true, cameraAllowed: false, screenAllowed: false } }))
    s.ctl.setActive({ enabled: true, inCall: true })
    await settle()
    await s.ctl.setCamera(true)
    expect(s.ctl.store.getState().notice?.line).toBe("Video isn't allowed for you in this room. A room admin can change that.")
    await s.ctl.setScreen(true)
    expect(s.ctl.store.getState().notice?.line).toBe("Screen sharing isn't allowed for you in this room. A room admin can change that.")
    expect(s.engine.calls.filter((c) => c.startsWith('camera') || c.startsWith('screen'))).toEqual([])
    s.engine.setPolicy({ videoEnabled: false, cameraAllowed: true, screenAllowed: true })
    await s.ctl.setCamera(true)
    expect(s.ctl.store.getState().notice?.line).toBe('Video is turned off for this room.')
  })
})

describe('camera and screen', () => {
  async function ready() {
    const s = setup()
    s.ctl.setActive({ enabled: true, inCall: true })
    await settle()
    return s
  }

  it('turns the camera on with the picked device, and the self-view sink closes when it goes off', async () => {
    const s = await ready()
    await s.ctl.setCameraDevice('cam:2')
    expect(s.saved).toEqual([{ cameraId: 'cam:2' }])
    expect(s.ctl.store.getState().cameraId).toBe('cam:2')
    await s.ctl.setCamera(true)
    expect(s.engine.calls).toContain('camera {"on":true,"device":"cam:2"}')
    expect(s.ctl.store.getState().local.camera).toBe('on')
    s.ctl.attach(LOCAL_CAMERA_TRACK, s.canvas(), { tile: 'grid' })
    await settle()
    expect(s.engine.sinkFor(LOCAL_CAMERA_TRACK)).not.toBeNull()
    await s.ctl.setCamera(false)
    expect(s.engine.sinkFor(LOCAL_CAMERA_TRACK)).toBeNull()
    await s.ctl.setCameraDevice(null)
    expect(s.engine.calls).toContain('set_video_device {"camera":"default"}')
  })

  it("explains the room's caps and the video permission in plain words", async () => {
    const s = await ready()
    s.engine.refuseNext('camera', 'video_slots_full')
    await s.ctl.setCamera(true)
    expect(s.ctl.store.getState().notice?.line).toBe('25 cameras are already on in this room, the most it allows. Try again when someone turns theirs off.')
    s.engine.refuseNext('screen', 'screen_slots_full')
    await s.ctl.setScreen(true, 'display:1')
    expect(s.ctl.store.getState().notice?.line).toMatch(/^3 people are already sharing their screens/)
    s.engine.refuseNext('camera', 'video_not_allowed')
    await s.ctl.setCamera(true)
    expect(s.ctl.store.getState().notice?.line).toBe("Video isn't allowed for you in this room. A room admin can change that.")
    s.engine.refuseNext('camera', 'media_busy', { retry_after: 5 })
    await s.ctl.setCamera(true)
    expect(s.ctl.store.getState().notice?.line).toBe('The call server is busy. Try again in 5 seconds.')
    expect(s.toasts).toHaveLength(4)
  })

  it('an OS refusal points to the privacy settings', async () => {
    const s = await ready()
    s.engine.refuseNext('camera', 'camera_denied')
    await s.ctl.setCamera(true)
    expect(s.ctl.store.getState().notice).toMatchObject({ settings: 'camera', kind: 'error' })
    s.engine.refuseNext('screen', 'screen_denied')
    await s.ctl.setScreen(true)
    expect(s.ctl.store.getState().notice).toMatchObject({ settings: 'screen' })
  })

  it('a cancelled screen picker says nothing', async () => {
    const s = await ready()
    s.engine.refuseNext('screen', 'picker_cancelled')
    await s.ctl.setScreen(true)
    expect(s.ctl.store.getState().notice).toBeNull()
    expect(s.toasts).toEqual([])
  })

  it('lists screens for the picker, and sharing one closes it', async () => {
    const s = await ready()
    await s.ctl.loadScreenSources()
    expect(s.ctl.store.getState().screenSources?.map((x) => x.id)).toEqual(['display:1', 'window:42'])
    await s.ctl.setScreen(true, 'window:42')
    expect(s.engine.calls).toContain('screen {"on":true,"source":"window:42"}')
    expect(s.ctl.store.getState().screenSources).toBeNull()
    expect(s.ctl.store.getState().local.screen).toBe('on')
  })

  it('reads the camera permission first, and asks the OS only from the button press, while it is undecided', async () => {
    const engine = new FakeVideoEngine({ cameraPermission: 'prompt', cameraPrompt: 'granted' })
    const s = setup(engine)
    s.ctl.setActive({ enabled: true, inCall: true })
    await settle()
    await s.ctl.loadDevices()
    await s.ctl.setCamera(true)
    expect(engine.calls).toEqual(['camera_permission', 'request_camera_permission', 'camera {"on":true}'])
    expect(s.ctl.store.getState().local.camera).toBe('on')
    // Granted now: the next press doesn't ask again.
    await s.ctl.setCamera(false)
    await s.ctl.setCamera(true)
    expect(engine.calls.filter((c) => c === 'request_camera_permission')).toHaveLength(1)
    expect(s.ctl.store.getState().notice).toBeNull()
  })

  it('a camera permission refused at the prompt, or before, sends no camera command and points to the settings', async () => {
    const engine = new FakeVideoEngine({ cameraPermission: 'prompt', cameraPrompt: 'denied' })
    const s = setup(engine)
    s.ctl.setActive({ enabled: true, inCall: true })
    await settle()
    await s.ctl.loadDevices()
    await s.ctl.setCamera(true)
    expect(engine.calls).toEqual(['camera_permission', 'request_camera_permission'])
    expect(s.ctl.store.getState()).toMatchObject({
      notice: { settings: 'camera', kind: 'error' },
      pending: { camera: false },
      local: { camera: 'off' },
    })
    expect(s.ctl.store.getState().devices?.permission.camera).toBe('denied')
    // Denied already: no prompt, the same words.
    await s.ctl.setCamera(true)
    expect(engine.calls.slice(2)).toEqual(['camera_permission'])
    expect(s.ctl.store.getState().notice?.line).toBe("K2 isn't allowed to use your camera. Allow K2 in your system's privacy settings.")
    // The plugin's own refusal, if the permission changed in between, reads the same.
    engine.permission = 'granted'
    s.engine.refuseNext('camera', 'camera_permission_denied')
    await s.ctl.setCamera(true)
    expect(s.ctl.store.getState().notice).toMatchObject({ settings: 'camera' })
  })

  it('turning the camera off says nothing (the plugin says unpublished)', async () => {
    const s = await ready()
    await s.ctl.setCamera(true)
    await s.ctl.setCamera(false)
    expect(s.ctl.store.getState().local).toEqual({ camera: 'off', screen: 'off', reason: 'unpublished' })
    expect(s.ctl.store.getState().notice).toBeNull()
    expect(s.toasts).toEqual([])
  })

  it('a minimised shared window pauses the share and says so', async () => {
    const s = await ready()
    await s.ctl.setScreen(true)
    s.engine.setLocal({ camera: 'off', screen: 'paused', reason: 'hidden' })
    expect(s.ctl.store.getState().notice?.line).toBe('Screen sharing is paused while the window you shared is minimised or hidden.')
  })

  it('switching cameras mid-call says to turn it off and on only where the plugin cannot switch live', async () => {
    const s = await ready()
    await s.ctl.setCamera(true)
    await s.ctl.setCameraDevice('cam:2')
    expect(s.ctl.store.getState().notice).toBeNull()
    const old = setup(new FakeVideoEngine({ caps: { liveSwitch: false } }))
    old.ctl.setActive({ enabled: true, inCall: true })
    await settle()
    await old.ctl.setCamera(true)
    await old.ctl.setCameraDevice('cam:2')
    expect(old.ctl.store.getState().notice?.line).toBe('Turn your camera off and on again to switch to that camera.')
  })

  it('says why the camera paused or stopped by itself', async () => {
    const s = await ready()
    await s.ctl.setCamera(true)
    s.engine.setLocal({ camera: 'paused', screen: 'off', reason: 'background' })
    expect(s.ctl.store.getState().notice?.line).toBe('Your camera is paused while K2 is in the background.')
    s.engine.setLocal({ camera: 'off', screen: 'off', reason: 'camera_lost' })
    expect(s.ctl.store.getState().notice).toMatchObject({ line: 'Your camera disconnected.', kind: 'error' })
    // Sew 0.8.3: open, but no frame came.
    await s.ctl.setCamera(true)
    s.engine.setLocal({ camera: 'off', screen: 'off', reason: 'no_frames' })
    expect(s.ctl.store.getState().notice).toMatchObject({ line: "Your camera isn't sending a picture. Is the lid closed?", kind: 'error' })
    s.engine.emitError('camera_lost')
    expect(s.ctl.store.getState().notice?.line).toBe('Your camera disconnected.')
    s.engine.emitError('encoder_lost')
    expect(s.ctl.store.getState().notice?.line).toMatch(/could not encode/)
  })
})
