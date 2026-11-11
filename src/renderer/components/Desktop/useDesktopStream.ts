// One viewer of this machine's desktop (prd-remote-desktop-tab-v1 §6, §8;
// D7b). The socket, the engine's player, and what the daemon said.
//
//  - Dials once the pane is shown and the desktop is ready for this viewer
//    (a fresh ticket each time). A hidden tab keeps its socket: it sends
//    `fps 0` and, when shown again, its rate and a `keyframe`
//    (`DesktopSession.setHidden`).
//  - Frames go to the engine's player (`Stream/stream-playback.ts`): a
//    worker where the webview allows it, else this thread. The offer is
//    what that player decodes (H.264, VP8, else JPEG).
//  - `cursor` / `cursor-pos` go straight to the cursor overlay (no React
//    render); `control`, `displays`, `hello`, `fps` move the toolbar.
//  - A drop reconnects on its own a few times (1 s, 2 s, 4 s); a 4401,
//    4403 or 4410 doesn't (`reconnectsAfter`): the reason is shown, and
//    `reconnect()` dials again on a gesture.
//  - The latency readout: the daemon's capture-to-send time (`stats`,
//    p50) plus this side's receive-to-drawn median, once a second.

import { useCallback, useEffect, useRef, useState } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import { STREAM_WINDOW, type StreamFrame } from '@/components/Stream/stream-wire'
import { decodeSupport } from '@/components/Stream/stream-video'
import type { WorkerLike } from '@/components/Stream/stream-player'
import { StreamPlayback, type PlayerKind } from '@/components/Stream/stream-playback'
import { median } from '@/components/Mobile/sim-latency'
import { DesktopSession, type ControlState, type Gesture } from './desktop-session'
import { wsDesktopStream, type DesktopConnState, type DesktopStreamConnection, type DesktopStreamConnector } from './desktop-stream'
import { CLOSE_DESKTOP_OFF, isInputMsg, reconnectsAfter, type Codec, type Display, type HostOs, type ServerMsg, type ViewerMsg, type ViewerPath } from './desktop-wire'

/** Retries after a drop, and the wait before each. */
export const RECONNECT_DELAYS_MS: readonly number[] = [1000, 2000, 4000]

export interface CursorSink {
  setShape(id: number, hotX: number, hotY: number, png: string): void
  move(x: number, y: number, visible: boolean): void
  reset(): void
}

export interface UseDesktopStreamArgs {
  scope: ServerScope
  /** The desktop is ready for this viewer (on, allowed, not this machine). */
  active: boolean
  visible: boolean
  mayControl: boolean
  /** Start view-only (`view=1`). */
  viewOnly: boolean
  /** The display asked for at dial time. */
  display: string | null
  canvasRef: React.RefObject<HTMLCanvasElement | null>
  cursor: CursorSink
  connector?: DesktopStreamConnector
  startWorker?: () => WorkerLike
  /** The canvas width in device pixels at dial time (`pw=`). */
  paneWidth?: () => number
}

export interface DesktopStreamView {
  conn: DesktopConnState | 'idle'
  reason: string | null
  closeCode: number | null
  /** deskd sends the pointer out of band (a `cursor` shape came). Until
   *  then it is drawn into the picture, and the pane keeps the local one. */
  remoteCursor: boolean
  control: ControlState
  displays: Display[]
  display: string | null
  codec: Codec | null
  path: ViewerPath | null
  os: HostOs | null
  fps: number | null
  /** The display in points, from the newest frame. */
  screen: { widthPt: number; heightPt: number } | null
  hasFrame: boolean
  inputError: string | null
  latencyMs: number | null
  player: PlayerKind
  canvasKey: number
  /** Input (pointer, keys, …): sent only while this viewer drives. */
  send(m: ViewerMsg): boolean
  /** A gesture's messages (take control, hand back, display, …). */
  gesture(g: Gesture): boolean
  session: DesktopSession
  /** The host's clipboard text (null: refused, with why). */
  clipboardGet(): Promise<{ text: string | null; reason?: string }>
  reconnect(): void
}

export function useDesktopStream(args: UseDesktopStreamArgs): DesktopStreamView {
  const { scope, active, visible, mayControl, canvasRef, startWorker } = args
  const connector = args.connector ?? wsDesktopStream
  const [session] = useState(() => new DesktopSession({ mayControl }))
  session.mayControl = mayControl
  const [, setVersion] = useState(0)
  const bump = useCallback(() => setVersion((v) => v + 1), [])
  const [conn, setConn] = useState<DesktopConnState | 'idle'>('idle')
  const [reason, setReason] = useState<string | null>(null)
  const [remoteCursor, setRemoteCursor] = useState(false)
  // The daemon's last `state` line: a 4410 ends on it (why the picture
  // stopped), not on the close code's generic sentence.
  const lastStateRef = useRef<string | null>(null)
  const [closeCode, setCloseCode] = useState<number | null>(null)
  const [codec, setCodec] = useState<Codec | null>(null)
  const [path, setPath] = useState<ViewerPath | null>(null)
  const [os, setOs] = useState<HostOs | null>(null)
  const [screen, setScreen] = useState<{ widthPt: number; heightPt: number } | null>(null)
  const [hasFrame, setHasFrame] = useState(false)
  const [inputError, setInputError] = useState<string | null>(null)
  const inputErrorRef = useRef<string | null>(null)
  const [latencyMs, setLatencyMs] = useState<number | null>(null)
  const [dial, setDial] = useState(0)
  const [started, setStarted] = useState(false)
  const connRef = useRef<DesktopStreamConnection | null>(null)
  const cursorRef = useRef(args.cursor)
  cursorRef.current = args.cursor
  const argsRef = useRef(args)
  argsRef.current = args
  const clipWaiters = useRef<Array<(r: { text: string | null; reason?: string }) => void>>([])
  // This side's receive → drawn times, per seq and over the last second.
  const recvAt = useRef(new Map<number, number>())
  const localMs = useRef<number[]>([])
  const retries = useRef(0)

  const canvasRefRef = useRef(canvasRef)
  canvasRefRef.current = canvasRef
  const [playback] = useState(
    () =>
      new StreamPlayback(
        {
          drawn: (seq, _decodedAt, drawnAt) => {
            if (!hasFrameRef.current) {
              hasFrameRef.current = true
              setHasFrame(true)
            }
            const r = recvAt.current.get(seq)
            if (r !== undefined) {
              recvAt.current.delete(seq)
              localMs.current.push(drawnAt - r)
              if (localMs.current.length > 120) localMs.current.splice(0, localMs.current.length - 120)
            }
          },
          ack: (seq) => connRef.current?.ack(seq),
          needKeyframe: () => connRef.current?.send({ t: 'keyframe' }),
          videoFailed: () => setJpegOnly(true),
          player: (kind) => {
            setPlayer(kind)
            if (kind === 'main') setCanvasKey((k) => k + 1)
          },
        },
        { canvas: () => canvasRefRef.current.current, held: () => false, startWorker, tag: 'desktop' },
      ),
  )
  const hasFrameRef = useRef(false)
  const [player, setPlayer] = useState<PlayerKind>(() => playback.kind)
  const [canvasKey, setCanvasKey] = useState(0)
  const [jpegOnly, setJpegOnly] = useState(false)
  useEffect(() => () => playback.close(), [playback])

  // The first show starts the stream; a hide later only pauses it.
  useEffect(() => {
    if (active && visible) setStarted(true)
    if (!active) setStarted(false)
  }, [active, visible])
  const wantPlayer = player === 'pending' && started
  useEffect(() => {
    if (wantPlayer) playback.start()
  }, [wantPlayer, playback])
  const settled = player !== 'pending'

  useEffect(() => {
    if (!started || !settled) {
      setConn('idle')
      return
    }
    playback.reset()
    let live = true
    let c: DesktopStreamConnection | null = null
    let retryTimer: ReturnType<typeof setTimeout> | null = null
    hasFrameRef.current = false
    setHasFrame(false)
    void (async () => {
      const worker = playback.workerDecode
      const probed = worker ?? (await decodeSupport())
      if (!live) return
      const decode = jpegOnly ? { h264: false, vp8: false, jpeg: true } : probed
      const a = argsRef.current
      const pw = a.paneWidth?.() ?? 0
      c = connector({
        scope,
        decode,
        window: STREAM_WINDOW,
        display: a.display,
        view: a.viewOnly,
        stats: true,
        ...(pw > 0 ? { pw } : {}),
        handlers: {
          onConn(s, why, code) {
            if (!live) return
            setConn(s)
            const ended = s === 'closed' && code === CLOSE_DESKTOP_OFF ? lastStateRef.current : null
            setReason(ended ?? why ?? null)
            setCloseCode(code ?? null)
            if (s === 'open') {
              lastStateRef.current = null
              retries.current = 0
              // Opened while hidden: pause at once.
              if (session.hidden) {
                session.hidden = false
                for (const m of session.setHidden(true)) c?.send(m)
              }
              return
            }
            if (s === 'connecting') return
            hasFrameRef.current = false
            setHasFrame(false)
            cursorRef.current.reset()
            setRemoteCursor(false)
            for (const w of clipWaiters.current.splice(0)) w({ text: null, reason: 'closed' })
            // A ticket refusal carries an HTTP status: never retried.
            const drop = s === 'closed' ? reconnectsAfter(code ?? 1006) : code === undefined
            // A failed socket says `error` then `closed`: one retry for both.
            if (drop && !retryTimer && retries.current < RECONNECT_DELAYS_MS.length) {
              const wait = RECONNECT_DELAYS_MS[retries.current]
              retries.current += 1
              retryTimer = setTimeout(() => {
                if (live) setDial((d) => d + 1)
              }, wait)
            }
          },
          onServer(m: ServerMsg) {
            if (!live) return
            switch (m.t) {
              case 'cursor':
                cursorRef.current.setShape(m.id, m.hotX, m.hotY, m.png)
                setRemoteCursor(true)
                return
              case 'cursor-pos':
                cursorRef.current.move(m.x, m.y, m.visible)
                return
              case 'hello':
                setCodec(m.codec)
                setPath(m.path)
                setOs(m.os)
                break
              case 'codec':
                console.warn('[desktop] the stream switched to', m.codec, m.message ?? '')
                setCodec(m.codec)
                if (m.codec === 'jpeg') playback.dropVideo()
                return
              case 'clipboard':
                clipWaiters.current.shift()?.({ text: m.text, ...(m.reason ? { reason: m.reason } : {}) })
                return
              case 'error':
                console.warn('[desktop] refused:', m.code, m.message)
                inputErrorRef.current = m.message
                setInputError(m.message)
                return
              case 'stats': {
                const total = m.stats.stages.find((s) => s.stage === 'total')
                const here = median(localMs.current)
                localMs.current = []
                if (total || here !== null) setLatencyMs((total?.p50 ?? 0) + (here ?? 0))
                return
              }
              case 'state':
                lastStateRef.current = m.message
                setReason(m.message)
                return
            }
            if (session.onServer(m)) bump()
          },
          onFrame(frame: StreamFrame) {
            if (!live) return
            if (recvAt.current.size > 64) recvAt.current.clear()
            recvAt.current.set(frame.seq, performance.now())
            setScreen((prev) => (prev && prev.widthPt === frame.widthPt && prev.heightPt === frame.heightPt ? prev : { widthPt: frame.widthPt, heightPt: frame.heightPt }))
            playback.frame(frame)
          },
        },
      })
      connRef.current = c
    })()
    return () => {
      live = false
      if (retryTimer) clearTimeout(retryTimer)
      playback.end()
      c?.close()
      if (connRef.current === c) connRef.current = null
    }
  }, [scope, started, settled, connector, jpegOnly, dial, playback, session, bump])

  // Hidden: pause (the socket stays); shown: the rate again, and a key.
  useEffect(() => {
    const msgs = session.setHidden(!visible)
    const c = connRef.current
    if (!c) return
    for (const m of msgs) c.send(m)
  }, [visible, session])

  const send = useCallback(
    (m: ViewerMsg): boolean => {
      const c = connRef.current
      if (!c) return false
      if (isInputMsg(m) && !session.drives(argsRef.current.viewOnly)) return false
      if (inputErrorRef.current !== null) {
        inputErrorRef.current = null
        setInputError(null)
      }
      session.sent(m)
      c.send(m)
      return true
    },
    [session],
  )

  const gesture = useCallback((g: Gesture): boolean => {
    const c = connRef.current
    if (!c || !('send' in g)) return false
    for (const m of g.send) c.send(m)
    return true
  }, [])

  const clipboardGet = useCallback((): Promise<{ text: string | null; reason?: string }> => {
    const g = session.clipboardGet()
    const c = connRef.current
    if (!c || !('send' in g)) return Promise.resolve({ text: null, reason: 'not_controller' })
    return new Promise((resolve) => {
      clipWaiters.current.push(resolve)
      for (const m of g.send) c.send(m)
    })
  }, [session])

  const reconnect = useCallback(() => {
    retries.current = 0
    setDial((d) => d + 1)
  }, [])

  return {
    conn,
    reason,
    closeCode,
    remoteCursor,
    control: session.control,
    displays: session.displays,
    display: session.display,
    codec,
    path,
    os,
    fps: session.fps,
    screen,
    hasFrame,
    inputError,
    latencyMs,
    player,
    canvasKey,
    send,
    gesture,
    session,
    clipboardGet,
    reconnect,
  }
}
