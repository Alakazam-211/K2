// One viewer of the workspace's device stream (prd-mobile-dev-v1 §7).
//
//  - Connects only while the tab is shown and a device is running; a hidden
//    tab closes its socket, so it stops counting as a viewer (the daemon's
//    idle shutdown counts viewers).
//  - Draws each frame on the canvas, then acks it: one frame in flight, so a
//    slow viewer skips frames instead of backing up the others.
//  - `hold` keeps the last drawn frame on screen (M-A3: "freeze" is a
//    client-side hold; the device keeps running and frames are still acked).
//    The S5 pick pop-up holds the frame this way.

import { useCallback, useEffect, useRef, useState } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import type { SimControl } from './simApi'
import type {
  SimFrame,
  SimInput,
  SimStreamConnection,
  SimStreamConnector,
  SimStreamMeta,
  SimStreamState,
} from './sim-stream'

export interface UseSimStreamArgs {
  scope: ServerScope
  /** The running sim session; null = no device (no socket). */
  simId: string | null
  /** The tab is shown in this window. */
  visible: boolean
  fps: number
  connector: SimStreamConnector
  canvasRef: React.RefObject<HTMLCanvasElement | null>
  /** Keep the last frame (frames are still acked). */
  hold: boolean
}

export interface SimStreamView {
  state: SimStreamState | 'idle'
  reason: string | null
  meta: SimStreamMeta | null
  control: SimControl | null
  /** A frame has been drawn since the socket opened. */
  hasFrame: boolean
  send(input: SimInput): void
}

/** Painted size for a stub frame: 2× dp so text stays crisp. */
const PAINT_SCALE = 2

async function drawFrame(canvas: HTMLCanvasElement, frame: SimFrame, meta: SimStreamMeta | null): Promise<boolean> {
  const ctx = canvas.getContext('2d')
  if (!ctx) return false
  if (frame.source.kind === 'paint') {
    const w = (meta?.width ?? canvas.width) * PAINT_SCALE
    const h = (meta?.height ?? canvas.height) * PAINT_SCALE
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

export function useSimStream({ scope, simId, visible, fps, connector, canvasRef, hold }: UseSimStreamArgs): SimStreamView {
  const [state, setState] = useState<SimStreamState | 'idle'>('idle')
  const [reason, setReason] = useState<string | null>(null)
  const [meta, setMeta] = useState<SimStreamMeta | null>(null)
  const [control, setControl] = useState<SimControl | null>(null)
  const [hasFrame, setHasFrame] = useState(false)
  const connRef = useRef<SimStreamConnection | null>(null)
  const metaRef = useRef<SimStreamMeta | null>(null)
  const holdRef = useRef(hold)
  holdRef.current = hold
  const warnedRef = useRef(false)

  useEffect(() => {
    if (!simId || !visible) {
      setState('idle')
      setHasFrame(false)
      return
    }
    let live = true
    // Frames arrive one at a time (ack-paced); draw strictly in order.
    let drawing: Promise<void> = Promise.resolve()
    const conn = connector({
      scope,
      simId,
      fps,
      handlers: {
        onState(s, why) {
          if (!live) return
          setState(s)
          setReason(why ?? null)
          if (s !== 'open') setHasFrame(false)
        },
        onMeta(m) {
          if (!live) return
          metaRef.current = m
          setMeta(m)
        },
        onControl(c) {
          if (live) setControl(c)
        },
        onFrame(frame) {
          drawing = drawing.then(async () => {
            if (!live) return
            try {
              const canvas = canvasRef.current
              if (canvas && !holdRef.current) {
                const drawn = await drawFrame(canvas, frame, metaRef.current)
                if (drawn) {
                  if (live) setHasFrame(true)
                } else if (!warnedRef.current) {
                  warnedRef.current = true
                  console.warn('[mobile] the device canvas has no 2d context; frames are acked but not drawn')
                }
              }
            } catch (err) {
              console.warn('[mobile] frame draw failed:', err)
            } finally {
              // Ack even a skipped or failed frame, or the stream stalls.
              if (live) conn.ack(frame.seq)
            }
          })
        },
      },
    })
    connRef.current = conn
    return () => {
      live = false
      conn.close()
      if (connRef.current === conn) connRef.current = null
    }
  }, [scope, simId, visible, fps, connector, canvasRef])

  const send = useCallback((input: SimInput) => {
    const conn = connRef.current
    if (!conn) {
      console.warn('[mobile] input with no stream open:', input.action)
      return
    }
    conn.send(input)
  }, [])

  return { state, reason, meta, control, hasFrame, send }
}
