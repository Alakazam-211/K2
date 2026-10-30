// The Mobile tab's pane (prd-mobile-dev-v1 §2; vs-live M-A1, M-A3, M-A7).
//
// A sibling of the Design tab and laid out the same way:
//  - Left: the agent's conversation in Thread view, exactly like Design
//    (`DesignThreadColumn`, Rosson 2026-10-27). Never the pinned Chat's
//    retained terminal.
//  - The split width is this window's, per room (`k2.windowChrome.mobile.`).
//  - Right: the device toolbar, then the device in its phone frame
//    (DeviceFrame: island or punch-hole, proportional corners) with
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
import { writeText as writeClipboardText } from '@tauri-apps/plugin-clipboard-manager'
import { useToastStore } from '@/stores/toast'
import { DevicePicker, type DevicePickerGroup } from './DevicePicker'
import { MobileHoldPill, MobileNotice, type MobileNoticeTone } from './MobileNotice'
import { useThreadViewer, type ThreadViewer } from '@/lib/thread-viewer'
import { useSettingsStore } from '@/stores/settings'
import type { ComposeInsertDetail } from '@/lib/external-drop-router'
import { useRoom } from '@/components/Room/RoomContext'
import { useIsTabVisible } from '@/contexts/TabVisibilityContext'
import { DesignThreadColumn } from '@/components/Design/DesignThreadColumn'
import { pinnedChatItem } from '@/components/Design/pinned-chat-item'
import type { MobileItemData } from '@/stores/tabs'
import { roomProjectForCwd } from '@/stores/room'
import { DeviceFrame, useElementSize } from './DeviceFrame'
import { frameLayout, resolveDeviceShape } from './device-frame'
import { MobileBootScreen, type MobileBootPhase } from './MobileBootScreen'
import { MobileControlRail, RAIL_INSET_PX } from './MobileControlRail'
import {
  readMobileLatencyHud,
  readMobileLiveTouch,
  readMobileScreenOnly,
  writeMobileLatencyHud,
  writeMobileLiveTouch,
  writeMobileScreenOnly,
} from './mobile-view-prefs'
import { MobileToolbar } from './MobileToolbar'
import { DEFAULT_MOBILE_SPLIT, readMobileSplit, writeMobileSplit } from './mobile-split-sizes'
import {
  bootSim,
  fetchSimDevices,
  firstUsableLane,
  IPHONE_NEEDS_A_MAC,
  lanePlatform,
  offeredPlatforms,
  screenshotSim,
  simIsUp,
  stopSim,
  type SimDeviceProfile,
  type SimHold,
  type SimLaneId,
  type SimPlatform,
  type SimStatus,
} from './simApi'
import {
  type BoxRect,
  canvasPointToDevice,
  gestureFor,
  type LiveTouch,
  liveTouchEnd,
  liveTouchHold,
  liveTouchMove,
  liveTouchStart,
  nextOrientation,
  type PointerSample,
  type ScreenPt,
  TAP_MAX_MS,
} from './sim-input'
import { defaultFps, type SimAppearance, type SimInput, type SimStreamConnector, wsSimStream } from './sim-stream'
import { SimKeyboardHelp, useSimKeyboard } from './useSimKeyboard'
import {
  altDragFingers,
  cssToPoints,
  type Pinch,
  pinchBegin,
  PINCH_IDLE_MS,
  PINCH_UNAVAILABLE,
  pinchPoints,
  pinchWheelScale,
  releaseVelocity,
  trimSamples,
  twoFingers,
  type WheelDrag,
  wheelBegin,
  wheelDeltaPx,
  wheelEnd,
  WHEEL_IDLE_MS,
  wheelMove,
  withVelocity,
} from './sim-gestures'
import { LatencyMeter, type LatencyReadout, latencyText } from './sim-latency'
import { useSimCanControl } from './useSimCanControl'
import { useSimStatus } from './useSimStatus'
import { useSimStream } from './useSimStream'

/** A small JPEG of what the canvas shows (the screenshot toast's
 *  thumbnail), or null when the canvas can't export (no frame, jsdom). */
function canvasThumb(canvas: HTMLCanvasElement | null): string | null {
  if (!canvas || canvas.width === 0 || canvas.height === 0) return null
  try {
    const url = canvas.toDataURL('image/jpeg', 0.6)
    return typeof url === 'string' && url.startsWith('data:image/') ? url : null
  } catch {
    return null
  }
}

/** Below this pane width the Thread and the device switch instead of split. */
const NARROW_PX = 720

/** A reconnect says so only once it has taken this long. */
const RECONNECT_NOTE_AFTER_MS = 2000

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
  // Narrow windows (research §4 item 10): a Thread | Device switch instead
  // of a sliver of Thread beside a tiny phone.
  const [rowRef, rowSize] = useElementSize<HTMLDivElement>()
  const narrow = rowSize.width > 0 && rowSize.width < NARROW_PX
  const [narrowView, setNarrowView] = useState<'thread' | 'device'>('device')
  const resetSplit = useCallback(() => {
    const next: [number, number] = [DEFAULT_MOBILE_SPLIT[0], DEFAULT_MOBILE_SPLIT[1]]
    setSplit(next)
    writeMobileSplit(roomKey, next)
  }, [roomKey])
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

  const switched = narrow && showChat
  return (
    <div ref={rowRef} className="flex h-full w-full min-w-0 flex-col" data-mobile-pane={itemId}>
      {switched ? (
        <div className="flex flex-shrink-0 justify-center border-b border-[var(--color-border)] py-1.5" role="tablist" aria-label="Show" data-mobile-narrow="">
          <div className="flex border border-[var(--color-border)] p-0.5" style={{ borderRadius: 'calc(var(--radius-field, 0px) * 100)' }}>
            {(['thread', 'device'] as const).map((v) => (
              <button
                key={v}
                type="button"
                role="tab"
                aria-selected={narrowView === v}
                className={`h-6 cursor-pointer px-3 text-xs transition-colors ${
                  narrowView === v
                    ? 'bg-[var(--color-bg-elevated)] text-[var(--color-text-primary)]'
                    : 'text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]'
                }`}
                style={{ borderRadius: 'calc(var(--radius-field, 0px) * 100)' }}
                onClick={() => setNarrowView(v)}
                data-mobile-narrow-view={v}
              >
                {v === 'thread' ? 'Thread' : 'Device'}
              </button>
            ))}
          </div>
        </div>
      ) : null}
      <div className="flex min-h-0 w-full min-w-0 flex-1">
        {showChat && chatAgent && chatPath ? (
          <>
            <div
              className={`h-full min-w-0 overflow-hidden ${switched && narrowView !== 'thread' ? 'hidden' : ''}`}
              style={{ flex: switched ? '1 1 0' : `${split[0]} 1 0` }}
              data-mobile-chat=""
            >
              <DesignThreadColumn
                projectPath={chatPath}
                agentName={chatAgent}
                conversationId={chatSession ?? null}
                visible={tabVisible}
              />
            </div>
            {switched ? null : (
              <div className="relative w-px flex-shrink-0 bg-[var(--color-border)]">
                <div
                  role="separator"
                  aria-orientation="vertical"
                  aria-label="Resize Thread and device"
                  title="Drag to resize · double-click to reset"
                  className="absolute inset-y-0 -left-1 z-10 w-[9px] cursor-col-resize transition-colors hover:bg-[var(--color-accent)]/25"
                  onMouseDown={startSplitDrag}
                  onDoubleClick={resetSplit}
                  data-mobile-split=""
                />
              </div>
            )}
          </>
        ) : null}
        <div
          className={`h-full min-w-0 ${switched && narrowView !== 'device' ? 'hidden' : ''}`}
          style={{ flex: switched ? '1 1 0' : `${showChat ? split[1] : 100} 1 0` }}
        >
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
      return `The owner of this server must accept the ${licence.title} first.`
    }
    const step = lane.needs.find((n) => n.kind === 'owner-step')
    if (step && step.kind === 'owner-step') return `The owner of this server must ${step.what} first: ${step.command}`
  }
  if (s?.state === 'failed') return s.reason ? `The device stopped: ${s.reason}` : 'The device stopped with an error.'
  return 'No device is running.'
}

const foldName = (s: string): string => s.trim().toLowerCase()

/** Is the hold's `by` (the daemon's `owner` for the owner token, else the
 *  Connect login) the person looking at this tab? The owner's linked
 *  logins are the owner too. An unknown viewer holds nothing. */
export function isSimHolder(by: string, viewer: ThreadViewer | null): boolean {
  if (!viewer) return false
  const me = foldName(viewer.principal)
  const who = foldName(by)
  if (!me || !who) return false
  if (who === me) return true
  return me === 'owner' && viewer.aliases.some((a) => foldName(a) === who)
}

/** The holder as others read it: `owner` is the owner's display name where
 *  known, else "The server's owner". */
export function simHolderLabel(by: string, ownerName: string): string {
  if (foldName(by) === 'owner') return ownerName.trim() || "The server's owner"
  return by.trim() || 'A person'
}

/** How the hold banner reads, or null when nothing shows. The person
 *  holding the device sees nothing (each of their inputs holds it a few
 *  seconds so agents wait; Rosson 2026-10-30 read his own hold as someone
 *  else's). Everyone else sees who has it. */
export function holdBanner(
  hold: SimHold,
  tookOver: boolean,
  viewer: ThreadViewer | null = null,
  ownerName = '',
): string | null {
  if (hold.kind === 'free') return null
  if (isSimHolder(hold.by, viewer)) {
    // Taken over from another window of yours: this one has no Hand back.
    return hold.kind === 'takeover' && !tookOver ? 'You have taken over the device in another window' : null
  }
  const who = simHolderLabel(hold.by, ownerName)
  if (hold.kind === 'person') return `${who} is using the device`
  if (hold.kind === 'takeover' && !tookOver) return `${who} has taken over the device`
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

  // ── Device profiles (the device list, grouped by platform) ────────────
  // The list shows only the platforms the daemon reports as usable here (a
  // Linux box: Android only), so a lane it lacks is never sent.
  const platforms = useMemo(() => (status ? offeredPlatforms(status.lanes) : []), [status])
  const [devicesBy, setDevicesBy] = useState<Partial<Record<SimPlatform, { lane: SimLaneId; devices: SimDeviceProfile[] }>>>({})
  const [pickedPlatform, setPickedPlatform] = useState<SimPlatform | null>(null)
  const firstLane = status ? firstUsableLane(status.lanes) : null
  const platform: SimPlatform | null =
    session && simIsUp(session)
      ? lanePlatform(session.lane)
      : pickedPlatform && platforms.includes(pickedPlatform)
        ? pickedPlatform
        : firstLane
          ? lanePlatform(firstLane.lane)
          : null
  // No iOS lane at all on this server (a Mac's own lane that isn't usable
  // yet explains itself in the lane's detail instead).
  const iphoneNote = status && !status.lanes.some((l) => lanePlatform(l.lane) === 'ios') ? IPHONE_NEEDS_A_MAC : null
  const platformsKey = platforms.join(',')
  useEffect(() => {
    if (!workspaceRef || !visible || !canControl || !platformsKey) return
    let cancelled = false
    for (const p of platformsKey.split(',') as SimPlatform[]) {
      fetchSimDevices(scope, workspaceRef, p)
        .then((r) => {
          if (cancelled) return
          setDevicesBy((prev) => ({ ...prev, [p]: r }))
        })
        .catch((err: unknown) => console.warn(`[mobile] sim/devices (${p}):`, err))
    }
    return () => {
      cancelled = true
    }
  }, [scope, workspaceRef, visible, canControl, platformsKey])
  const devices = (platform && devicesBy[platform]?.devices) || []
  const devicesLane = (platform && devicesBy[platform]?.lane) || null
  const [picked, setPicked] = useState<string>(data.deviceType ?? '')
  const fallbackDevice = devices.find((d) => d.default)?.id ?? devices[0]?.id ?? ''
  const pickedHere = devices.some((d) => d.id === picked) ? picked : ''
  const device = session && simIsUp(session) && session.profile ? session.profile.id : pickedHere || fallbackDevice

  // ── Gestures to the daemon ────────────────────────────────────────────
  const [busy, setBusy] = useState(false)
  // A boot this tab asked for that the status doesn't show yet (the
  // loading screen covers the gap between Start and `booting`).
  const [bootPending, setBootPending] = useState(false)
  const [notice, setNotice] = useState<{ text: string; tone: MobileNoticeTone; shot?: { path: string; thumb: string | null } } | null>(
    null,
  )
  const act = useCallback(
    async (what: string, fn: () => Promise<void>) => {
      setBusy(true)
      setNotice(null)
      try {
        await fn()
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err)
        console.warn(`[mobile] ${what}:`, msg)
        setNotice({ text: msg, tone: 'error' })
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
  const [bootingId, setBootingId] = useState<string | null>(null)
  const boot = useCallback(
    (which: string, on: SimPlatform | null = platform) => {
      if (!workspaceRef) return
      remember(which)
      setBootingId(which)
      setBootPending(true)
      void act('boot', async () => {
        // The platform (android | ios), never a lane name: the daemon picks
        // the lane, and the tab only offers platforms it reported.
        try {
          const view = await bootSim(scope, workspaceRef, { device: which, ...(on ? { platform: on } : {}) })
          if (!simIsUp(view)) setBootPending(false)
        } catch (err) {
          setBootPending(false)
          throw err
        }
      })
    },
    [act, platform, remember, scope, workspaceRef],
  )
  // A row's Start: that device on its platform (a running device is
  // replaced; the daemon shuts the old one).
  const onStart = useCallback(
    (on: SimPlatform, which: string) => {
      setPickedPlatform(on)
      setPicked(which)
      boot(which, on)
    },
    [boot],
  )
  // "Show the licence": a boot that needs it makes the daemon put the
  // owner's Accept card on this workspace's Thread (beside the device).
  const onShowLicence = useCallback(
    (on: SimPlatform) => {
      const list = devicesBy[on]?.devices ?? []
      const which = list.find((d) => d.default)?.id ?? list[0]?.id
      if (!workspaceRef) return
      void act('licence', async () => {
        try {
          await bootSim(scope, workspaceRef, { ...(which ? { device: which } : {}), platform: on })
        } catch (err) {
          throw new Error(
            `${err instanceof Error ? err.message : String(err)} The licence card is on this workspace's Thread.`,
          )
        }
      })
    },
    [act, devicesBy, scope, workspaceRef],
  )
  const onCopy = useCallback((text: string) => {
    writeClipboardText(text)
      .then(() => useToastStore.getState().addToast('Copied', 'success', 2000))
      .catch((err: unknown) => useToastStore.getState().addToast(`Couldn't copy: ${String(err)}`, 'error'))
  }, [])
  const onStop = useCallback(() => {
    if (!workspaceRef) return
    void act('stop', async () => {
      await stopSim(scope, workspaceRef)
    })
  }, [act, scope, workspaceRef])
  const onScreenshot = useCallback(() => {
    if (!workspaceRef) return
    // The toast's thumbnail: the frame on screen as the shot was taken.
    const thumb = canvasThumb(canvasRef.current)
    void act('screenshot', async () => {
      const path = await screenshotSim(scope, workspaceRef)
      setNotice({ text: `Saved ${path}`, tone: 'success', shot: { path, thumb } })
    })
  }, [act, scope, workspaceRef])
  // Attach to Thread: the Thread message box beside the device takes the
  // shot as an attachment (the same event a file drop sends it).
  const stageEl = useRef<HTMLDivElement | null>(null)
  const onAttachShot = useCallback((path: string) => {
    const bar = stageEl.current?.closest('[data-mobile-pane]')?.querySelector('[data-mobile-chat] [data-compose-bar]')
    if (!bar) {
      setNotice({ text: "Open the agent's Thread beside the device to attach the screenshot.", tone: 'error' })
      return
    }
    const detail: ComposeInsertDetail = { data: path, paths: [path], uploaded: true }
    bar.dispatchEvent(new CustomEvent('k2so:compose-insert', { detail }))
    setNotice({ text: 'Attached to your Thread message.', tone: 'success' })
  }, [])
  const onCopyShot = useCallback((path: string) => {
    writeClipboardText(path)
      .then(() => setNotice({ text: `Copied ${path}`, tone: 'success' }))
      .catch((err: unknown) => setNotice({ text: `Couldn't copy: ${String(err)}`, tone: 'error' }))
  }, [])

  // ── Stream ────────────────────────────────────────────────────────────
  const [fps, setFpsState] = useState(() => defaultFps(scope.isRemote))
  const canvasRef = useRef<HTMLCanvasElement | null>(null)
  // The latency readout (⋯ → Show input latency): input → first frame.
  const meterRef = useRef<LatencyMeter | null>(null)
  if (meterRef.current === null) meterRef.current = new LatencyMeter()
  const onFrameDrawn = useCallback((seq: number) => meterRef.current?.frame(seq, performance.now()), [])
  const stream = useSimStream({
    scope,
    // The daemon's own name for the workspace, once it reported the device.
    workspace: running && session ? session.workspace : null,
    visible,
    fps,
    connector,
    canvasRef,
    hold: holdFrame,
    onFrameDrawn,
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

  // ── The stage the phone fits in (both ways, measured) ─────────────────
  const [stageRef, stageSize] = useElementSize<HTMLDivElement>()
  const [screenOnly, setScreenOnlyState] = useState(readMobileScreenOnly)
  const setScreenOnly = useCallback((on: boolean) => {
    setScreenOnlyState(on)
    writeMobileScreenOnly(on)
  }, [])
  // What this tab last set; the daemon takes appearance but doesn't report
  // it, so a new device starts from light.
  const [appearance, setAppearance] = useState<SimAppearance>('light')
  const simId = session?.sim ?? null
  useEffect(() => {
    setAppearance('light')
  }, [simId])

  // ── Pointer and keys on the device ────────────────────────────────────
  const screenRef = useRef<HTMLDivElement | null>(null)
  // The screen element as state too: the wheel / pinch listeners bind to it.
  const [screenNode, setScreenNode] = useState<HTMLDivElement | null>(null)
  const screenElRef = useCallback((el: HTMLDivElement | null) => {
    screenRef.current = el
    setScreenNode(el)
  }, [])
  const downRef = useRef<PointerSample | null>(null)
  const interactive = canControl && live
  // The computer's keyboard types into the device while its screen has focus.
  const keyboard = useSimKeyboard({ interactive, send: stream.send })
  const box = (): BoxRect | null => {
    const r = canvasRef.current?.getBoundingClientRect()
    return r ? { left: r.left, top: r.top, width: r.width, height: r.height } : null
  }
  // Live touch is on by default (0.46.7); the ⋯ switch turns it off, and
  // then a drag is one swipe on release as before.
  const [liveTouchOn, setLiveTouchState] = useState(readMobileLiveTouch)
  const setLiveTouch = useCallback((on: boolean) => {
    setLiveTouchState(on)
    writeMobileLiveTouch(on)
  }, [])
  const [latencyHud, setLatencyHudState] = useState(readMobileLatencyHud)
  const setLatencyHud = useCallback((on: boolean) => {
    setLatencyHudState(on)
    writeMobileLatencyHud(on)
    meterRef.current?.reset()
  }, [])
  const [latency, setLatency] = useState<LatencyReadout | null>(null)
  useEffect(() => {
    if (!latencyHud) {
      setLatency(null)
      return
    }
    const tick = (): void => setLatency(meterRef.current?.readout(performance.now()) ?? null)
    tick()
    const id = window.setInterval(tick, 500)
    return () => window.clearInterval(id)
  }, [latencyHud])
  // Every pointer input goes through here, so the readout sees it; `at`
  // is the pointer event's time when the send waited for a frame.
  const sendPointer = (m: SimInput, at: number = performance.now()): void => {
    if (m.t === 'touch' || m.t === 'tap' || m.t === 'swipe') meterRef.current?.input(at)
    stream.send(m)
  }
  const touchRef = useRef<LiveTouch | null>(null)
  // The drag's recent samples, for the flick velocity at release.
  const samplesRef = useRef<PointerSample[]>([])
  // ⌥-drag pinch: 'on' (two fingers down), 'refused' (no multi-touch).
  const altRef = useRef<'on' | 'refused' | null>(null)
  const holdTimerRef = useRef<number | null>(null)
  const pendingMoveRef = useRef<PointerSample | null>(null)
  const moveFrameRef = useRef<number | null>(null)
  // The touch dot: where the person's finger is, fading on release.
  const [dot, setDot] = useState<{ x: number; y: number; down: boolean } | null>(null)
  const dotTimerRef = useRef<number | null>(null)
  const sample = (e: { clientX: number; clientY: number }): PointerSample => ({ clientX: e.clientX, clientY: e.clientY, at: performance.now() })
  const placeDot = (e: { clientX: number; clientY: number }, down: boolean): void => {
    const r = screenRef.current?.getBoundingClientRect()
    if (!r) return
    if (dotTimerRef.current !== null) window.clearTimeout(dotTimerRef.current)
    dotTimerRef.current = null
    setDot({ x: e.clientX - r.left, y: e.clientY - r.top, down })
    if (!down) dotTimerRef.current = window.setTimeout(() => setDot(null), 200)
  }
  const clearHold = (): void => {
    if (holdTimerRef.current !== null) window.clearTimeout(holdTimerRef.current)
    holdTimerRef.current = null
  }
  const flushMove = (): void => {
    if (moveFrameRef.current !== null) cancelAnimationFrame(moveFrameRef.current)
    moveFrameRef.current = null
    const at = pendingMoveRef.current
    pendingMoveRef.current = null
    const t = touchRef.current
    const b = box()
    if (!at || !t || !b || !screen) return
    if (altRef.current === 'on') {
      sendPointer(altDragFingers('move', at.clientX, at.clientY, b, screen), at.at)
      return
    }
    const r = liveTouchMove(t, at, b, screen)
    touchRef.current = r.touch
    if (r.touch.started) clearHold()
    for (const m of r.send) sendPointer(m, at.at)
  }
  useEffect(
    () => () => {
      if (holdTimerRef.current !== null) window.clearTimeout(holdTimerRef.current)
      if (dotTimerRef.current !== null) window.clearTimeout(dotTimerRef.current)
      if (moveFrameRef.current !== null) cancelAnimationFrame(moveFrameRef.current)
    },
    [],
  )
  const onPointerDown = (e: React.PointerEvent): void => {
    if (!interactive || e.button !== 0) return
    screenRef.current?.focus()
    endWheelRef.current?.()
    const down = sample(e)
    downRef.current = down
    samplesRef.current = [down]
    placeDot(e, true)
    if (e.altKey) {
      // ⌥-drag: pinch with the second finger mirrored around the centre.
      const b = box()
      if (!stream.inputCaps?.multiTouch) {
        altRef.current = 'refused'
        setNotice({ text: PINCH_UNAVAILABLE, tone: 'info' })
      } else if (b && screen) {
        altRef.current = 'on'
        touchRef.current = { down, started: true }
        sendPointer(altDragFingers('down', e.clientX, e.clientY, b, screen))
      }
    } else if (liveTouchOn) {
      touchRef.current = liveTouchStart(down)
      clearHold()
      holdTimerRef.current = window.setTimeout(() => {
        holdTimerRef.current = null
        const t = touchRef.current
        const b = box()
        if (!t || !b || !screen) return
        const r = liveTouchHold(t, b, screen)
        touchRef.current = r.touch
        for (const m of r.send) sendPointer(m)
      }, TAP_MAX_MS)
    }
    try {
      ;(e.target as Element).setPointerCapture?.(e.pointerId)
    } catch {
      // jsdom / a pointer that already ended.
    }
  }
  const onPointerMove = (e: React.PointerEvent): void => {
    if (!interactive || !downRef.current) return
    placeDot(e, true)
    if (altRef.current === 'refused') return
    if (!touchRef.current) return
    const at = sample(e)
    samplesRef.current = trimSamples([...samplesRef.current, at], at.at)
    pendingMoveRef.current = at
    if (moveFrameRef.current === null) moveFrameRef.current = requestAnimationFrame(flushMove)
  }
  const onPointerUp = (e: React.PointerEvent): void => {
    const down = downRef.current
    downRef.current = null
    if (down) placeDot(e, false)
    clearHold()
    if (touchRef.current) flushMove()
    const track = touchRef.current
    touchRef.current = null
    const alt = altRef.current
    altRef.current = null
    if (!interactive || !down || !screen) return
    const b = box()
    if (!b) return
    if (alt === 'refused') return
    if (alt === 'on') {
      sendPointer(altDragFingers('up', e.clientX, e.clientY, b, screen))
      return
    }
    const up = sample(e)
    if (track) {
      // A flick carries its release velocity; the daemon replays its tail.
      const v = track.started ? releaseVelocity(samplesRef.current, up.at, b, screen) : null
      for (const m of liveTouchEnd(track, up, b, screen)) sendPointer(withVelocity(m, v))
      return
    }
    sendPointer(gestureFor(down, up, b, screen))
  }
  const onPointerCancel = (e: React.PointerEvent): void => {
    const down = downRef.current
    downRef.current = null
    clearHold()
    pendingMoveRef.current = null
    const t = touchRef.current
    touchRef.current = null
    const alt = altRef.current
    altRef.current = null
    if (down) placeDot(e, false)
    // A drag the device already follows must lift, or the finger sticks.
    const b = box()
    if (alt === 'on' && b && screen) {
      sendPointer(altDragFingers('up', e.clientX, e.clientY, b, screen))
      return
    }
    if (t?.started && b && screen) for (const m of liveTouchEnd(t, sample(e), b, screen)) sendPointer(m)
  }

  // ── Trackpad / mouse wheel and pinch (sim-gestures) ───────────────────
  // Native listeners: React's wheel handler is passive, and WebKit's
  // gesture* events have no React prop. The handlers read the latest
  // render through a ref, so they bind once per screen element.
  const gestureLive = useRef({ interactive, screen, multiTouch: false, box, sendPointer, setNotice })
  gestureLive.current = { interactive, screen, multiTouch: stream.inputCaps?.multiTouch === true, box, sendPointer, setNotice }
  const endWheelRef = useRef<(() => void) | null>(null)
  useEffect(() => {
    const el = screenNode
    if (!el) return
    let wheel: WheelDrag | null = null
    const pendingDelta = { dx: 0, dy: 0 }
    let wheelFrame: number | null = null
    let wheelIdle: number | null = null
    let pinch: Pinch | null = null
    let pinchRefused = false
    let pinchFrame: number | null = null
    let pinchIdle: number | null = null
    let gestureScale: number | null = null
    const flushWheel = (): void => {
      if (wheelFrame !== null) cancelAnimationFrame(wheelFrame)
      wheelFrame = null
      const L = gestureLive.current
      if (!wheel || !L.screen || (pendingDelta.dx === 0 && pendingDelta.dy === 0)) return
      const r = wheelMove(wheel, pendingDelta, L.screen)
      pendingDelta.dx = 0
      pendingDelta.dy = 0
      wheel = r.drag
      for (const m of r.send) L.sendPointer(m)
    }
    const endWheel = (): void => {
      if (wheelIdle !== null) window.clearTimeout(wheelIdle)
      wheelIdle = null
      if (!wheel) return
      flushWheel()
      for (const m of wheelEnd(wheel)) gestureLive.current.sendPointer(m)
      wheel = null
    }
    endWheelRef.current = endWheel
    // Sends the queued scale, if a change is waiting for its frame.
    const flushPinch = (): void => {
      const due = pinchFrame !== null
      if (pinchFrame !== null) cancelAnimationFrame(pinchFrame)
      pinchFrame = null
      const L = gestureLive.current
      if (!due || !pinch || !L.screen) return
      const [a, b] = pinchPoints(pinch, L.screen)
      L.sendPointer(twoFingers('move', a, b))
    }
    // Returns false when the device can't pinch (said once per gesture).
    const startPinch = (clientX: number, clientY: number): boolean => {
      const L = gestureLive.current
      const b = L.box()
      if (!L.screen || !b) return false
      endWheel()
      if (!L.multiTouch) {
        if (!pinchRefused) L.setNotice({ text: PINCH_UNAVAILABLE, tone: 'info' })
        pinchRefused = true
        return false
      }
      const cx = Number.isFinite(clientX) ? clientX : b.left + b.width / 2
      const cy = Number.isFinite(clientY) ? clientY : b.top + b.height / 2
      pinch = pinchBegin(canvasPointToDevice(cx, cy, b, L.screen), L.screen)
      const [p1, p2] = pinchPoints(pinch, L.screen)
      L.sendPointer(twoFingers('down', p1, p2))
      return true
    }
    const endPinch = (): void => {
      if (pinchIdle !== null) window.clearTimeout(pinchIdle)
      pinchIdle = null
      pinchRefused = false
      gestureScale = null
      if (!pinch) return
      flushPinch()
      const L = gestureLive.current
      if (L.screen) {
        const [a, b] = pinchPoints(pinch, L.screen)
        L.sendPointer(twoFingers('up', a, b))
      }
      pinch = null
    }
    const queuePinch = (): void => {
      if (pinchFrame === null) pinchFrame = requestAnimationFrame(flushPinch)
    }
    const onWheel = (e: WheelEvent): void => {
      const L = gestureLive.current
      if (!L.interactive || !L.screen) return
      e.preventDefault()
      e.stopPropagation()
      const b = L.box()
      // A pointer drag or a WebKit pinch owns the device's fingers now.
      if (!b || downRef.current || gestureScale !== null) return
      if (e.ctrlKey) {
        // Chrome-style trackpad pinch (and Ctrl+wheel on a mouse).
        if (!pinch && !pinchRefused) startPinch(e.clientX, e.clientY)
        if (pinchIdle !== null) window.clearTimeout(pinchIdle)
        pinchIdle = window.setTimeout(endPinch, PINCH_IDLE_MS)
        if (!pinch) return
        pinch = pinchWheelScale(pinch, wheelDeltaPx(e, b).dy)
        queuePinch()
        return
      }
      if (pinch || pinchRefused) endPinch()
      const d = cssToPoints(wheelDeltaPx(e, b), b, L.screen)
      if (!wheel) {
        const r = wheelBegin(canvasPointToDevice(e.clientX, e.clientY, b, L.screen), L.screen)
        wheel = r.drag
        for (const m of r.send) L.sendPointer(m)
      }
      pendingDelta.dx += d.dx
      pendingDelta.dy += d.dy
      if (wheelFrame === null) wheelFrame = requestAnimationFrame(flushWheel)
      if (wheelIdle !== null) window.clearTimeout(wheelIdle)
      wheelIdle = window.setTimeout(endWheel, WHEEL_IDLE_MS)
    }
    // WebKit (the Mac app): trackpad pinch as gesture* with a cumulative scale.
    const scaleOf = (e: Event): number | null => {
      const v = (e as Event & { scale?: unknown }).scale
      return typeof v === 'number' && Number.isFinite(v) && v > 0 ? v : null
    }
    const pointOf = (e: Event): { x: number; y: number } => {
      const g = e as Event & { clientX?: unknown; clientY?: unknown }
      return { x: typeof g.clientX === 'number' ? g.clientX : NaN, y: typeof g.clientY === 'number' ? g.clientY : NaN }
    }
    const onGestureStart = (e: Event): void => {
      if (!gestureLive.current.interactive) return
      e.preventDefault()
      e.stopPropagation()
      endPinch()
      gestureScale = scaleOf(e) ?? 1
      const p = pointOf(e)
      startPinch(p.x, p.y)
    }
    const onGestureChange = (e: Event): void => {
      if (gestureScale === null) return
      e.preventDefault()
      e.stopPropagation()
      const next = scaleOf(e)
      if (!pinch || next === null) return
      pinch = { ...pinch, scale: next }
      queuePinch()
    }
    const onGestureEnd = (e: Event): void => {
      if (gestureScale === null) return
      e.preventDefault()
      e.stopPropagation()
      endPinch()
    }
    el.addEventListener('wheel', onWheel, { passive: false })
    el.addEventListener('gesturestart', onGestureStart, { passive: false } as AddEventListenerOptions)
    el.addEventListener('gesturechange', onGestureChange, { passive: false } as AddEventListenerOptions)
    el.addEventListener('gestureend', onGestureEnd, { passive: false } as AddEventListenerOptions)
    return () => {
      el.removeEventListener('wheel', onWheel)
      el.removeEventListener('gesturestart', onGestureStart)
      el.removeEventListener('gesturechange', onGestureChange)
      el.removeEventListener('gestureend', onGestureEnd)
      // Lift anything still down, so no finger sticks on the device.
      endWheel()
      endPinch()
      endWheelRef.current = null
    }
  }, [screenNode])

  // ── Loading screen (installing → booting → first frame) ───────────────
  const sessionState = session?.state ?? null
  useEffect(() => {
    if (sessionState === 'installing' || sessionState === 'booting' || sessionState === 'running') setBootPending(false)
  }, [sessionState])
  const streamClosed = stream.conn === 'error' || stream.conn === 'closed'
  // Once this device has drawn, a reconnect (a hidden tab shown again, a
  // network blip) keeps the last frame, dimmed, instead of a blank or the
  // boot screen; "Reconnecting…" shows only after 2 s.
  const [hadFrame, setHadFrame] = useState(false)
  useEffect(() => {
    if (stream.hasFrame) setHadFrame(true)
  }, [stream.hasFrame])
  useEffect(() => {
    if (!running) setHadFrame(false)
  }, [running, simId])
  const reconnecting = running && hadFrame && !stream.hasFrame
  const [slowReconnect, setSlowReconnect] = useState(false)
  useEffect(() => {
    if (!reconnecting) {
      setSlowReconnect(false)
      return
    }
    const t = window.setTimeout(() => setSlowReconnect(true), RECONNECT_NOTE_AFTER_MS)
    return () => window.clearTimeout(t)
  }, [reconnecting])
  const bootPhase: MobileBootPhase | null =
    sessionState === 'installing'
      ? 'installing'
      : sessionState === 'booting'
        ? 'booting'
        : running && !stream.hasFrame && !streamClosed && !hadFrame
          ? 'connecting'
          : bootPending
            ? 'booting'
            : null
  const bootFailed = sessionState === 'failed' || (running && !stream.hasFrame && streamClosed && !hadFrame)
  const [bootRun, setBootRun] = useState<{
    startedAt: number
    phase: MobileBootPhase
    withInstall: boolean
    failed: boolean
  } | null>(null)
  useEffect(() => {
    setBootRun((prev) => {
      if (bootPhase) {
        if (!prev || prev.failed) {
          return { startedAt: Date.now(), phase: bootPhase, withInstall: bootPhase === 'installing', failed: false }
        }
        const withInstall = prev.withInstall || bootPhase === 'installing'
        return prev.phase === bootPhase && prev.withInstall === withInstall ? prev : { ...prev, phase: bootPhase, withInstall }
      }
      // A start that stopped keeps its screen (with the error); anything
      // else (stopped, first frame drawn) ends it.
      if (prev && bootFailed) return prev.failed ? prev : { ...prev, failed: true }
      return null
    })
  }, [bootPhase, bootFailed])

  const blocked = deviceNotice(status, statusError)
  const laneReady = !!status && status.lanes.some((l) => l.usable && l.ready)
  // Stopped (or failed) on a ready lane: Start is offered (the boot
  // screen's Try again uses it).
  const canStart = canControl && laneReady && !!device && !simIsUp(session)
  const pickerGroups: DevicePickerGroup[] = platforms.map((p) => {
    const lanes = status?.lanes.filter((l) => l.usable && lanePlatform(l.lane) === p) ?? []
    const readyLane = lanes.find((l) => l.ready)
    return {
      platform: p,
      devices: devicesBy[p]?.devices ?? [],
      ready: !!readyLane,
      needs: (readyLane ?? lanes[0])?.needs ?? [],
      loading: !devicesBy[p],
    }
  })
  const pickerProps = {
    groups: pickerGroups,
    up: session && simIsUp(session) && session.profile ? { id: session.profile.id, state: session.state } : null,
    selected: device,
    pending: bootPending ? bootingId : null,
    canControl,
    busy,
    onStart,
    onShowLicence,
    onCopy,
  }
  const pickedProfile = devices.find((d) => d.id === device)
  // The phone the frame draws: the running device, else (while it starts)
  // the one being started, so the outline doesn't jump at the first frame.
  const frameProfile = session?.profile ?? pickedProfile ?? null
  const frameScreen: ScreenPt =
    screen ?? (frameProfile ? { widthPt: frameProfile.widthPt, heightPt: frameProfile.heightPt } : { widthPt: 393, heightPt: 852 })
  const shape = resolveDeviceShape(session?.lane ?? devicesLane, frameProfile?.name ?? '', frameScreen)
  const showRail = canControl && running
  const overlayBox = renderOverlay && screen && live ? box() : null
  // Who is looking, so a person's own hold doesn't read as someone else's.
  // The owner's display name is this computer's setting (unknown for a
  // remote server's owner).
  const viewer = useThreadViewer(scope)
  const localOwnerName = useSettingsStore((st) => st.ownerDisplayName)
  const banner = holdBanner(hold, tookOver, viewer, scope.isRemote ? '' : localOwnerName)
  const [dismissedInputError, setDismissedInputError] = useState<string | null>(null)
  const inputNotice = stream.inputError && stream.inputError !== dismissedInputError ? stream.inputError : null
  // Room for the rail from the first boot step, so the phone doesn't shift
  // when the rail appears.
  const frame = frameLayout(shape, frameScreen, stageSize, {
    rightInset: canControl && (running || bootPhase !== null) ? RAIL_INSET_PX : 0,
    screenOnly,
  })
  const bootDevice = session?.profile?.name ?? devices.find((d) => d.id === device)?.name ?? (device || 'the device')
  const bootScreen = bootPhase ? (
    <MobileBootScreen
      deviceName={bootDevice}
      phase={bootPhase}
      startedAt={bootRun && !bootRun.failed ? bootRun.startedAt : Date.now()}
      withInstall={bootRun?.withInstall ?? false}
    />
  ) : bootRun?.failed && bootFailed ? (
    <MobileBootScreen
      deviceName={bootDevice}
      phase={bootRun.phase}
      startedAt={bootRun.startedAt}
      withInstall={bootRun.withInstall}
      error={
        sessionState === 'failed'
          ? blocked
          : stream.reason ?? 'The device stream closed. Reopen the tab to try again.'
      }
      onRetry={sessionState === 'failed' && canStart ? () => boot(device) : undefined}
      retryDisabled={busy}
    />
  ) : null

  return (
    <div className="flex h-full min-h-0 w-full flex-col" data-testid="mobile-device">
      <MobileToolbar
        session={session}
        picked={pickedProfile ? { name: pickedProfile.name, os: pickedProfile.os } : null}
        picker={pickerProps}
        canControl={canControl}
        live={live}
        tookOver={tookOver}
        onTakeOver={() => {
          stream.send({ t: 'take-over' })
          setTookOver(true)
        }}
        onHandBack={() => {
          stream.send({ t: 'hand-back' })
          setTookOver(false)
        }}
      />
      <div
        ref={(el) => {
          stageRef.current = el
          stageEl.current = el
        }}
        className="relative flex min-h-0 flex-1 items-center justify-center overflow-hidden bg-[radial-gradient(ellipse_at_50%_40%,var(--color-bg)_0%,var(--color-bg-stripe)_75%)] px-6 pb-8 pt-10"
        data-mobile-stage=""
      >
        {latency && running ? (
          <span
            className="pointer-events-none absolute left-3 top-3 z-10 rounded bg-black/70 px-2 py-0.5 font-mono text-[10px] text-white/85"
            data-mobile-latency=""
          >
            {latencyText(latency)}
          </span>
        ) : null}
        {running ? (
          <DeviceFrame shape={shape} layout={frame} screenOnly={screenOnly} dimmed={reconnecting}>
            <div
              ref={screenElRef}
              tabIndex={-1}
              role="application"
              aria-label={`${session?.profile?.name ?? 'Device'} screen${interactive ? '' : ' (watching)'}`}
              className="relative h-full w-full overflow-hidden outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-[var(--color-accent)]"
              style={{ cursor: interactive ? 'pointer' : 'default', touchAction: 'none' }}
              onPointerDown={onPointerDown}
              onPointerMove={onPointerMove}
              onPointerUp={onPointerUp}
              onPointerCancel={onPointerCancel}
              onFocus={keyboard.onScreenFocus}
              onMouseDown={keyboard.onScreenMouseDown}
              data-mobile-screen={interactive ? 'interactive' : 'watching'}
            >
              <canvas
                ref={canvasRef}
                className="block h-full w-full transition-opacity duration-150"
                style={{ opacity: stream.hasFrame || hadFrame ? 1 : 0 }}
                data-mobile-canvas=""
              />
              {bootScreen ? (
                <div className="absolute inset-0 bg-black" data-mobile-boot-overlay="">
                  {bootScreen}
                </div>
              ) : !stream.hasFrame && (streamClosed || slowReconnect || !hadFrame) ? (
                <div className="pointer-events-none absolute inset-x-0 top-[12%] flex justify-center px-3">
                  <span
                    className="max-w-full rounded-full bg-black/70 px-3 py-1 text-center text-[11px] text-white/85 backdrop-blur"
                    data-mobile-stream-note=""
                  >
                    {streamClosed
                      ? stream.reason ?? 'The device stream closed. Reopen the tab to try again.'
                      : hadFrame
                        ? 'Reconnecting…'
                        : 'Connecting to the device…'}
                  </span>
                </div>
              ) : null}
              {overlayBox && screen && renderOverlay ? renderOverlay({ box: overlayBox, screen }) : null}
              {dot && interactive ? (
                <span
                  aria-hidden
                  className="pointer-events-none absolute h-7 w-7 rounded-full border border-white/70 bg-white/35 shadow-[0_0_0_1px_rgba(0,0,0,.25)] transition-[opacity,transform] duration-200 ease-out"
                  style={{
                    left: dot.x,
                    top: dot.y,
                    opacity: dot.down ? 1 : 0,
                    transform: `translate(-50%, -50%) scale(${dot.down ? 1 : 1.5})`,
                  }}
                  data-mobile-touch-dot={dot.down ? 'down' : 'up'}
                />
              ) : null}
              {keyboard.overlay}
            </div>
          </DeviceFrame>
        ) : bootScreen ? (
          <DeviceFrame shape={shape} layout={frame} screenOnly={screenOnly}>
            <div className="absolute inset-0" data-mobile-boot-overlay="">
              {bootScreen}
            </div>
          </DeviceFrame>
        ) : null}
        {running ? keyboard.hint : null}
        {showRail ? (
          <div className="absolute right-3 top-1/2 -translate-y-1/2">
            <MobileControlRail
              lane={session?.lane ?? null}
              live={live}
              busy={busy}
              appearance={appearance}
              screenOnly={screenOnly}
              fps={scope.isRemote ? stream.fps ?? fps : null}
              deviceName={session?.profile?.name ?? 'Device'}
              deviceOs={session?.profile?.os}
              isolation={stream.isolation}
              onHome={() => stream.send({ t: 'button', button: 'home' })}
              onBack={() => stream.send({ t: 'button', button: 'back' })}
              onRecents={() => stream.send({ t: 'button', button: 'app-switch' })}
              onRotate={() => screen && stream.send({ t: 'rotate', orientation: nextOrientation(screen) })}
              onAppearance={(next) => {
                stream.send({ t: 'appearance', appearance: next })
                setAppearance(next)
              }}
              onScreenshot={onScreenshot}
              onStop={onStop}
              onOpenUrl={(url) => stream.send({ t: 'open-url', url })}
              onScreenOnly={setScreenOnly}
              liveTouch={liveTouchOn}
              onLiveTouch={setLiveTouch}
              latencyHud={latencyHud}
              onLatencyHud={setLatencyHud}
              onFps={onFps}
              extra={toolbarExtra}
              keyboardHelp={<SimKeyboardHelp />}
            />
          </div>
        ) : null}
        {running ? null : bootScreen ? null : (
          <div className="flex max-h-full w-full max-w-sm flex-col items-center gap-4 overflow-y-auto" data-mobile-empty="">
            <div className="text-center text-xs text-[var(--color-text-muted)]" data-mobile-blocked="">
              {blocked}
            </div>
            {canControl && pickerGroups.length > 0 ? <DevicePicker {...pickerProps} /> : null}
            {iphoneNote && canControl ? (
              <div className="text-center text-[11px] text-[var(--color-text-muted)]" data-mobile-iphone-note="">
                {iphoneNote}
              </div>
            ) : null}
          </div>
        )}
        {banner ? (
          <div className="pointer-events-none absolute left-1/2 top-2 z-10 -translate-x-1/2">
            <MobileHoldPill text={banner} kind={hold.kind} />
          </div>
        ) : null}
        {notice || inputNotice ? (
          <div className="pointer-events-none absolute inset-x-3 bottom-3 z-20 flex flex-col items-center gap-2">
            {notice ? (
              <MobileNotice
                tone={notice.tone}
                text={notice.text}
                thumb={notice.shot?.thumb}
                actions={
                  notice.shot
                    ? [
                        { label: 'Attach to Thread', onClick: () => onAttachShot(notice.shot!.path), testId: 'attach' },
                        { label: 'Copy path', onClick: () => onCopyShot(notice.shot!.path), testId: 'copy' },
                      ]
                    : []
                }
                onDismiss={() => setNotice(null)}
              />
            ) : null}
            {inputNotice ? (
              <MobileNotice tone="error" text={inputNotice} onDismiss={() => setDismissedInputError(inputNotice)} />
            ) : null}
          </div>
        ) : null}
      </div>
    </div>
  )
}
