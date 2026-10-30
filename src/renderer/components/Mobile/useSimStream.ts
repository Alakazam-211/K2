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
//    24 fps; a rate the person picked is kept.
//  - The screen size in points comes from each frame's header (it changes
//    when the device rotates); `hello` / `hold` / `state` / `fps` text
//    messages keep the rest current.
//  - `hold` keeps the last drawn frame (M-A3: "freeze" is a client-side
//    hold; the device keeps running and frames are still acked, and video
//    keeps decoding so it can resume without a keyframe). The S5 pick
//    pop-up holds the frame this way.

import { useCallback, useEffect, useRef, useState } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import type { SimHold, SimState } from './simApi'
import type { ScreenPt } from './sim-input'
import {
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
} from './sim-stream'
import { browserWebCodecs, H264Sink, type VideoFrameLike } from './sim-video'

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
  /** Each frame once it is on the canvas (the latency readout). */
  onFrameDrawn?: (seq: number) => void
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
  send(input: SimInput): void
  setFps(fps: number): void
}

/** Painted size for a stub frame: 2× points so text stays crisp. */
const PAINT_SCALE = 2

async function drawFrame(canvas: HTMLCanvasElement, frame: SimFrame): Promise<boolean> {
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

export function useSimStream({ scope, workspace, visible, fps, connector, canvasRef, hold, onFrameDrawn }: UseSimStreamArgs): SimStreamView {
  const [conn, setConn] = useState<SimConnState | 'idle'>('idle')
  const [reason, setReason] = useState<string | null>(null)
  const [screen, setScreen] = useState<ScreenPt | null>(null)
  const [deviceHold, setDeviceHold] = useState<SimHold | null>(null)
  const [state, setState] = useState<SimState | null>(null)
  const [fpsGiven, setFpsGiven] = useState<number | null>(null)
  const [isolation, setIsolation] = useState<string | null>(null)
  const [inputError, setInputError] = useState<string | null>(null)
  const [hasFrame, setHasFrame] = useState(false)
  const [inputCaps, setInputCaps] = useState<SimInputCaps | null>(null)
  const drawnRef = useRef(onFrameDrawn)
  drawnRef.current = onFrameDrawn
  const [codec, setCodec] = useState<SimCodec | null>(null)
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

  useEffect(() => {
    if (!workspace || !visible) {
      setConn('idle')
      setHasFrame(false)
      return
    }
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
          if (drawVideoFrame(canvas, frame, widthPx, heightPx)) {
            setHasFrame(true)
            // The decoder's timestamp is the frame's seq (H264Sink).
            drawnRef.current?.(frame.timestamp)
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
      offer: { video: !jpegOnly, window: STREAM_WINDOW },
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
              if (m.codec === 'h264' && m.remote && !pickedRef.current && fpsRef.current === REMOTE_DEFAULT_FPS) {
                c?.setFps(REMOTE_VIDEO_DEFAULT_FPS)
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
              if (m.codec === 'jpeg') dropSink(true)
              return
            case 'error':
              console.warn('[mobile] input refused:', m.code, m.message)
              setInputError(m.message)
              return
          }
        },
        onFrame(frame) {
          if (!live) return
          setScreen((prev) =>
            prev && prev.widthPt === frame.widthPt && prev.heightPt === frame.heightPt
              ? prev
              : { widthPt: frame.widthPt, heightPt: frame.heightPt },
          )
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
                const drawn = await drawFrame(canvas, frame)
                if (drawn) {
                  if (live) setHasFrame(true)
                  drawnRef.current?.(frame.seq)
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
  }, [scope, workspace, visible, connector, canvasRef, jpegOnly])

  const send = useCallback((input: SimInput) => {
    const c = connRef.current
    if (!c) {
      console.warn('[mobile] input with no stream open:', input.t)
      return
    }
    setInputError(null)
    c.send(input)
  }, [])

  const setFps = useCallback((n: number) => {
    fpsRef.current = n
    pickedRef.current = true
    connRef.current?.setFps(n)
  }, [])

  return { conn, reason, screen, deviceHold, state, fps: fpsGiven, isolation, inputError, hasFrame, inputCaps, codec, send, setFps }
}
