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
// `renderOverlay` draws over the canvas with the points ↔ canvas mapping
// (`deviceRectToCanvas`), `holdFrame` keeps the last frame while a note
// pop-up is open, and the toolbar's `extra` slot takes the Select toggle.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useStore } from 'zustand'
import { useRoom } from '@/components/Room/RoomContext'
import { useIsTabVisible } from '@/contexts/TabVisibilityContext'
import { DesignThreadColumn } from '@/components/Design/DesignThreadColumn'
import { pinnedChatItem } from '@/components/Design/pinned-chat-item'
import type { MobileItemData } from '@/stores/tabs'
import { roomProjectForCwd } from '@/stores/room'
import { MobileToolbar } from './MobileToolbar'
import { readMobileSplit, writeMobileSplit } from './mobile-split-sizes'
import {
  bootSim,
  fetchSimDevices,
  firstUsableLane,
  screenshotSim,
  simIsUp,
  stopSim,
  type SimDeviceProfile,
  type SimHold,
  type SimLaneId,
  type SimStatus,
} from './simApi'
import { type BoxRect, gestureFor, keyToSimInput, nextOrientation, type PointerSample, type ScreenPt } from './sim-input'
import { defaultFps, type SimStreamConnector, wsSimStream } from './sim-stream'
import { useSimCanControl } from './useSimCanControl'
import { useSimStatus } from './useSimStatus'
import { useSimStream } from './useSimStream'

/** Smallest share either side of the split keeps (flex units of 100). */
const MIN_SPLIT_SHARE = 15

export interface MobileOverlayContext {
  /** The canvas box in client px. */
  box: BoxRect
  /** The screen in points (map tree frames with `deviceRectToCanvas`). */
  screen: ScreenPt
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
  const s = status.sim
  if (s?.state === 'installing') return `Installing the device tools for ${s.profile?.name ?? 'the device'}… (first use only)`
  if (s?.state === 'booting') return `Starting ${s.profile?.name ?? 'the device'}…`
  if (s?.state === 'stopping') return `Stopping ${s.profile?.name ?? 'the device'}…`
  if (s?.state === 'running') return null
  const lane = firstUsableLane(status.lanes)
  if (!lane) {
    const why = status.lanes.map((l) => l.detail).filter(Boolean).join(' ')
    return `This server can't run a device yet.${why ? ` ${why}` : ''}`
  }
  if (!lane.ready) {
    const licence = lane.needs.find((n) => n.kind === 'licence')
    if (licence && licence.kind === 'licence') {
      return `The owner of this server must accept the ${licence.title} first (on its card in Settings).`
    }
    const step = lane.needs.find((n) => n.kind === 'owner-step')
    if (step && step.kind === 'owner-step') return `The owner of this server must ${step.what} first: ${step.command}`
  }
  if (s?.state === 'failed') return s.reason ? `The device stopped: ${s.reason}` : 'The device stopped with an error.'
  return 'No device is running.'
}

/** How the hold banner reads, or null when nothing shows. */
export function holdBanner(hold: SimHold, tookOver: boolean): string | null {
  if (hold.kind === 'person') return `${hold.by || 'A person'} is using the device`
  if (hold.kind === 'takeover' && !tookOver) return `${hold.by || 'A person'} has taken over the device`
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
  // The daemon names a workspace by handle or project id (sim::api); the
  // room's project id when it knows the folder, else the folder itself.
  const workspaceRef = useStore(room.projects, () => (workspace ? roomProjectForCwd(room, workspace)?.id ?? workspace : null))
  const { status, error: statusError, refresh } = useSimStatus(scope, workspaceRef, visible)
  const session = status?.sim ?? null
  const canControl = useSimCanControl(scope, status?.canControl ?? null)
  const running = session?.state === 'running'

  // ── Device profiles (the chooser) ─────────────────────────────────────
  const [devices, setDevices] = useState<SimDeviceProfile[]>([])
  const [devicesLane, setDevicesLane] = useState<SimLaneId | null>(null)
  const anyLane = !!status && firstUsableLane(status.lanes) !== null
  useEffect(() => {
    if (!workspaceRef || !visible || !canControl || !anyLane) return
    let cancelled = false
    fetchSimDevices(scope, workspaceRef)
      .then((r) => {
        if (cancelled) return
        setDevices(r.devices)
        setDevicesLane(r.lane)
      })
      .catch((err: unknown) => console.warn('[mobile] sim/devices:', err))
    return () => {
      cancelled = true
    }
  }, [scope, workspaceRef, visible, canControl, anyLane])
  const [picked, setPicked] = useState<string>(data.deviceType ?? '')
  const fallbackDevice = devices.find((d) => d.default)?.id ?? devices[0]?.id ?? ''
  const device = session && simIsUp(session) && session.profile ? session.profile.id : picked || fallbackDevice

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
      if (!workspaceRef) return
      remember(which)
      void act('boot', async () => {
        await bootSim(scope, workspaceRef, { device: which, ...(devicesLane ? { platform: devicesLane } : {}) })
      })
    },
    [act, devicesLane, remember, scope, workspaceRef],
  )
  const onDevice = useCallback(
    (which: string) => {
      setPicked(which)
      // Changing the device boots a new one; the daemon shuts the old one.
      if (session && (session.state === 'running' || session.state === 'failed')) boot(which)
      else remember(which)
    },
    [boot, remember, session],
  )
  const onStop = useCallback(() => {
    if (!workspaceRef) return
    void act('stop', async () => {
      await stopSim(scope, workspaceRef)
    })
  }, [act, scope, workspaceRef])
  const onScreenshot = useCallback(() => {
    if (!workspaceRef) return
    void act('screenshot', async () => {
      const path = await screenshotSim(scope, workspaceRef)
      try {
        await navigator.clipboard.writeText(path)
        setNotice(`Saved ${path} (path copied)`)
      } catch {
        setNotice(`Saved ${path}`)
      }
    })
  }, [act, scope, workspaceRef])

  // ── Stream ────────────────────────────────────────────────────────────
  const [fps, setFpsState] = useState(() => defaultFps(scope.isRemote))
  const canvasRef = useRef<HTMLCanvasElement | null>(null)
  const stream = useSimStream({
    scope,
    // The daemon's own name for the workspace, once it reported the device.
    workspace: running && session ? session.workspace : null,
    visible,
    fps,
    connector,
    canvasRef,
    hold: holdFrame,
  })
  const live = stream.conn === 'open'
  const screen: ScreenPt | null =
    stream.screen ?? (session?.profile ? { widthPt: session.profile.widthPt, heightPt: session.profile.heightPt } : null)
  const hold: SimHold = stream.deviceHold ?? session?.hold ?? { kind: 'free' }
  const [tookOver, setTookOver] = useState(false)
  useEffect(() => {
    // The daemon ended the hold (Hand back elsewhere, 10 min idle).
    if (hold.kind !== 'takeover') setTookOver(false)
  }, [hold.kind])
  const onFps = useCallback(
    (n: number) => {
      setFpsState(n)
      stream.setFps(n)
    },
    [stream],
  )

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
    if (!interactive || !down || !screen) return
    const b = box()
    if (!b) return
    stream.send(gestureFor(down, { clientX: e.clientX, clientY: e.clientY, at: performance.now() }, b, screen))
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
  const laneReady = !!status && status.lanes.some((l) => l.usable && l.ready)
  // Stopped (or failed) on a ready lane: offer Start right there, not only
  // in the toolbar.
  const canStart = canControl && laneReady && !!device && !simIsUp(session)
  const portrait = !screen || screen.heightPt >= screen.widthPt
  const aspect = screen ? `${screen.widthPt} / ${screen.heightPt}` : '9 / 19.5'
  const overlayBox = renderOverlay && screen && live ? box() : null
  const banner = holdBanner(hold, tookOver)
  const shownNotice = notice ?? stream.inputError

  return (
    <div className="flex h-full min-h-0 w-full flex-col" data-testid="mobile-device">
      <MobileToolbar
        session={session}
        lane={devicesLane}
        devices={devices}
        device={device}
        canControl={canControl}
        busy={busy}
        canBoot={laneReady}
        live={live}
        tookOver={tookOver}
        fps={scope.isRemote ? stream.fps ?? fps : null}
        onDevice={onDevice}
        onBoot={() => boot(device)}
        onStop={onStop}
        onRotate={() => screen && stream.send({ t: 'rotate', orientation: nextOrientation(screen) })}
        onHome={() => stream.send({ t: 'button', button: 'home' })}
        onBack={() => stream.send({ t: 'button', button: 'back' })}
        onScreenshot={onScreenshot}
        onTakeOver={() => {
          stream.send({ t: 'take-over' })
          setTookOver(true)
        }}
        onHandBack={() => {
          stream.send({ t: 'hand-back' })
          setTookOver(false)
        }}
        onFps={onFps}
        extra={toolbarExtra}
      />
      {shownNotice ? (
        <div className="flex-shrink-0 border-b border-[var(--color-border)] px-3 py-1 text-xs text-[var(--color-text-secondary)]" data-mobile-notice="">
          {shownNotice}
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
              aria-label={`${session?.profile?.name ?? 'Device'} screen${interactive ? '' : ' (watching)'}`}
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
                <div className="absolute inset-0 flex items-center justify-center px-4 text-center text-xs text-[var(--color-text-muted)]" data-mobile-stream-note="">
                  {stream.conn === 'error' || stream.conn === 'closed'
                    ? stream.reason ?? 'The device stream closed. Reopen the tab to try again.'
                    : 'Connecting to the device…'}
                </div>
              ) : null}
              {overlayBox && screen && renderOverlay ? renderOverlay({ box: overlayBox, screen }) : null}
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
        {banner ? (
          <div
            className="absolute bottom-2 left-1/2 -translate-x-1/2 border border-[var(--color-border)] bg-[var(--color-bg)] px-2 py-0.5 text-xs text-[var(--color-text-secondary)]"
            data-mobile-held={hold.kind}
          >
            {banner}
          </div>
        ) : null}
      </div>
      {running && stream.isolation ? (
        <div className="flex-shrink-0 px-3 pb-2 text-center text-[11px] text-[var(--color-text-muted)]" data-mobile-isolation="">
          {stream.isolation}
        </div>
      ) : null}
    </div>
  )
}
