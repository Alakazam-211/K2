// One viewer of the workspace's device stream (prd-mobile-dev-v1 §7).
//
//  - Connects only while the tab is shown and the device is up; a hidden
//    tab closes its socket, so it stops counting as a viewer (the daemon's
//    idle shutdown counts viewers).
//  - Offers H.264 and a window of 3 frames in flight (0.46.7); the daemon
//    answers in `hello` (an older daemon: JPEG, one at a time).
//  - JPEG: draws each frame on the canvas, then acks its seq. A frame with a
//    newer one already waiting is acked without drawing (stale).
//  - H.264: `H264Sink` decodes every frame in order (WebCodecs), draws each
//    output, acks it; it asks for a keyframe when it lost its place. If
//    video fails here the tab reconnects JPEG-only; the canvas keeps the
//    last frame meanwhile.
//  - A remote H.264 stream left at the JPEG default rate (8) is raised to
//    24 fps over Connect and to 60 on the LAN (`hello.path`); a rate the
//    person picked is kept.
//  - The screen size in points comes from each frame's header (it changes
//    when the device rotates); `hello` / `hold` / `state` / `fps` text
//    messages keep the rest current.
//  - Fix 4: decode and draw run in a worker on an OffscreenCanvas where the
//    webview allows it (`sim-player.ts`): the socket stays here and each
//    frame's buffer is transferred. Until the worker answers its probe the
//    socket waits; a worker with no VideoDecoder means JPEG only; a worker
//    that fails falls back to this thread on the same socket, and the pane
//    re-keys its <canvas> (`canvasKey`) since a canvas hands over control
//    once. `localStorage['k2.mobile.streamPlayer'] = 'main'` forces this
//    thread. jsdom has no OffscreenCanvas, so tests take this thread.
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
import {
  LAN_VIDEO_DEFAULT_FPS,
  REMOTE_DEFAULT_FPS,
  REMOTE_VIDEO_DEFAULT_FPS,
  STREAM_WINDOW,
  type SimCodec,
  type SimConnState,
  type SimFrame,
  type SimInput,
  type SimInputCaps,
  type SimStreamConnection,
  type SimStreamConnector,
  type SimViewerPath,
} from './sim-stream'
import { browserWebCodecs, H264Sink, type VideoFrameLike } from './sim-video'
import type { FrameStage, SimStreamStats } from './sim-latency'
import { blobDataUrl, canUseWorkerPlayer, PROBE_TIMEOUT_MS, SimPlayer, startPlayerWorker, type PlayerHandlers, type WorkerLike } from './sim-player'
import type { PixelRgb } from './sim-player.worker'

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
  player: 'pending' | 'worker' | 'main'
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

/** A small JPEG of what the canvas shows, or null when it can't export
 *  (no frame, jsdom). */
export function canvasThumb(canvas: HTMLCanvasElement | null): string | null {
  if (!canvas || canvas.width === 0 || canvas.height === 0) return null
  try {
    const url = canvas.toDataURL('image/jpeg', 0.6)
    return typeof url === 'string' && url.startsWith('data:image/') ? url : null
  } catch {
    return null
  }
}

/** The canvas's centre pixel, or null without a 2d context (jsdom). */
function readCentre(canvas: HTMLCanvasElement | null): PixelRgb | null {
  if (!canvas || canvas.width === 0 || canvas.height === 0) return null
  const ctx = canvas.getContext('2d', { willReadFrequently: true })
  if (!ctx) return null
  const d = ctx.getImageData(canvas.width >> 1, canvas.height >> 1, 1, 1).data
  return [d[0], d[1], d[2]]
}

/** Painted size for a stub frame: 2× points so text stays crisp. */
const PAINT_SCALE = 2

async function drawFrame(canvas: HTMLCanvasElement, frame: SimFrame, onDecoded?: () => void): Promise<boolean> {
  const ctx = canvas.getContext('2d')
  if (!ctx) return false
  if (frame.source.kind === 'paint') {
    const w = frame.widthPt * PAINT_SCALE
    const h = frame.heightPt * PAINT_SCALE
    if (canvas.width !== w) canvas.width = w
    if (canvas.height !== h) canvas.height = h
    frame.source.paint(ctx, w, h)
    return true
  }
  if (frame.source.kind !== 'jpeg') return false
  const bmp = await createImageBitmap(frame.source.data)
  onDecoded?.()
  try {
    if (canvas.width !== bmp.width) canvas.width = bmp.width
    if (canvas.height !== bmp.height) canvas.height = bmp.height
    ctx.drawImage(bmp, 0, 0)
  } finally {
    bmp.close()
  }
  return true
}

/** Draw a decoded video frame, cropped to the header's size (the coded
 *  picture may be one pixel wider / taller: H.264 pads odd sizes). */
function drawVideoFrame(canvas: HTMLCanvasElement, frame: VideoFrameLike, widthPx: number, heightPx: number): boolean {
  const ctx = canvas.getContext('2d')
  if (!ctx) return false
  const w = Math.min(widthPx, frame.displayWidth)
  const h = Math.min(heightPx, frame.displayHeight)
  if (canvas.width !== w) canvas.width = w
  if (canvas.height !== h) canvas.height = h
  ctx.drawImage(frame as unknown as CanvasImageSource, 0, 0, w, h, 0, 0, w, h)
  return true
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
  const warnedRef = useRef(false)
  // The first socket's fps; later changes go over the open socket.
  const fpsRef = useRef(fps)
  // The person picked a rate (keep it; don't raise it for video).
  const pickedRef = useRef(false)

  // ── The player (fix 4) ──────────────────────────────────────────────
  const [player, setPlayer] = useState<'pending' | 'worker' | 'main'>(() => (canUseWorkerPlayer() ? 'pending' : 'main'))
  const [canvasKey, setCanvasKey] = useState(0)
  const playerRef = useRef<SimPlayer | null>(null)
  // The worker can decode H.264 (its probe); the socket offers video only then.
  const playerVideoRef = useRef(true)
  const sampleRef = useRef(samplePixels)
  sampleRef.current = samplePixels
  const startWorkerRef = useRef(startWorker ?? startPlayerWorker)
  const lostWarnedRef = useRef(false)
  const wantPlayer = player === 'pending' && !!workspace && visible
  useEffect(() => {
    if (!wantPlayer || playerRef.current) return
    const toMain = (why: string): void => {
      if (!lostWarnedRef.current) {
        lostWarnedRef.current = true
        console.warn('[mobile] drawing on the main thread; the worker player is off here:', why)
      }
      playerRef.current?.close()
      playerRef.current = null
      setPlayer('main')
      // The old canvas may have handed its control to the worker.
      setCanvasKey((k) => k + 1)
    }
    const canvas = canvasRef.current
    if (!canvas) {
      toMain('no canvas yet')
      return
    }
    const handlers: PlayerHandlers = {
      drawn: (seq, decodedAt, drawnAt, px) => {
        setHasFrame(true)
        stageRef.current?.(seq, 'decoded', decodedAt)
        stageRef.current?.(seq, 'drawn', drawnAt)
        drawnRef.current?.(seq, px)
      },
      ack: (seq) => connRef.current?.ack(seq),
      keyframe: () => connRef.current?.requestKeyframe(),
      videoFailed: (why) => {
        console.warn('[mobile] video failed in the player worker; reconnecting with JPEG:', why)
        setJpegOnly(true)
      },
      lost: (why) => toMain(why),
    }
    let p: SimPlayer
    try {
      const off = canvas.transferControlToOffscreen()
      p = new SimPlayer(startWorkerRef.current(), off, handlers, { sample: sampleRef.current })
    } catch (err) {
      toMain(err instanceof Error ? err.message : String(err))
      return
    }
    playerRef.current = p
    let settled = false
    const timer = setTimeout(() => {
      if (!settled && playerRef.current === p) toMain('the worker did not answer its probe')
    }, PROBE_TIMEOUT_MS)
    void p.ready.then((probe) => {
      settled = true
      clearTimeout(timer)
      if (playerRef.current !== p) return
      if (!probe.canvas2d) {
        toMain('no OffscreenCanvas 2D context in the worker')
        return
      }
      playerVideoRef.current = probe.video
      setPlayer('worker')
    })
  }, [wantPlayer, canvasRef])
  useEffect(() => {
    playerRef.current?.hold(hold)
  }, [hold, player])
  useEffect(() => {
    playerRef.current?.sample(samplePixels)
  }, [samplePixels, player])
  useEffect(
    () => () => {
      playerRef.current?.close()
      playerRef.current = null
    },
    [],
  )
  const settled = player !== 'pending'

  useEffect(() => {
    if (!workspace || !visible) {
      setConn('idle')
      setHasFrame(false)
      return
    }
    // The worker player answers its probe first (it says what to offer).
    if (!settled) return
    playerRef.current?.reset()
    let live = true
    let c: SimStreamConnection | null = null
    // JPEG frames draw strictly in order; one newer than the frame being
    // drawn makes it stale.
    let drawing: Promise<void> = Promise.resolve()
    let newestJpeg = -1
    let sink: H264Sink | null = null
    const dropSink = (ackQueued = false): void => {
      sink?.close(ackQueued)
      sink = null
    }
    const videoSink = (): H264Sink | null => {
      if (sink) return sink
      const codecs = browserWebCodecs()
      if (!codecs) {
        console.warn('[mobile] H.264 frames but no WebCodecs here; reconnecting with JPEG')
        setJpegOnly(true)
        return null
      }
      sink = new H264Sink({
        codecs,
        held: () => holdRef.current,
        ack: (seq) => {
          if (live) c?.ack(seq)
        },
        needKeyframe: () => {
          if (live) c?.requestKeyframe()
        },
        failed: (why) => {
          console.warn('[mobile] video failed here; reconnecting with JPEG:', why)
          if (live) setJpegOnly(true)
        },
        draw: (frame, widthPx, heightPx) => {
          const canvas = canvasRef.current
          if (!live || !canvas) return
          // The decoder's timestamp is the frame's seq (H264Sink).
          stageRef.current?.(frame.timestamp, 'decoded', performance.now())
          if (drawVideoFrame(canvas, frame, widthPx, heightPx)) {
            setHasFrame(true)
            stageRef.current?.(frame.timestamp, 'drawn', performance.now())
            drawnRef.current?.(frame.timestamp, null)
          } else if (!warnedRef.current) {
            warnedRef.current = true
            console.warn('[mobile] the device canvas has no 2d context; frames are acked but not drawn')
          }
        },
      })
      return sink
    }
    c = connector({
      scope,
      workspace,
      fps: fpsRef.current,
      offer: { video: !jpegOnly && (playerRef.current ? playerVideoRef.current : true), window: STREAM_WINDOW },
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
              if (m.codec === 'jpeg') {
                dropSink(true)
                playerRef.current?.dropVideo()
              }
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
          // The worker player takes the frame's buffer (fix 4).
          const p = playerRef.current
          if (p) {
            if (!p.frame(frame)) c?.ack(frame.seq)
            return
          }
          if (frame.source.kind === 'h264') {
            const s = videoSink()
            if (!s) {
              c?.ack(frame.seq)
              return
            }
            const { key, data, widthPx, heightPx } = frame.source
            s.push({ seq: frame.seq, key, data, widthPx, heightPx })
            return
          }
          newestJpeg = frame.seq
          drawing = drawing.then(async () => {
            if (!live) return
            try {
              const canvas = canvasRef.current
              // A newer frame is already here: this one is stale.
              const stale = frame.seq !== newestJpeg
              if (canvas && !holdRef.current && !stale) {
                const drawn = await drawFrame(canvas, frame, () => stageRef.current?.(frame.seq, 'decoded', performance.now()))
                if (drawn) {
                  if (live) setHasFrame(true)
                  stageRef.current?.(frame.seq, 'drawn', performance.now())
                  drawnRef.current?.(frame.seq, null)
                } else if (!warnedRef.current) {
                  warnedRef.current = true
                  console.warn('[mobile] the device canvas has no 2d context; frames are acked but not drawn')
                }
              }
            } catch (err) {
              console.warn('[mobile] frame draw failed:', err)
            } finally {
              // Ack even a skipped or failed frame, or the stream stalls.
              if (live) c?.ack(frame.seq)
            }
          })
        },
      },
    })
    connRef.current = c
    const mine = c
    return () => {
      live = false
      dropSink()
      mine.close()
      if (connRef.current === mine) connRef.current = null
    }
  }, [scope, workspace, visible, connector, canvasRef, jpegOnly, stats, settled, showInputError])

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

  const thumb = useCallback(async (): Promise<string | null> => {
    const p = playerRef.current
    if (p) {
      const b = await p.thumb()
      return b ? blobDataUrl(b) : null
    }
    return canvasThumb(canvasRef.current)
  }, [canvasRef])

  const centrePixel = useCallback((): PixelRgb | null => {
    const p = playerRef.current
    return p ? p.lastPx : readCentre(canvasRef.current)
  }, [canvasRef])

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
