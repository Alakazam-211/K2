// Video in K2's voice rooms: this window's view of the call's video, and the
// sinks that draw it. Every dependency is injected, so tests drive it with
// `FakeVideoEngine` and no webview. `stores/video.ts` builds the app's instance.
//
// Daemon-first check: the call and its media live in the app's Rust process
// (Sew's plugin); its events are the truth and reach every window. What lives
// here is per-window view state (which tiles this window draws, at what size)
// and nothing a headless daemon needs.
//
// Dark behind the `videoRoomsPreview` setting: off, nothing here loads.

import { createStore, type StoreApi } from 'zustand/vanilla'
import {
  LOCAL_CAMERA_TRACK,
  LOCAL_SCREEN_TRACK,
  LOCAL_VIDEO_OFF,
  type LocalVideo,
  type ScreenSource,
  type SinkInfo,
  type SinkTile,
  type VideoCaps,
  type VideoDevicesInfo,
  type VideoEngine,
  type VideoPolicy,
  type VideoTrack,
  NO_VIDEO_BACKEND,
} from '@/lib/voice/video-engine'
import { canvasSurface, probeDecode, SinkDecoder, type FrameSurface, type WebCodecsEnv } from '@/lib/voice/video-decoder'
import { NativeVoiceError } from '@/lib/voice/native-voice-engine'
import {
  localVideoReasonText,
  policyBlockText,
  videoEngineErrorText,
  videoRefusalText,
  type PrivacyPane,
  type VideoNoticeText,
} from '@/lib/voice/video-labels'
import type { VideoCodec } from '@/lib/voice/video-frame'

/** A detached tile's sink stays open this long, so a re-render doesn't flap it. */
export const SINK_CLOSE_GRACE_MS = 2_000
/** Size and visibility changes reach the plugin at most this often. */
export const SINK_UPDATE_DEBOUNCE_MS = 250
/** Each open sink's decode stats go to the plugin's beacon this often. */
export const SINK_STATS_MS = 10_000

export type VideoAvailability = 'off' | 'loading' | 'ready' | 'unavailable'

export interface VideoNotice extends VideoNoticeText {
  at: number
}

export interface VideoState {
  /** `unavailable`: this build's plugin has no video (backend `null`, or an
   *  older plugin with no `video_caps`), or the engine failed. */
  availability: VideoAvailability
  caps: VideoCaps | null
  tracks: VideoTrack[]
  local: LocalVideo
  devices: VideoDevicesInfo | null
  /** The last `screen_sources` answer, while the share picker is open. */
  screenSources: ScreenSource[] | null
  /** Sew's media policy for this call. `cameraAllowed` and `screenAllowed` keep
   *  the join answer's value for the whole call (sew-proto: later copies always
   *  say true); `videoEnabled` follows each update. Null: no call yet. */
  policy: VideoPolicy | null
  notice: VideoNotice | null
  /** A camera or screen command is on its way. */
  pending: { camera: boolean; screen: boolean }
  /** The camera this person picked (saved). Null: the system default. */
  cameraId: string | null
}

export interface VideoPrefs {
  /** Null: the system default camera. */
  cameraId: string | null
}

/** What a tile's box looks like right now. */
export interface TileBox {
  /** CSS pixels. */
  width: number
  height: number
  /** On screen (scrolled into view, window not hidden). */
  visible: boolean
}

export interface VideoDeps {
  /** The engine, when this build has one. Null: no video. */
  loadEngine: () => Promise<VideoEngine | null>
  /** This webview's WebCodecs, or null. */
  webCodecs: () => WebCodecsEnv | null
  /** Watch a tile's box; returns the unwatch. Calls back at once with the first box. */
  watchBox: (el: HTMLElement, onBox: (box: TileBox) => void) => () => void
  devicePixelRatio: () => number
  surfaceFor?: (canvas: HTMLCanvasElement) => FrameSurface
  loadPrefs: () => VideoPrefs
  savePrefs: (p: VideoPrefs) => void
  toast: (text: string, kind: 'error' | 'info') => void
  /** Opens the OS privacy pane. False: this OS has none. */
  openPrivacySettings?: (pane: PrivacyPane) => Promise<boolean>
  now: () => number
}

export interface AttachOptions {
  tile: SinkTile
  /** The track is muted or paused: the plugin needn't send it. */
  held?: boolean
}

export interface VideoController {
  store: StoreApi<VideoState>
  /** The setting and the call: load the engine while both say so. */
  setActive(active: { enabled: boolean; inCall: boolean }): void
  setCamera(on: boolean): Promise<void>
  /** No `source`: the OS's own picker. A `source` id from `screen_sources`
   *  shares that one (the list, where the OS allows it). */
  setScreen(on: boolean, source?: string): Promise<void>
  loadScreenSources(): Promise<void>
  closeScreenPicker(): void
  loadDevices(): Promise<void>
  /** Null: the system default. */
  setCameraDevice(id: string | null): Promise<void>
  /** Draw `track` on `canvas`; returns the detach. */
  attach(track: string, canvas: HTMLCanvasElement, opts: AttachOptions): () => void
  /** A tile's muted or paused state changed. */
  setHeld(track: string, held: boolean): void
  openPrivacySettings(pane: PrivacyPane): Promise<void>
  clearNotice(): void
  /** Open sinks, for tests and the demo. */
  sinks(): Array<{ track: string; info: SinkInfo | null; refs: number; stats: SinkDecoder['stats'] }>
  stop(): void
}

interface Sink {
  track: string
  tile: SinkTile
  canvas: HTMLCanvasElement | null
  surface: FrameSurface | null
  refs: number
  info: SinkInfo | null
  decoder: SinkDecoder
  box: TileBox
  held: boolean
  /** What the plugin was last told. */
  sent: { width: number; height: number; visible: boolean; tile: SinkTile } | null
  unwatch: (() => void) | null
  closeTimer: ReturnType<typeof setTimeout> | null
  updateTimer: ReturnType<typeof setTimeout> | null
  closed: boolean
}

const isLocal = (track: string): boolean => track === LOCAL_CAMERA_TRACK || track === LOCAL_SCREEN_TRACK

export function createVideoController(deps: VideoDeps): VideoController {
  let prefs = deps.loadPrefs()
  const store = createStore<VideoState>(() => ({
    availability: 'off',
    caps: null,
    tracks: [],
    local: LOCAL_VIDEO_OFF,
    devices: null,
    screenSources: null,
    policy: null,
    notice: null,
    pending: { camera: false, screen: false },
    cameraId: prefs.cameraId,
  }))
  let engine: VideoEngine | null = null
  let loading: Promise<VideoEngine | null> | null = null
  /** Bumped on every stop, so a load that finishes late does nothing. */
  let gen = 0
  let active = { enabled: false, inCall: false }
  let decodeList: Promise<VideoCodec[]> | null = null
  const sinks = new Map<string, Sink>()
  let statsTimer: ReturnType<typeof setInterval> | null = null

  const notice = (n: VideoNoticeText | null): void => {
    if (!n) return
    store.setState({ notice: { ...n, at: deps.now() } })
    deps.toast(n.line, n.kind)
  }

  // ── sinks ─────────────────────────────────────────────────────────

  const sendUpdate = (s: Sink, withStats = false): void => {
    if (s.closed || !s.info || !engine) return
    const dpr = deps.devicePixelRatio()
    const want = {
      width: Math.round(s.box.width * dpr),
      height: Math.round(s.box.height * dpr),
      visible: s.box.visible && !s.held,
      tile: s.tile,
    }
    const prev = s.sent
    const changed = !prev || prev.width !== want.width || prev.height !== want.height || prev.visible !== want.visible || prev.tile !== want.tile
    if (!changed && !withStats) return
    s.sent = want
    void engine
      .updateSink(s.info.sinkId, {
        ...(changed ? want : {}),
        ...(withStats ? { stats: { ...s.decoder.stats } } : {}),
      })
      .catch((err: unknown) => console.warn('[video] updating a sink failed:', err))
  }

  const scheduleUpdate = (s: Sink): void => {
    if (s.updateTimer) clearTimeout(s.updateTimer)
    s.updateTimer = setTimeout(() => {
      s.updateTimer = null
      sendUpdate(s)
    }, SINK_UPDATE_DEBOUNCE_MS)
  }

  const closeSink = (s: Sink): void => {
    if (s.closed) return
    s.closed = true
    sinks.delete(s.track)
    if (s.closeTimer) clearTimeout(s.closeTimer)
    if (s.updateTimer) clearTimeout(s.updateTimer)
    s.unwatch?.()
    s.unwatch = null
    s.decoder.close()
    const e = engine
    if (s.info && e) void e.closeSink(s.info.sinkId).catch((err: unknown) => console.warn('[video] closing a sink failed:', err))
    if (sinks.size === 0 && statsTimer) {
      clearInterval(statsTimer)
      statsTimer = null
    }
  }

  const closeAllSinks = (): void => {
    for (const s of [...sinks.values()]) closeSink(s)
  }

  const watch = (s: Sink, canvas: HTMLCanvasElement): void => {
    s.unwatch?.()
    s.canvas = canvas
    s.surface = (deps.surfaceFor ?? canvasSurface)(canvas)
    const box = canvas.parentElement ?? canvas
    s.unwatch = deps.watchBox(box, (b) => {
      s.box = b
      scheduleUpdate(s)
    })
  }

  const openSink = (s: Sink, e: VideoEngine): void => {
    decodeList ??= probeDecode(deps.webCodecs())
    void (async () => {
      const decode = await decodeList
      if (s.closed) return
      const dpr = deps.devicePixelRatio()
      const req = {
        track: s.track,
        width: Math.round(s.box.width * dpr),
        height: Math.round(s.box.height * dpr),
        visible: s.box.visible && !s.held,
        tile: s.tile,
        decode,
      }
      const info = await e.openSink(req, (data) => {
        try {
          s.decoder.push(data)
        } catch (err) {
          // A frame this client can't read: loud in the log, and the picture
          // starts over at the next keyframe.
          console.error('[video] bad frame:', err)
          s.decoder.resume()
        }
      })
      s.sent = { width: req.width, height: req.height, visible: req.visible, tile: req.tile }
      if (s.closed) {
        // Detached while it opened.
        void e.closeSink(info.sinkId).catch((err: unknown) => console.warn('[video] closing a sink failed:', err))
        return
      }
      s.info = info
      sendUpdate(s)
    })().catch((err: unknown) => {
      console.warn(`[video] opening a sink for ${s.track} failed:`, err)
      closeSink(s)
    })
  }

  const attach = (track: string, canvas: HTMLCanvasElement, opts: AttachOptions): (() => void) => {
    const e = engine
    if (!e) return () => undefined
    let s = sinks.get(track)
    if (s) {
      if (s.closeTimer) {
        clearTimeout(s.closeTimer)
        s.closeTimer = null
      }
      s.refs += 1
      if (s.canvas !== canvas) watch(s, canvas)
      if (s.tile !== opts.tile) {
        s.tile = opts.tile
        scheduleUpdate(s)
      }
    } else {
      const sink: Sink = {
        track,
        tile: opts.tile,
        canvas: null,
        surface: null,
        refs: 1,
        info: null,
        box: { width: 0, height: 0, visible: true },
        held: opts.held === true,
        sent: null,
        unwatch: null,
        closeTimer: null,
        updateTimer: null,
        closed: false,
        decoder: new SinkDecoder({
          env: deps.webCodecs(),
          surface: () => sink.surface,
          requestKeyframe: () => {
            const id = sink.info?.sinkId
            if (id === undefined || !engine) return
            void engine.requestKeyframe(id).catch((err: unknown) => console.warn('[video] keyframe request failed:', err))
          },
          now: deps.now,
        }),
      }
      s = sink
      sinks.set(track, sink)
      watch(sink, canvas)
      if (sink.updateTimer) {
        // The first box is part of the open, not an update.
        clearTimeout(sink.updateTimer)
        sink.updateTimer = null
      }
      openSink(sink, e)
      statsTimer ??= setInterval(() => {
        for (const x of sinks.values()) sendUpdate(x, true)
      }, SINK_STATS_MS)
    }
    const mine = s
    let detached = false
    return () => {
      if (detached || mine.closed) return
      detached = true
      mine.refs -= 1
      if (mine.refs > 0) return
      mine.closeTimer = setTimeout(() => closeSink(mine), SINK_CLOSE_GRACE_MS)
    }
  }

  const setHeld = (track: string, held: boolean): void => {
    const s = sinks.get(track)
    if (!s || s.held === held) return
    s.held = held
    if (held) s.decoder.hold()
    else s.decoder.resume()
    scheduleUpdate(s)
  }

  // ── events ────────────────────────────────────────────────────────

  const onVideo = (tracks: VideoTrack[]): void => {
    const live = new Set(tracks.map((t) => t.track))
    for (const s of [...sinks.values()]) {
      if (!isLocal(s.track) && !live.has(s.track)) closeSink(s)
    }
    for (const t of tracks) setHeld(t.track, t.muted || t.paused !== null)
    store.setState({ tracks })
  }

  /** The join answer's policy, from the plugin's `state` (a window opened
   *  mid-call, or a new call in this window). */
  const readJoinPolicy = async (): Promise<void> => {
    const e = engine
    if (!e) return
    const myGen = gen
    const snap = await e.snapshot()
    if (myGen !== gen || !snap.policy) return
    const now = store.getState().policy
    // A media_policy_updated that came in meanwhile has the newer video_enabled.
    store.setState({ policy: now ? { ...snap.policy, videoEnabled: now.videoEnabled } : snap.policy })
  }

  /** READY and media_policy_updated always say camera and screen are allowed
   *  (sew-proto): only the join answer can say no. So a later copy changes
   *  `videoEnabled` only, and the join's two leases hold for the whole join. */
  const onPolicy = (p: VideoPolicy): void => {
    const prev = store.getState().policy
    if (prev) {
      store.setState({ policy: { ...prev, videoEnabled: p.videoEnabled } })
      return
    }
    // No join answer held yet: this copy's video_enabled for now, and the
    // leases from the join answer in the plugin's state.
    store.setState({ policy: p })
    void readJoinPolicy().catch((err: unknown) => console.warn("[video] reading the join's media policy failed:", err))
  }

  const onLocalVideo = (local: LocalVideo): void => {
    const prev = store.getState().local
    store.setState({ local })
    if (local.camera === 'off') {
      const s = sinks.get(LOCAL_CAMERA_TRACK)
      if (s) closeSink(s)
    }
    if (local.screen === 'off') {
      const s = sinks.get(LOCAL_SCREEN_TRACK)
      if (s) closeSink(s)
    }
    setHeld(LOCAL_CAMERA_TRACK, local.camera === 'paused')
    setHeld(LOCAL_SCREEN_TRACK, local.screen === 'paused')
    // Stopped or paused by something other than this person: say why, once.
    const cameraChanged = local.camera !== prev.camera && (local.camera === 'off' || local.camera === 'paused')
    const screenChanged = local.screen !== prev.screen && (local.screen === 'off' || local.screen === 'paused')
    if (cameraChanged && prev.camera !== 'off') notice(localVideoReasonText(local.reason, 'camera'))
    else if (screenChanged && prev.screen !== 'off') notice(localVideoReasonText(local.reason, 'screen'))
  }

  // ── the engine ────────────────────────────────────────────────────

  const unload = (): void => {
    gen += 1
    closeAllSinks()
    engine?.stop()
    engine = null
    loading = null
    store.setState({
      availability: 'off',
      tracks: [],
      local: LOCAL_VIDEO_OFF,
      devices: null,
      screenSources: null,
      policy: null,
      pending: { camera: false, screen: false },
    })
  }

  const load = (): Promise<VideoEngine | null> => {
    if (loading) return loading
    const myGen = gen
    store.setState({ availability: 'loading' })
    loading = (async () => {
      const e = await deps.loadEngine()
      if (!e) {
        if (myGen === gen) store.setState({ availability: 'unavailable' })
        return null
      }
      let caps: VideoCaps
      try {
        caps = await e.caps()
      } catch (err) {
        // A plugin from before video has no `video_caps`.
        console.info('[video] this build has no video yet:', err)
        if (myGen === gen) store.setState({ availability: 'unavailable' })
        return null
      }
      if (myGen !== gen) return null
      if (caps.backend === NO_VIDEO_BACKEND) {
        // Built without video: it answers with everything off.
        store.setState({ availability: 'unavailable', caps })
        return null
      }
      await e.start({
        onVideo,
        onLocalVideo,
        onDevices: (d) => store.setState({ devices: d }),
        onPolicy,
        onError: (code) => notice(videoEngineErrorText(code)),
      })
      if (myGen !== gen) {
        e.stop()
        return null
      }
      engine = e
      const snap = await e.snapshot()
      if (myGen !== gen) return null
      store.setState({ availability: 'ready', caps, policy: snap.policy })
      onLocalVideo(snap.local)
      onVideo(snap.tracks)
      return e
    })().catch((err: unknown) => {
      console.error('[video] the video engine did not start:', err)
      if (myGen === gen) store.setState({ availability: 'unavailable' })
      loading = null
      return null
    })
    return loading
  }

  const setActive = (next: { enabled: boolean; inCall: boolean }): void => {
    const was = active
    active = next
    if (!next.enabled) {
      if (was.enabled || engine || loading) unload()
      return
    }
    if (!loading) void load()
    // A new call: its join answer's policy.
    if (!was.inCall && next.inCall && engine) {
      void readJoinPolicy().catch((err: unknown) => console.warn("[video] reading the join's media policy failed:", err))
    }
    // The call ended: its video went with it.
    if (was.inCall && !next.inCall) {
      closeAllSinks()
      store.setState({ tracks: [], local: LOCAL_VIDEO_OFF, screenSources: null, policy: null, pending: { camera: false, screen: false } })
    }
  }

  const refusal = (err: unknown, what: 'camera' | 'screen'): void => {
    const e = err instanceof NativeVoiceError ? err : new NativeVoiceError('failed', err instanceof Error ? err.message : String(err))
    if (e.code === 'failed') console.warn(`[video] ${what} failed:`, err)
    notice(videoRefusalText(e.code, e.detail, what))
  }

  const setCamera = async (on: boolean): Promise<void> => {
    const e = engine
    if (!e || store.getState().pending.camera) return
    const blocked = on ? policyBlockText(store.getState().policy, 'camera') : null
    if (blocked) {
      notice({ line: blocked, kind: 'error' })
      return
    }
    store.setState((s) => ({ pending: { ...s.pending, camera: true }, notice: null }))
    try {
      await e.camera(on, on && prefs.cameraId ? prefs.cameraId : undefined)
    } catch (err) {
      refusal(err, 'camera')
    } finally {
      store.setState((s) => ({ pending: { ...s.pending, camera: false } }))
    }
  }

  const setScreen = async (on: boolean, source?: string): Promise<void> => {
    const e = engine
    if (!e || store.getState().pending.screen) return
    const blocked = on ? policyBlockText(store.getState().policy, 'screen') : null
    if (blocked) {
      notice({ line: blocked, kind: 'error' })
      return
    }
    store.setState((s) => ({ pending: { ...s.pending, screen: true }, notice: null, screenSources: null }))
    try {
      await e.screen(on, source)
    } catch (err) {
      refusal(err, 'screen')
    } finally {
      store.setState((s) => ({ pending: { ...s.pending, screen: false } }))
    }
  }

  const loadScreenSources = async (): Promise<void> => {
    const e = engine
    if (!e) return
    try {
      store.setState({ screenSources: await e.screenSources() })
    } catch (err) {
      refusal(err, 'screen')
    }
  }

  const loadDevices = async (): Promise<void> => {
    const e = engine
    if (!e) return
    store.setState({ devices: await e.devices() })
  }

  const setCameraDevice = async (id: string | null): Promise<void> => {
    prefs = { ...prefs, cameraId: id }
    deps.savePrefs(prefs)
    store.setState({ cameraId: id })
    const e = engine
    if (!e) return
    try {
      await e.setDevice(id ?? 'default')
    } catch (err) {
      refusal(err, 'camera')
    }
  }

  const openPrivacySettings = async (pane: PrivacyPane): Promise<void> => {
    let opened = false
    try {
      opened = (await deps.openPrivacySettings?.(pane)) === true
    } catch (err) {
      console.warn('[video] opening the privacy settings failed:', err)
    }
    if (!opened) notice({ line: "K2 couldn't open your settings. Open your system's privacy settings and allow K2.", kind: 'error' })
  }

  return {
    store,
    setActive,
    setCamera,
    setScreen,
    loadScreenSources,
    closeScreenPicker: () => store.setState({ screenSources: null }),
    loadDevices,
    setCameraDevice,
    attach,
    setHeld,
    openPrivacySettings,
    clearNotice: () => store.setState({ notice: null }),
    sinks: () => [...sinks.values()].map((s) => ({ track: s.track, info: s.info, refs: s.refs, stats: s.decoder.stats })),
    stop: unload,
  }
}
