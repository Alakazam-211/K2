// One viewer of the workspace's device stream (prd-mobile-dev-v1 §7).
//
//  - Connects only while the tab is shown and the device is up; a hidden
//    tab closes its socket, so it stops counting as a viewer (the daemon's
//    idle shutdown counts viewers).
//  - Offers H.264 and a window of 3 frames in flight (0.46.7); the daemon
//    answers in `hello` (an older daemon: JPEG, one at a time).
//  - Frames go to the engine's player (`Stream/stream-playback.ts`): a
//    worker, or this thread (`Stream/stream-main-player.ts`: JPEG drawn
//    newest-only, H.264 through `VideoSink`, every seq acked). If video
//    fails the tab reconnects JPEG-only; the canvas keeps the last frame
//    meanwhile.
//  - A remote H.264 stream left at the JPEG default rate (8) is raised to
//    24 fps over Connect and to 60 on the LAN (`hello.path`); a rate the
//    person picked is kept.
//  - The screen size in points comes from each frame's header (it changes
//    when the device rotates); `hello` / `hold` / `state` / `fps` text
//    messages keep the rest current.
//  - Fix 4: decode and draw run in a worker on an OffscreenCanvas where the
//    webview allows it (the engine's `StreamPlayback` picks): the socket
//    stays here and each frame's buffer is transferred. Until the worker
//    answers its probe the socket waits; a worker with no H.264 decoder
//    means JPEG only; a worker that fails falls back to this thread on the
//    same socket, and the pane re-keys its <canvas> (`canvasKey`) since a
//    canvas hands over control once. `localStorage['k2.stream.player'] =
//    'main'` (or the older `k2.mobile.streamPlayer`) forces this thread.
//    jsdom has no OffscreenCanvas, so tests take this thread.
//  - M0 (the latency readout only): `stats` asks the daemon for its stage
//    times (`onStats`), and `onFrameStage` gets each frame's socket,
//    decoded and drawn times on this side.
//  - `hold` keeps the last drawn frame (M-A3: "freeze" is a client-side
//    hold; the device keeps running and frames are still acked, and video
//    keeps decoding so it can resume without a keyframe). The S5 pick
//    pop-up holds the frame this way.

import { useCallback, useEffect, useRef, useState } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import type { SimHold, SimState } from './simApi'
import type { ScreenPt } from './sim-input'
import { STREAM_WINDOW } from '@/components/Stream/stream-wire'
import type { WorkerLike } from '@/components/Stream/stream-player'
import type { PixelRgb } from '@/components/Stream/stream-player.worker'
import { StreamPlayback, type PlayerKind } from '@/components/Stream/stream-playback'
import {
  LAN_VIDEO_DEFAULT_FPS,
  REMOTE_DEFAULT_FPS,
  REMOTE_VIDEO_DEFAULT_FPS,
  type SimCodec,
  type SimConnState,
  type SimInput,
  type SimInputCaps,
  type SimStreamConnection,
  type SimStreamConnector,
  type SimViewerPath,
} from './sim-stream'
import type { FrameStage, SimStreamStats } from './sim-latency'

export interface UseSimStreamArgs {
  scope: ServerScope
  /** The workspace whose device to watch; null = no socket. */
  workspace: string | null
  /** The tab is shown in this window. */
  visible: boolean
  fps: number
  connector: SimStreamConnector
  canvasRef: React.RefObject<HTMLCanvasElement | null>
  /** Keep the last frame (frames are still acked). */
  hold: boolean
  /** Each frame once it is on the canvas (the latency readout), with its
   *  centre pixel from the worker player while `samplePixels`. */
  onFrameDrawn?: (seq: number, px: PixelRgb | null) => void
  /** M0's photon probe: the worker samples each drawn frame's centre. */
  samplePixels?: boolean
  /** The worker player's worker (tests inject a fake). */
  startWorker?: () => WorkerLike
  /** M0: ask the daemon for `stats` (reconnects when it changes). */
  stats?: boolean
  onStats?: (s: SimStreamStats) => void
  /** M0: a frame reached the socket, the decoder's output, the canvas. */
  onFrameStage?: (seq: number, stage: FrameStage, at: number) => void
}

export interface SimStreamView {
  conn: SimConnState | 'idle'
  reason: string | null
  /** The screen in points, from the latest frame. */
  screen: ScreenPt | null
  /** From `hello` / `hold`. */
  deviceHold: SimHold | null
  /** From `hello` / `state`. */
  state: SimState | null
  /** The cap the daemon gave this socket. */
  fps: number | null
  /** The isolation note from `hello` ("separate, not sealed"). */
  isolation: string | null
  /** The last refused input (`error`). */
  inputError: string | null
  /** A frame has been drawn since the socket opened. */
  hasFrame: boolean
  /** From `hello`: what input the device takes beyond one finger; null
   *  from an older daemon (no pinch, no fling tail). */
  inputCaps: SimInputCaps | null
  /** What the socket streams (from `hello` / `codec`); null before hello. */
  codec: SimCodec | null
  /** From `hello` (fix 2); null before hello or from an older daemon. */
  path: SimViewerPath | null
  /** Where frames are drawn (fix 4): a worker, this thread, or not yet
   *  known (the worker hasn't answered its probe). */
  player: PlayerKind
  /** The pane keys its <canvas> with this: it changes when the worker
   *  player gives up its (transferred) canvas. */
  canvasKey: number
  /** The canvas as a small JPEG data URL (the screenshot toast), or null. */
  thumb(): Promise<string | null>
  /** The centre pixel of what is on screen (M0's photon probe), or null. */
  centrePixel(): PixelRgb | null
  send(input: SimInput): void
  setFps(fps: number): void
}

export function useSimStream({
  scope,
  workspace,
  visible,
  fps,
  connector,
  canvasRef,
  hold,
  onFrameDrawn,
  stats = false,
  onStats,
  onFrameStage,
  samplePixels = false,
  startWorker,
}: UseSimStreamArgs): SimStreamView {
  const [conn, setConn] = useState<SimConnState | 'idle'>('idle')
  const [reason, setReason] = useState<string | null>(null)
  const [screen, setScreen] = useState<ScreenPt | null>(null)
  const [deviceHold, setDeviceHold] = useState<SimHold | null>(null)
  const [state, setState] = useState<SimState | null>(null)
  const [fpsGiven, setFpsGiven] = useState<number | null>(null)
  const [isolation, setIsolation] = useState<string | null>(null)
  const [inputError, setInputError] = useState<string | null>(null)
  // M-17: `send` clears the error only when one is shown (no React work
  // per input otherwise).
  const inputErrorRef = useRef<string | null>(null)
  const showInputError = useCallback((e: string | null) => {
    inputErrorRef.current = e
    setInputError(e)
  }, [])
  const [hasFrame, setHasFrame] = useState(false)
  const [inputCaps, setInputCaps] = useState<SimInputCaps | null>(null)
  const drawnRef = useRef(onFrameDrawn)
  drawnRef.current = onFrameDrawn
  const statsRef = useRef(onStats)
  statsRef.current = onStats
  const stageRef = useRef(onFrameStage)
  stageRef.current = onFrameStage
  const [codec, setCodec] = useState<SimCodec | null>(null)
  const [path, setPath] = useState<SimViewerPath | null>(null)
  // Video failed in this webview: reconnect offering JPEG only.
  const [jpegOnly, setJpegOnly] = useState(false)
  const connRef = useRef<SimStreamConnection | null>(null)
  const holdRef = useRef(hold)
  holdRef.current = hold
  // The first socket's fps; later changes go over the open socket.
  const fpsRef = useRef(fps)
  // The person picked a rate (keep it; don't raise it for video).
  const pickedRef = useRef(false)

  // ── The player (fix 4): the engine picks worker or this thread ─────
  const canvasRefRef = useRef(canvasRef)
  canvasRefRef.current = canvasRef
  const [playback] = useState(
    () =>
      new StreamPlayback(
        {
          drawn: (seq, decodedAt, drawnAt, px) => {
            setHasFrame(true)
            if (decodedAt !== null) stageRef.current?.(seq, 'decoded', decodedAt)
            stageRef.current?.(seq, 'drawn', drawnAt)
            drawnRef.current?.(seq, px)
          },
          ack: (seq) => connRef.current?.ack(seq),
          needKeyframe: () => connRef.current?.requestKeyframe(),
          videoFailed: () => setJpegOnly(true),
          player: (kind) => {
            setPlayer(kind)
            // The old canvas may have handed its control to the worker.
            if (kind === 'main') setCanvasKey((k) => k + 1)
          },
        },
        { canvas: () => canvasRefRef.current.current, held: () => holdRef.current, startWorker, tag: 'mobile' },
      ),
  )
  const [player, setPlayer] = useState<PlayerKind>(() => playback.kind)
  const [canvasKey, setCanvasKey] = useState(0)
  const sampleRef = useRef(samplePixels)
  sampleRef.current = samplePixels
  const wantPlayer = player === 'pending' && !!workspace && visible
  useEffect(() => {
    if (wantPlayer) playback.start({ sample: sampleRef.current })
  }, [wantPlayer, canvasRef, playback])
  useEffect(() => {
    playback.hold(hold)
  }, [hold, player, playback])
  useEffect(() => {
    playback.sample(samplePixels)
  }, [samplePixels, player, playback])
  useEffect(() => () => playback.close(), [playback])
  const settled = player !== 'pending'

  useEffect(() => {
    if (!workspace || !visible) {
      setConn('idle')
      setHasFrame(false)
      return
    }
    // The worker player answers its probe first (it says what to offer).
    if (!settled) return
    playback.reset()
    let live = true
    let c: SimStreamConnection | null = null
    // The sim daemon streams H.264 or JPEG (VP8 is the desktop's).
    const workerVideo = playback.workerDecode?.h264 ?? true
    c = connector({
      scope,
      workspace,
      fps: fpsRef.current,
      offer: { video: !jpegOnly && workerVideo, window: STREAM_WINDOW },
      ...(stats ? { stats: true } : {}),
      handlers: {
        onConn(s, why) {
          if (!live) return
          setConn(s)
          setReason(why ?? null)
          if (s !== 'open') setHasFrame(false)
        },
        onServer(m) {
          if (!live) return
          switch (m.t) {
            case 'hello':
              setState(m.state)
              setDeviceHold(m.hold)
              setFpsGiven(m.fps)
              setIsolation(m.isolation || null)
              setInputCaps(m.input ?? null)
              setCodec(m.codec)
              setPath(m.path ?? null)
              if (m.codec === 'h264' && m.remote && !pickedRef.current && fpsRef.current === REMOTE_DEFAULT_FPS) {
                c?.setFps(m.path === 'lan' ? LAN_VIDEO_DEFAULT_FPS : REMOTE_VIDEO_DEFAULT_FPS)
              }
              return
            case 'state':
              setState(m.state)
              if (m.message) setReason(m.message)
              return
            case 'hold':
              setDeviceHold(m.hold)
              return
            case 'fps':
              setFpsGiven(m.fps)
              return
            case 'codec':
              console.warn('[mobile] the stream switched to', m.codec, m.message ?? '')
              setCodec(m.codec)
              if (m.codec === 'jpeg') playback.dropVideo()
              return
            case 'error':
              console.warn('[mobile] input refused:', m.code, m.message)
              showInputError(m.message)
              return
            case 'stats':
              statsRef.current?.(m.stats)
              return
          }
        },
        onFrame(frame) {
          if (!live) return
          stageRef.current?.(frame.seq, 'recv', performance.now())
          setScreen((prev) =>
            prev && prev.widthPt === frame.widthPt && prev.heightPt === frame.heightPt
              ? prev
              : { widthPt: frame.widthPt, heightPt: frame.heightPt },
          )
          playback.frame(frame)
        },
      },
    })
    connRef.current = c
    const mine = c
    return () => {
      live = false
      playback.end()
      mine.close()
      if (connRef.current === mine) connRef.current = null
    }
  }, [scope, workspace, visible, connector, canvasRef, jpegOnly, stats, settled, showInputError, playback])

  const send = useCallback((input: SimInput) => {
    const c = connRef.current
    if (!c) {
      console.warn('[mobile] input with no stream open:', input.t)
      return
    }
    if (inputErrorRef.current !== null) showInputError(null)
    c.send(input)
  }, [showInputError])

  const setFps = useCallback((n: number) => {
    fpsRef.current = n
    pickedRef.current = true
    connRef.current?.setFps(n)
  }, [])

  const thumb = useCallback((): Promise<string | null> => playback.thumb(), [playback])

  const centrePixel = useCallback((): PixelRgb | null => playback.centrePixel(), [playback])

  return {
    conn,
    reason,
    screen,
    deviceHold,
    state,
    fps: fpsGiven,
    isolation,
    inputError,
    hasFrame,
    inputCaps,
    codec,
    path,
    player,
    canvasKey,
    thumb,
    centrePixel,
    send,
    setFps,
  }
}
