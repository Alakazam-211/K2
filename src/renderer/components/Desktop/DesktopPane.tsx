// The Desktop tab's pane (prd-remote-desktop-tab-v1 §8): the toolbar, the
// state cards, and the live picture with its input (D7b).
//
// Daemon-first: the daemon owns the switch, who may open, the control
// lease, the display and the machine's state. This pane reads
// `GET /cli/desktop/status` on the ROOM's server (the server that owns the
// workspace whose strip holds this tab, vs-live R22), draws the one card
// that status means for this viewer (`tabState`), and once `ready` opens
// the stream (`useDesktopStream`) and sends this person's gestures. A
// server without `remote-desktop-v1` is never dialled.
//
//  - The picture: a <canvas> the engine's player draws; Fit (scaled to the
//    pane, centred) or 1:1 (one display pixel per device pixel, scrolled).
//  - The cursor: an <img> moved outside React (`cursor-overlay.ts`).
//  - Input only while this viewer holds control and isn't view-only:
//    pointer, buttons, wheel, physical keys by `code` (`routeKey`: Capture
//    keys off keeps K2's ⌘ shortcuts), composed text as `text`. Every key
//    and button held is released on blur, hide, view-only and hand-back.

import { useCallback, useEffect, useRef, useState } from 'react'
import { useToastStore } from '@/stores/toast'
import { useRoom } from '@/components/Room/RoomContext'
import { useIsTabVisible } from '@/contexts/TabVisibilityContext'
import { isMacPlatform } from '@/lib/desktop-chrome'
import type { DesktopItemData } from '@/stores/tabs'
import { CursorOverlay } from './cursor-overlay'
import { DesktopStateCard } from './DesktopStateCard'
import { DesktopToolbar } from './DesktopToolbar'
import { enableDesktop, fetchDesktopDisplays } from './desktop-api'
import { checkInput, keyCodeToSend, mapPointer, oneToOneCssPerPt, pictureLayout, pointerButton, splitText, wheelMode, type Fit } from './desktop-input'
import { chordEvents, keysMenu, routeKey, type KeysMenuEntry } from './desktop-keymap'
import type { DesktopFit } from './desktop-serialize'
import { tabState, type CardAction } from './desktop-state'
import type { DesktopStreamConnector } from './desktop-stream'
import { CLOSE_DESKTOP_OFF, type Display, type ViewerMsg } from './desktop-wire'
import { useDesktopStatus } from './useDesktopStatus'
import { useDesktopStream } from './useDesktopStream'
import type { WorkerLike } from '@/components/Stream/stream-player'

export const DESKTOP_CONNECTING = 'Connecting to this computer’s screen…'

export interface DesktopPaneProps {
  tabId: string
  paneGroupId: string
  itemId: string
  data: DesktopItemData
  /** Tests: the socket and the player worker. */
  connector?: DesktopStreamConnector
  startWorker?: () => WorkerLike
}

function dpr(): number {
  return typeof window !== 'undefined' && window.devicePixelRatio > 0 ? window.devicePixelRatio : 1
}

export function DesktopPane(props: DesktopPaneProps): React.JSX.Element {
  const room = useRoom()
  const scope = room.scope
  const visible = useIsTabVisible()
  const { supported, status, error, refresh } = useDesktopStatus(scope, visible)
  const [busy, setBusy] = useState(false)
  const [enableCard, setEnableCard] = useState<string | undefined>(undefined)
  const [fetchedDisplays, setFetchedDisplays] = useState<Display[]>([])
  const [capture, setCapture] = useState(false)
  const [flip, setFlip] = useState(false)
  const canvasRef = useRef<HTMLCanvasElement | null>(null)
  const stageRef = useRef<HTMLDivElement | null>(null)
  const [cursor] = useState(() => new CursorOverlay())
  useEffect(() => () => cursor.dispose(), [cursor])

  const state = status ? tabState(status) : null
  const ready = state === 'ready'
  const fit: DesktopFit = props.data.fit ?? 'fit'
  const mayControl = !!status?.you.mayControl
  const viewOnly = !!props.data.viewOnly || !mayControl

  const stream = useDesktopStream({
    scope,
    active: ready && supported,
    visible,
    mayControl,
    viewOnly,
    display: props.data.displayId ?? null,
    canvasRef,
    cursor,
    connector: props.connector,
    startWorker: props.startWorker,
    paneWidth: () => {
      const c = canvasRef.current
      if (!c || typeof c.getBoundingClientRect !== 'function') return 0
      return Math.round(c.getBoundingClientRect().width * dpr())
    },
  })
  const live = stream.conn === 'open'
  const controlling = stream.control.you
  const drives = live && stream.session.drives(viewOnly)
  const drivesRef = useRef(drives)
  drivesRef.current = drives

  // The desktop went off under the socket: read status (its card says so).
  useEffect(() => {
    if (stream.closeCode === CLOSE_DESKTOP_OFF) refresh()
  }, [stream.closeCode, refresh])

  useEffect(() => {
    if (!ready || !visible) return
    let cancelled = false
    fetchDesktopDisplays(scope).then(
      (d) => {
        if (!cancelled) setFetchedDisplays(d)
      },
      (err: unknown) => console.warn('[desktop] desktop/displays:', err instanceof Error ? err.message : String(err)),
    )
    return () => {
      cancelled = true
    }
  }, [scope, ready, visible])
  const displays = stream.displays.length > 0 ? stream.displays : fetchedDisplays
  const current = stream.session.currentDisplay()
  const widthPt = stream.screen?.widthPt ?? current?.widthPt ?? 0
  const heightPt = stream.screen?.heightPt ?? current?.heightPt ?? 0
  const cssPerPt = oneToOneCssPerPt(current?.widthPx ?? widthPt, current?.widthPt ?? widthPt, dpr())
  const geomRef = useRef({ fit: fit as Fit, widthPt, heightPt, cssPerPt, imageScale: current?.scale ?? 1 })
  geomRef.current = { fit, widthPt, heightPt, cssPerPt, imageScale: current?.scale ?? 1 }

  // The cursor overlay follows the picture's place in the canvas.
  const relayout = useCallback(() => {
    const c = canvasRef.current
    const g = geomRef.current
    if (!c || typeof c.getBoundingClientRect !== 'function') return
    const r = c.getBoundingClientRect()
    cursor.setLayout(pictureLayout({ canvasW: r.width, canvasH: r.height, widthPt: g.widthPt, heightPt: g.heightPt, fit: g.fit, cssPerPt: g.cssPerPt }), g.imageScale)
  }, [cursor])
  useEffect(() => {
    relayout()
  }, [relayout, fit, widthPt, heightPt, cssPerPt, stream.canvasKey])
  useEffect(() => {
    const c = canvasRef.current
    if (!c || typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(() => relayout())
    ro.observe(c)
    return () => ro.disconnect()
  }, [relayout, stream.canvasKey, ready])

  const stamp = useCallback(
    (patch: Partial<Omit<DesktopItemData, 'desktopId'>>) => {
      room.tabs.getState().setDesktopItemState(props.tabId, props.paneGroupId, props.itemId, patch)
    },
    [room, props.tabId, props.paneGroupId, props.itemId],
  )

  const toast = (msg: string, kind: 'error' | 'info' = 'error'): void => {
    useToastStore.getState().addToast(msg, kind)
  }

  // ── Input ───────────────────────────────────────────────────────────
  const sendInput = useCallback(
    (m: ViewerMsg): void => {
      if (!drivesRef.current) return
      const refused = checkInput(m)
      if (refused) {
        console.warn('[desktop] input not sent:', refused.message)
        return
      }
      stream.send(m)
    },
    [stream],
  )

  const toPoint = (clientX: number, clientY: number): { x: number; y: number } | null => {
    const c = canvasRef.current
    if (!c) return null
    const r = c.getBoundingClientRect()
    const g = geomRef.current
    return mapPointer({ cx: clientX - r.left, cy: clientY - r.top, canvasW: r.width, canvasH: r.height, widthPt: g.widthPt, heightPt: g.heightPt, fit: g.fit, cssPerPt: g.cssPerPt })
  }

  const onPointerMove = (e: React.PointerEvent): void => {
    if (!drivesRef.current) return
    const p = toPoint(e.clientX, e.clientY)
    if (p) sendInput({ t: 'pointer', x: p.x, y: p.y })
  }
  const onPointerButton = (e: React.PointerEvent, down: boolean): void => {
    if (!drivesRef.current) return
    const b = pointerButton(e.button)
    if (!b) return
    e.preventDefault()
    if (down) {
      stageRef.current?.focus()
      const p = toPoint(e.clientX, e.clientY)
      if (!p) return
      sendInput({ t: 'pointer', x: p.x, y: p.y })
    }
    sendInput({ t: 'button', button: b, down })
  }
  const onWheel = (e: React.WheelEvent): void => {
    if (!drivesRef.current) return
    sendInput({ t: 'wheel', dx: e.deltaX, dy: e.deltaY, mode: wheelMode(e.deltaMode) })
  }
  const hostIsMac = stream.os === 'macos' || (stream.os === null && status?.backend === 'macos')
  const onKey = (e: React.KeyboardEvent, down: boolean): void => {
    if (!drivesRef.current || e.nativeEvent.isComposing) return
    const route = routeKey({ meta: e.metaKey, ctrl: e.ctrlKey, alt: e.altKey, shift: e.shiftKey, code: e.code }, capture, isMacPlatform())
    if (down && route === 'release') {
      e.preventDefault()
      setCapture(false)
      return
    }
    if (route === 'k2') {
      // K2 keeps the chord, but a key-up is always let through so nothing
      // stays held on the other side.
      if (!down) sendInput({ t: 'key', code: keyCodeToSend(e.code, isMacPlatform(), hostIsMac, flip), down: false })
      return
    }
    if (route === 'keys-menu') return
    e.preventDefault()
    if (down && e.repeat) return // the host repeats a held key itself
    sendInput({ t: 'key', code: keyCodeToSend(e.code, isMacPlatform(), hostIsMac, flip), down })
  }
  const onCompositionEnd = (e: React.CompositionEvent): void => {
    if (!drivesRef.current || !e.data) return
    for (const text of splitText(e.data)) sendInput({ t: 'text', text })
  }
  const onBlur = (): void => {
    for (const m of stream.session.releaseHeld()) stream.send(m)
  }

  const onKeys = (entry: KeysMenuEntry): void => {
    for (const k of chordEvents(entry.chord)) sendInput({ t: 'key', code: keyCodeToSend(k.code, isMacPlatform(), hostIsMac, flip), down: k.down })
  }
  const onClipSend = (): void => {
    const clip = typeof navigator !== 'undefined' ? navigator.clipboard : undefined
    if (!clip?.readText) {
      toast('This window can’t read your clipboard.')
      return
    }
    clip.readText().then(
      (text) => {
        const g = stream.session.clipboardSet(text)
        if (!('send' in g)) return
        const bad = checkInput(g.send[0])
        if (bad) {
          toast(bad.message)
          return
        }
        stream.gesture(g)
      },
      (err: unknown) => toast(`Couldn’t read your clipboard: ${err instanceof Error ? err.message : String(err)}`),
    )
  }
  const onClipCopy = (): void => {
    void stream.clipboardGet().then(async (r) => {
      if (r.text === null) {
        toast(r.reason === 'concealed' ? 'That copy on the other computer is a password; K2 won’t bring it over.' : 'Couldn’t read the other computer’s clipboard.')
        return
      }
      try {
        await navigator.clipboard.writeText(r.text)
        toast('Copied their clipboard text.', 'info')
      } catch (err) {
        toast(`Couldn’t write your clipboard: ${err instanceof Error ? err.message : String(err)}`)
      }
    })
  }

  const onAction = useCallback(
    (action: CardAction) => {
      if (action === 'turn-on' || action === 'ask-at-mac') {
        // Ask at the Mac re-arms K2 Screen, which asks for its permission.
        setBusy(true)
        enableDesktop(scope)
          .then(
            (r) => {
              if (r.card) setEnableCard(r.card)
            },
            (err: unknown) => {
              const msg = err instanceof Error ? err.message : String(err)
              useToastStore.getState().addToast(`Couldn’t turn on Remote desktop: ${msg}`, 'error')
            },
          )
          .finally(() => {
            setBusy(false)
            refresh()
          })
        return
      }
      refresh()
    },
    [scope, refresh],
  )

  if (!supported) {
    return <DesktopStateCard state="update_k2" mayManage={false} onAction={onAction} />
  }
  if (!status || !state) {
    return (
      <div className="flex h-full w-full items-center justify-center text-xs text-[var(--color-text-muted)]" data-desktop-loading="">
        {error ? `Couldn’t read this computer’s Remote desktop: ${error}` : 'Checking this computer…'}
      </div>
    )
  }
  const oneToOne = fit === '1:1'
  const frameStyle: React.CSSProperties | undefined = oneToOne && widthPt > 0 ? { width: widthPt * cssPerPt, height: heightPt * cssPerPt } : undefined
  const closed = stream.conn === 'closed' || stream.conn === 'error'
  return (
    <div className="flex h-full w-full flex-col" data-desktop-pane={state}>
      <DesktopToolbar
        machine={status.machine}
        displays={displays}
        displayId={stream.display ?? props.data.displayId ?? displays.find((d) => d.main)?.id ?? null}
        fit={fit}
        viewOnly={viewOnly}
        mayControl={mayControl}
        capture={capture}
        flipModifiers={flip}
        live={live}
        controlling={controlling}
        controller={stream.control.controller}
        localHold={stream.control.localHold}
        keys={keysMenu(stream.os ?? (hostIsMac ? 'macos' : 'linux'))}
        viewers={status.viewers ? status.viewers.length : null}
        latencyMs={live ? stream.latencyMs : null}
        onDisplay={(id) => {
          stamp({ displayId: id })
          stream.gesture(stream.session.selectDisplay(id))
        }}
        onFit={(f) => stamp({ fit: f })}
        onViewOnly={(on) => {
          stamp({ viewOnly: on })
          stream.gesture({ send: stream.session.setViewOnly(on) })
        }}
        onCapture={setCapture}
        onFlipModifiers={setFlip}
        onTakeControl={() => stream.gesture(stream.session.takeControl())}
        onHandBack={() => stream.gesture(stream.session.handBack())}
        onClipSend={onClipSend}
        onClipCopy={onClipCopy}
        onKeys={onKeys}
      />
      <div className="relative min-h-0 flex-1" data-desktop-stage="" data-desktop-viewer-mac={String(isMacPlatform())}>
        {ready ? (
          <div
            ref={stageRef}
            tabIndex={0}
            className={`absolute inset-0 outline-none ${oneToOne ? 'overflow-auto' : 'overflow-hidden'}`}
            data-desktop-picture={fit}
            data-desktop-drives={String(drives)}
            onPointerMove={onPointerMove}
            onPointerDown={(e) => onPointerButton(e, true)}
            onPointerUp={(e) => onPointerButton(e, false)}
            onWheel={onWheel}
            onContextMenu={(e) => {
              if (drivesRef.current) e.preventDefault()
            }}
            onKeyDown={(e) => onKey(e, true)}
            onKeyUp={(e) => onKey(e, false)}
            onCompositionEnd={onCompositionEnd}
            onBlur={onBlur}
          >
            <div className={oneToOne ? 'relative' : 'relative h-full w-full'} style={frameStyle} data-desktop-frame="">
              <canvas
                key={stream.canvasKey}
                ref={canvasRef}
                className={`block h-full w-full ${oneToOne ? '' : 'object-contain'}`}
                style={{ opacity: stream.hasFrame ? 1 : 0, cursor: drives && stream.remoteCursor ? 'none' : 'default' }}
                data-desktop-canvas=""
              />
              <img ref={(el) => cursor.attach(el)} alt="" aria-hidden="true" />
            </div>
            {!stream.hasFrame ? (
              <div className="pointer-events-none absolute inset-0 flex items-center justify-center text-xs text-[var(--color-text-muted)]" data-desktop-connecting="">
                {closed ? null : DESKTOP_CONNECTING}
              </div>
            ) : null}
            {closed ? (
              <div className="absolute inset-0 flex flex-col items-center justify-center gap-2 text-xs text-[var(--color-text-muted)]" data-desktop-stream-closed={String(stream.closeCode ?? '')}>
                <span>{stream.reason ?? 'The connection to this computer closed.'}</span>
                <button type="button" className="h-6 cursor-pointer border border-[var(--color-border)] px-2 text-[var(--color-text-primary)]" data-desktop-reconnect="" onClick={() => stream.reconnect()}>
                  Reconnect
                </button>
              </div>
            ) : null}
            {stream.inputError ? (
              <div className="pointer-events-none absolute bottom-2 left-1/2 -translate-x-1/2 border border-[var(--color-border)] bg-[var(--color-bg)] px-2 py-1 text-xs text-[var(--color-text-primary)]" data-desktop-input-error="">
                {stream.inputError}
              </div>
            ) : null}
          </div>
        ) : (
          <DesktopStateCard
            state={state}
            mayManage={status.you.mayManage}
            detail={state === status.state ? status.message : undefined}
            enableCard={enableCard}
            busy={busy}
            onAction={onAction}
          />
        )}
      </div>
    </div>
  )
}
