// One viewer of the workspace's device stream (prd-mobile-dev-v1 §7).
//
//  - Connects only while the tab is shown and the device is up; a hidden
//    tab closes its socket, so it stops counting as a viewer (the daemon's
//    idle shutdown counts viewers).
//  - Draws each frame on the canvas, then acks its seq: one frame in
//    flight, so a slow viewer skips frames instead of backing up others.
//  - The screen size in points comes from each frame's header (it changes
//    when the device rotates); `hello` / `hold` / `state` / `fps` text
//    messages keep the rest current.
//  - `hold` keeps the last drawn frame (M-A3: "freeze" is a client-side
//    hold; the device keeps running and frames are still acked). The S5
//    pick pop-up holds the frame this way.

import { useCallback, useEffect, useRef, useState } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import type { SimHold, SimState } from './simApi'
import type { ScreenPt } from './sim-input'
import type { SimConnState, SimFrame, SimInput, SimInputCaps, SimStreamConnection, SimStreamConnector } from './sim-stream'

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
  const connRef = useRef<SimStreamConnection | null>(null)
  const holdRef = useRef(hold)
  holdRef.current = hold
  const warnedRef = useRef(false)
  // The first socket's fps; later changes go over the open socket.
  const fpsRef = useRef(fps)

  useEffect(() => {
    if (!workspace || !visible) {
      setConn('idle')
      setHasFrame(false)
      return
    }
    let live = true
    // Frames arrive one at a time (ack-paced); draw strictly in order.
    let drawing: Promise<void> = Promise.resolve()
    const c = connector({
      scope,
      workspace,
      fps: fpsRef.current,
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
            case 'error':
              console.warn('[mobile] input refused:', m.code, m.message)
              setInputError(m.message)
              return
          }
        },
        onFrame(frame) {
          drawing = drawing.then(async () => {
            if (!live) return
            try {
              const canvas = canvasRef.current
              setScreen((prev) =>
                prev && prev.widthPt === frame.widthPt && prev.heightPt === frame.heightPt
                  ? prev
                  : { widthPt: frame.widthPt, heightPt: frame.heightPt },
              )
              if (canvas && !holdRef.current) {
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
              if (live) c.ack(frame.seq)
            }
          })
        },
      },
    })
    connRef.current = c
    return () => {
      live = false
      c.close()
      if (connRef.current === c) connRef.current = null
    }
  }, [scope, workspace, visible, connector, canvasRef])

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
    connRef.current?.setFps(n)
  }, [])

  return { conn, reason, screen, deviceHold, state, fps: fpsGiven, isolation, inputError, hasFrame, inputCaps, send, setFps }
}
