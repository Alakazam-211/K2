// The Mobile tab's pane (prd-mobile-dev-v1 §2; vs-live M-A1, M-A3, M-A7).
//
// A sibling of the Design tab and laid out the same way:
//  - Left: the agent's conversation in Thread view, exactly like Design
//    (`DesignThreadColumn`, Rosson 2026-10-27). Never the pinned Chat's
//    retained terminal.
//  - The split width is this window's, per room (`k2.windowChrome.mobile.`).
//  - Right: the device toolbar, then the device in a plain drawn bezel with
//    the live stream on a <canvas>. The canvas is ordinary DOM (M-A3): no
//    native child, no overlay arbiter, no freeze code, and the hosted web
//    client shows it too (no web stub).
//
// Daemon-first: the daemon owns the sim session (one per workspace), its
// state and who holds it. This pane reads `sim/status`, draws frames and
// sends gestures. It works against any `SimStreamConnector`; the default is
// the real `/cli/sim/stream` socket.
//
// S5 (pick → note → Send) is undecided. Its hooks are here and unused:
// `renderOverlay` draws over the canvas with the dp ↔ canvas mapping
// (`deviceRectToCanvas`), `holdFrame` keeps the last frame while a note
// pop-up is open, and the toolbar's `extra` slot takes the Select toggle.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useStore } from 'zustand'
import { useRoom } from '@/components/Room/RoomContext'
import { useIsTabVisible } from '@/contexts/TabVisibilityContext'
import { DesignThreadColumn } from '@/components/Design/DesignThreadColumn'
import { pinnedChatItem } from '@/components/Design/pinned-chat-item'
import type { MobileItemData } from '@/stores/tabs'
import { scopeMayWrite } from '@/kessel/server-scope'
import { MobileToolbar } from './MobileToolbar'
import { readMobileSplit, writeMobileSplit } from './mobile-split-sizes'
import {
  bootSim,
  fetchSimDevices,
  firstUsableLane,
  screenshotSim,
  SIM_BOOT_ROUTE,
  stopSim,
  type SimDeviceProfile,
  type SimStatus,
} from './simApi'
import { type BoxRect, gestureFor, keyToSimInput, type PointerSample } from './sim-input'
import { defaultFps, type SimStreamConnector, type SimStreamMeta, wsSimStream } from './sim-stream'
import { useSimStatus } from './useSimStatus'
import { useSimStream } from './useSimStream'

/** Smallest share either side of the split keeps (flex units of 100). */
const MIN_SPLIT_SHARE = 15

export interface MobileOverlayContext {
  /** The canvas box in client px. */
  box: BoxRect
  meta: SimStreamMeta
}

export interface MobilePaneProps {
  itemId: string
  tabId: string
  paneGroupId: string
  data: MobileItemData
  /** Test seam / stub: the stream (default: the daemon socket). */
  connector?: SimStreamConnector
  /** S5 hook: drawn over the canvas (pick outlines, note badges). */
  renderOverlay?: (ctx: MobileOverlayContext) => React.ReactNode
  /** S5 hook: keep the last frame (a note pop-up is open). */
  holdFrame?: boolean
  /** S5 hook: the toolbar's Select toggle. */
  toolbarExtra?: React.ReactNode
}

export function MobilePane({
  itemId,
  tabId,
  paneGroupId,
  data,
  connector = wsSimStream,
  renderOverlay,
  holdFrame = false,
  toolbarExtra,
}: MobilePaneProps): React.JSX.Element {
  const room = useRoom()
  const tabVisible = useIsTabVisible()

  // ── Thread column (same as Design) ────────────────────────────────────
  const chatAgent = useStore(room.tabs, (s) => pinnedChatItem(s.tabs)?.agentName ?? null)
  const chatPath = useStore(room.tabs, (s) => pinnedChatItem(s.tabs)?.projectPath ?? null)
  const chatSession = useStore(room.tabs, (s) => pinnedChatItem(s.tabs)?.sessionId)
  const showChat = !!chatAgent && !!chatPath
  const workspace = chatPath ?? room.cwd()

  // ── Split width (this window, this room) ──────────────────────────────
  const roomKey = room.roomId() ?? room.key
  const [split, setSplit] = useState<[number, number]>(() => readMobileSplit(roomKey))
  useEffect(() => {
    setSplit(readMobileSplit(roomKey))
  }, [roomKey])
  const rowRef = useRef<HTMLDivElement | null>(null)
  const startSplitDrag = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault()
      const row = rowRef.current
      if (!row) return
      const box = row.getBoundingClientRect()
      if (box.width <= 0) return
      let latest: [number, number] = split
      const onMove = (ev: MouseEvent): void => {
        const chat = Math.round(((ev.clientX - box.left) / box.width) * 100)
        const clamped = Math.max(MIN_SPLIT_SHARE, Math.min(100 - MIN_SPLIT_SHARE, chat))
        latest = [clamped, 100 - clamped]
        setSplit(latest)
      }
      const onUp = (): void => {
        window.removeEventListener('mousemove', onMove)
        window.removeEventListener('mouseup', onUp)
        writeMobileSplit(roomKey, latest)
      }
      window.addEventListener('mousemove', onMove)
      window.addEventListener('mouseup', onUp)
    },
    [roomKey, split],
  )

  return (
    <div ref={rowRef} className="flex h-full w-full min-w-0" data-mobile-pane={itemId}>
      {showChat && chatAgent && chatPath ? (
        <>
          <div className="h-full min-w-0 overflow-hidden" style={{ flex: `${split[0]} 1 0` }} data-mobile-chat="">
            <DesignThreadColumn
              projectPath={chatPath}
              agentName={chatAgent}
              conversationId={chatSession ?? null}
              visible={tabVisible}
            />
          </div>
          <div
            role="separator"
            aria-orientation="vertical"
            aria-label="Resize Thread and device"
            className="w-1 flex-shrink-0 cursor-col-resize bg-[var(--color-border)] hover:bg-[var(--color-accent)]"
            onMouseDown={startSplitDrag}
            data-mobile-split=""
          />
        </>
      ) : null}
      <div className="h-full min-w-0" style={{ flex: `${showChat ? split[1] : 100} 1 0` }}>
        <MobileDevice
          itemId={itemId}
          tabId={tabId}
          paneGroupId={paneGroupId}
          workspace={workspace}
          data={data}
          visible={tabVisible}
          connector={connector}
          renderOverlay={renderOverlay}
          holdFrame={holdFrame}
          toolbarExtra={toolbarExtra}
        />
      </div>
    </div>
  )
}

interface MobileDeviceProps {
  itemId: string
  tabId: string
  paneGroupId: string
  workspace: string | null
  data: MobileItemData
  visible: boolean
  connector: SimStreamConnector
  renderOverlay?: MobilePaneProps['renderOverlay']
  holdFrame: boolean
  toolbarExtra?: React.ReactNode
}

/** What the device area says when there is no stream to draw. */
export function deviceNotice(status: SimStatus | null, statusError: string | null): string | null {
  if (!status) return statusError ? `Can't reach the device service: ${statusError}` : 'Checking for devices…'
  const lane = firstUsableLane(status.lanes)
  const s = status.session
  if (!s || s.state === 'off') {
    if (!lane) {
      const why = status.lanes.map((l) => l.reason).filter(Boolean).join(' ')
      return `This server can't run a device yet.${why ? ` ${why}` : ''}`
    }
    return 'No device is running.'
  }
  if (s.state === 'booting') return `Starting ${s.device}…`
  if (s.state === 'stopping') return `Stopping ${s.device}…`
  if (s.state === 'error') return s.error ? `The device stopped: ${s.error}` : 'The device stopped with an error.'
  return null
}

function MobileDevice({
  itemId,
  tabId,
  paneGroupId,
  workspace,
  data,
  visible,
  connector,
  renderOverlay,
  holdFrame,
  toolbarExtra,
}: MobileDeviceProps): React.JSX.Element {
  const room = useRoom()
  const scope = room.scope
  const { status, error: statusError, refresh } = useSimStatus(scope, workspace, visible)
  const session = status?.session ?? null
  // The daemon answers per login (remote Members watch only). A Home room on
  // another server also watches only until its server's sim routes are on
  // the room write allowlist (kessel/room-writes.ts).
  const canControl = (status?.canControl ?? false) && scopeMayWrite(scope, SIM_BOOT_ROUTE)
  const running = session?.state === 'running'

  // ── Device profiles (the chooser) ─────────────────────────────────────
  const [devices, setDevices] = useState<SimDeviceProfile[]>([])
  const anyLane = !!status && firstUsableLane(status.lanes) !== null
  useEffect(() => {
    if (!workspace || !visible || !canControl || !anyLane) return
    let cancelled = false
    fetchSimDevices(scope, workspace)
      .then((list) => {
        if (!cancelled) setDevices(list)
      })
      .catch((err: unknown) => console.warn('[mobile] sim/list-devices:', err))
    return () => {
      cancelled = true
    }
  }, [scope, workspace, visible, canControl, anyLane])
  const [picked, setPicked] = useState<string>(data.deviceType ?? '')
  const device = session && session.state !== 'off' ? session.device : picked || devices[0]?.id || ''

  // ── Gestures to the daemon ────────────────────────────────────────────
  const [busy, setBusy] = useState(false)
  const [notice, setNotice] = useState<string | null>(null)
  const act = useCallback(
    async (what: string, fn: () => Promise<void>) => {
      setBusy(true)
      setNotice(null)
      try {
        await fn()
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err)
        console.warn(`[mobile] ${what}:`, msg)
        setNotice(msg)
      } finally {
        setBusy(false)
        refresh()
      }
    },
    [refresh],
  )
  const remember = useCallback(
    (deviceType: string) => {
      room.tabs.getState().setMobileItemDevice(tabId, paneGroupId, itemId, deviceType)
    },
    [room, tabId, paneGroupId, itemId],
  )
  const boot = useCallback(
    (which: string) => {
      if (!workspace) return
      const profile = devices.find((d) => d.id === which)
      remember(which)
      void act('boot', async () => {
        await bootSim(scope, workspace, { device: which, ...(profile ? { platform: profile.lane } : {}) })
      })
    },
    [act, devices, remember, scope, workspace],
  )
  const onDevice = useCallback(
    (which: string) => {
      setPicked(which)
      // Changing the device boots a new one; the daemon shuts the old one.
      if (session && (session.state === 'running' || session.state === 'error')) boot(which)
      else remember(which)
    },
    [boot, remember, session],
  )
  const onStop = useCallback(() => {
    if (!workspace) return
    void act('stop', () => stopSim(scope, workspace))
  }, [act, scope, workspace])
  const onScreenshot = useCallback(() => {
    if (!workspace) return
    void act('screenshot', async () => {
      const path = await screenshotSim(scope, workspace)
      try {
        await navigator.clipboard.writeText(path)
        setNotice(`Saved ${path} (path copied)`)
      } catch {
        setNotice(`Saved ${path}`)
      }
    })
  }, [act, scope, workspace])

  // ── Stream ────────────────────────────────────────────────────────────
  const [fps, setFps] = useState(() => defaultFps(scope.isRemote))
  const canvasRef = useRef<HTMLCanvasElement | null>(null)
  const stream = useSimStream({
    scope,
    simId: running && session ? session.id : null,
    visible,
    fps,
    connector,
    canvasRef,
    hold: holdFrame,
  })
  const live = stream.state === 'open'
  const meta: SimStreamMeta | null =
    stream.meta ??
    (session?.screen
      ? session.orientation === 'landscape'
        ? { width: session.screen.height, height: session.screen.width, orientation: 'landscape' }
        : { width: session.screen.width, height: session.screen.height, orientation: 'portrait' }
      : null)
  const control = stream.control ?? session?.control ?? { heldBy: null }
  const [tookOver, setTookOver] = useState(false)
  useEffect(() => {
    // The daemon ended the hold (Hand back elsewhere, 10 min idle).
    if (control.heldBy !== 'takeover') setTookOver(false)
  }, [control.heldBy])

  // ── Pointer and keys on the device ────────────────────────────────────
  const screenRef = useRef<HTMLDivElement | null>(null)
  const downRef = useRef<PointerSample | null>(null)
  const interactive = canControl && live
  const box = (): BoxRect | null => {
    const r = canvasRef.current?.getBoundingClientRect()
    return r ? { left: r.left, top: r.top, width: r.width, height: r.height } : null
  }
  const onPointerDown = (e: React.PointerEvent): void => {
    if (!interactive || e.button !== 0) return
    screenRef.current?.focus()
    downRef.current = { clientX: e.clientX, clientY: e.clientY, at: performance.now() }
    try {
      ;(e.target as Element).setPointerCapture?.(e.pointerId)
    } catch {
      // jsdom / a pointer that already ended.
    }
  }
  const onPointerUp = (e: React.PointerEvent): void => {
    const down = downRef.current
    downRef.current = null
    if (!interactive || !down || !meta) return
    const b = box()
    if (!b) return
    stream.send(gestureFor(down, { clientX: e.clientX, clientY: e.clientY, at: performance.now() }, b, meta))
  }
  const onKeyDown = (e: React.KeyboardEvent): void => {
    if (!interactive) return
    const r = keyToSimInput(e)
    if (r === 'pass') return
    e.preventDefault()
    e.stopPropagation()
    if (r === 'blur') {
      screenRef.current?.blur()
      return
    }
    stream.send(r.send)
  }

  const blocked = deviceNotice(status, statusError)
  // Off (or stopped with an error) on a usable lane: offer Start right
  // there, not only in the toolbar.
  const canStart =
    canControl && anyLane && !!device && (!session || session.state === 'off' || session.state === 'error')
  const portrait = (meta?.orientation ?? 'portrait') === 'portrait'
  const aspect = meta ? `${meta.width} / ${meta.height}` : portrait ? '9 / 19.5' : '19.5 / 9'
  const overlayBox = renderOverlay && meta && live ? box() : null

  return (
    <div className="flex h-full min-h-0 w-full flex-col" data-testid="mobile-device">
      <MobileToolbar
        session={session}
        devices={devices}
        device={device}
        canControl={canControl}
        busy={busy}
        live={live}
        tookOver={tookOver}
        fps={scope.isRemote ? fps : null}
        onDevice={onDevice}
        onBoot={() => boot(device)}
        onStop={onStop}
        onRotate={() => stream.send({ action: 'rotate', direction: 'right' })}
        onHome={() => stream.send({ action: 'button', button: 'home' })}
        onBack={() => stream.send({ action: 'button', button: 'back' })}
        onScreenshot={onScreenshot}
        onTakeOver={() => {
          stream.send({ action: 'take' })
          setTookOver(true)
        }}
        onHandBack={() => {
          stream.send({ action: 'release' })
          setTookOver(false)
        }}
        onFps={setFps}
        extra={toolbarExtra}
      />
      {notice ? (
        <div className="flex-shrink-0 border-b border-[var(--color-border)] px-3 py-1 text-xs text-[var(--color-text-secondary)]" data-mobile-notice="">
          {notice}
        </div>
      ) : null}
      <div className="relative flex min-h-0 flex-1 items-center justify-center overflow-hidden p-4">
        {running ? (
          <div
            className="relative flex max-h-full max-w-full items-center justify-center border-[10px] border-[var(--color-text-muted)] bg-black"
            style={{
              aspectRatio: aspect,
              height: portrait ? '100%' : 'auto',
              width: portrait ? 'auto' : '100%',
              borderRadius: 28,
            }}
            data-mobile-bezel={portrait ? 'portrait' : 'landscape'}
          >
            <div
              ref={screenRef}
              tabIndex={interactive ? 0 : -1}
              role="application"
              aria-label={`${session?.device ?? 'Device'} screen${interactive ? '' : ' (watching)'}`}
              className="relative h-full w-full overflow-hidden outline-none focus-visible:ring-2 focus-visible:ring-[var(--color-accent)]"
              style={{ borderRadius: 18, cursor: interactive ? 'pointer' : 'default', touchAction: 'none' }}
              onPointerDown={onPointerDown}
              onPointerUp={onPointerUp}
              onPointerCancel={() => {
                downRef.current = null
              }}
              onKeyDown={onKeyDown}
              data-mobile-screen={interactive ? 'interactive' : 'watching'}
            >
              <canvas ref={canvasRef} className="block h-full w-full" data-mobile-canvas="" />
              {!stream.hasFrame ? (
                <div className="absolute inset-0 flex items-center justify-center text-xs text-[var(--color-text-muted)]">
                  {stream.state === 'error' || stream.state === 'closed'
                    ? stream.reason ?? 'The device stream closed. Reopen the tab to try again.'
                    : 'Connecting to the device…'}
                </div>
              ) : null}
              {overlayBox && meta && renderOverlay ? renderOverlay({ box: overlayBox, meta }) : null}
            </div>
          </div>
        ) : (
          <div className="flex max-w-sm flex-col items-center gap-3">
            <div className="text-center text-xs text-[var(--color-text-muted)]" data-mobile-blocked="">
              {blocked}
            </div>
            {canStart ? (
              <button
                type="button"
                className="h-7 cursor-pointer border border-[var(--color-border)] px-3 text-xs text-[var(--color-text)] hover:border-[var(--color-accent)] disabled:cursor-default disabled:opacity-50"
                disabled={busy}
                onClick={() => boot(device)}
                data-mobile-start=""
              >
                Start {devices.find((d) => d.id === device)?.name ?? device}
              </button>
            ) : null}
          </div>
        )}
        {control.heldBy && !tookOver ? (
          <div
            className="absolute bottom-2 left-1/2 -translate-x-1/2 border border-[var(--color-border)] bg-[var(--color-bg)] px-2 py-0.5 text-xs text-[var(--color-text-secondary)]"
            data-mobile-held={control.heldBy}
          >
            {control.heldBy === 'takeover'
              ? `${control.by ?? 'A person'} has taken over the device`
              : `${control.by ?? 'A person'} is using the device`}
          </div>
        ) : null}
      </div>
    </div>
  )
}
