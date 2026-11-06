// The floating call tile (Rosson 2026-11-06, after K2 Companion's phone
// card): while video is on in the call (anyone's camera or screen share) and
// the call view is closed, a small video tile floats in a corner of the
// window, with the call's controls in their own small strip just under it.
// The two drag as one, snap to a corner on the release velocity, and the
// corner is remembered per window. With no video there's no tile: the
// controls sit in the top bar beside the headset.
//
// Ported from K2 Companion's `callTile.tsx` (the card, the strip, the time)
// with its motion (`useCornerDrag`, `mini-snap.ts`). What differs: the
// picture is K2's own video tile (a canvas a sink draws on, not a native
// layer), it follows the active speaker with the same 1.5 s hold, the strip
// is separate from the picture, and the look is K2's (Surface panels, the
// theme's tokens).
//
// A press that never drags is a click: a button's is the button's; anywhere
// else on the tile opens the call view over the current page.

import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react'
import { createPortal } from 'react-dom'
import { useStore } from 'zustand'
import { Surface } from '@/components/ui/Surface'
import { measureElement, overlayClaims, rectsOverlap, subscribeOverlayClaims, useBrowserOverlay } from '@/lib/browser-overlay'
import { CALL_TILE_Z } from '@/lib/overlay-layers'
import { cornerSpots, freeArea, type Bounds, type Corner, type Rect, type Vec } from '@/lib/voice/mini-snap'
import { usePanelsStore } from '@/stores/panels'
import { DRAWER_ANIM_MS } from '@/components/Layout/drawer-motion'
import { holdLeft, nextSpeaker, NO_SPEAKER, speakingIds, tilePicture, videoLive, type SpeakerHold } from '@/lib/voice/call-tile-picture'
import { videoTiles, type CallPerson, type VideoTile as Tile } from '@/lib/voice/video-labels'
import { getWindowLabel } from '@/lib/window-chrome'
import { useCallScreen } from '@/stores/call-screen'
import type { VideoController } from '@/stores/video-core'
import type { VoiceCall } from '@/stores/voice-core'
import CallControls, { type CallControlsProps } from './CallControls'
import { ScreenPicker, VideoNoticeLine } from './VideoControls'
import { callDuration } from './voice-labels'
import { reduceMotion, useCornerDrag, useFadeIn } from './useCornerDrag'
import VideoTile from './VideoTile'
import { TOPBAR_HEIGHT } from '../../../shared/constants'

/** The tile's width (px); the picture is 16:9 inside it. */
export const CALL_TILE_WIDTH = 240
/** The band under the picture that carries the call's time (px). */
export const DURATION_BAND = 22
/** Clear of the window's edges, the top bar and what the tile avoids (px). */
export const CALL_TILE_INSETS = { top: TOPBAR_HEIGHT + 12, bottom: 12, side: 12, gap: 8 }
/** What the tile never covers: composers (the Thread's and the terminal's
 *  compose bars) and anything marked `data-call-tile-avoid`. */
export const CALL_TILE_AVOID = '[data-compose-bar], [data-testid="agent-session-thread-compose-slot"], [data-call-tile-avoid]'
/** Composers come and go with tabs: the corner's spot is checked this often. */
export const CALL_TILE_RELAYOUT_MS = 1_000

/** The call's video tiles, as the call view draws them. */
export function useCallTiles(video: VideoController, people: readonly CallPerson[]): Tile[] {
  const tracks = useStore(video.store, (s) => s.tracks)
  const local = useStore(video.store, (s) => s.local)
  return useMemo(() => videoTiles(tracks, local, people), [tracks, local, people])
}

/** Video is on in the call (the tile shows, unless the call view is open). */
export function useCallVideoLive(video: VideoController, people: readonly CallPerson[]): boolean {
  const ready = useStore(video.store, (s) => s.availability === 'ready')
  const tiles = useCallTiles(video, people)
  return ready && videoLive(tiles)
}

/** The active speaker, held 1.5 s after they stop before someone else who is
 *  talking takes over (Sew's active-speaker hold). */
export function useActiveSpeaker(people: readonly CallPerson[]): string | null {
  const key = speakingIds(people).join('\n')
  const [hold, setHold] = useState<SpeakerHold>(NO_SPEAKER)
  const [recheck, setRecheck] = useState(0)
  useEffect(() => {
    const speaking = key ? key.split('\n') : []
    setHold((prev) => nextSpeaker(prev, speaking, Date.now()))
  }, [key, recheck])
  // Someone else is talking while the hold runs: look again when it ends.
  useEffect(() => {
    const speaking = key ? key.split('\n') : []
    if (speaking.length === 0 || hold.id === null || speaking.includes(hold.id)) return
    const id = setTimeout(() => setRecheck((n) => n + 1), holdLeft(hold, Date.now()))
    return () => clearTimeout(id)
  }, [hold, key])
  return hold.id
}

/** The call's time, ticking on the call's own second while mounted (Companion's
 *  `useCallDuration`). Null until the call connects. */
export function useCallDuration(startedAt: number | null): string | null {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (startedAt === null) return
    let timer: ReturnType<typeof setTimeout> | null = null
    const tick = (): void => {
      const t = Date.now()
      setNow(t)
      timer = setTimeout(tick, 1000 - ((((t - startedAt) % 1000) + 1000) % 1000))
    }
    tick()
    return () => {
      if (timer) clearTimeout(timer)
    }
  }, [startedAt])
  return startedAt === null ? null : callDuration(startedAt, now)
}

function rectOf(el: Element): Rect {
  const r = el.getBoundingClientRect()
  return { left: r.left, top: r.top, width: r.width, height: r.height }
}

function visibleRects(selector: string, except: Element | null): Rect[] {
  const out: Rect[] = []
  for (const el of document.querySelectorAll(selector)) {
    if (except?.contains(el)) continue
    const r = rectOf(el)
    if (r.width > 0 && r.height > 0) out.push(r)
  }
  return out
}

/** The room's content column (Layout, FocusLayout). */
export const CALL_TILE_AREA = '[data-call-tile-area]'
/** The panes' tab strips (TabBar). */
export const CALL_TILE_TABS = '[data-room-tab-bar]'
/** The drawers, as AnimatedDrawer marks them (their width is the panels
 *  store's, as drawn). */
export const CALL_TILE_DRAWERS = '[data-drawer-side]'

/** Measure the free content area now: under the tab strip, beside the open
 *  drawers (freeArea). Pages with no content column use the window. */
export function measureCallTileArea(): Bounds {
  const content = visibleRects(CALL_TILE_AREA, null)[0] ?? null
  const drawers: { side: 'left' | 'right'; rect: Rect }[] = []
  for (const el of document.querySelectorAll(CALL_TILE_DRAWERS)) {
    const side = (el as HTMLElement).dataset.drawerSide
    if (side === 'left' || side === 'right') drawers.push({ side, rect: rectOf(el) })
  }
  return freeArea({ width: window.innerWidth, height: window.innerHeight }, { content, drawers, tabs: visibleRects(CALL_TILE_TABS, null) })
}

/** A drawer opened, closed or was resized (the panels store): look again once
 *  its slide has ended, so the tile moves out of its way, and back. */
function useRelayoutOnDrawers(relayout: () => void): void {
  const left = usePanelsStore((s) => (s.leftPanelOpen ? s.leftPanelWidth : 0))
  const right = usePanelsStore((s) => (s.rightPanelOpen ? s.rightPanelWidth : 0))
  const first = useRef(true)
  const again = useRef(relayout)
  again.current = relayout
  useEffect(() => {
    if (first.current) {
      first.current = false
      return
    }
    const id = setTimeout(() => again.current(), DRAWER_ANIM_MS + 30)
    return () => clearTimeout(id)
  }, [left, right])
}

/** A menu or popover over the tile: the tile steps aside until it closes. */
function useOverlapped(ref: React.RefObject<HTMLElement | null>): boolean {
  const claims = useSyncExternalStore(subscribeOverlayClaims, overlayClaims, overlayClaims)
  const [overlapped, setOverlapped] = useState(false)
  useEffect(() => {
    const el = ref.current
    const mine = measureElement(el)
    const hit =
      mine !== null &&
      claims.some(
        (c) => (c.kind === 'menu' || c.kind === 'popover') && c.area !== 'fullscreen' && !(c.element && el?.contains(c.element)) && rectsOverlap(c.area, mine),
      )
    setOverlapped(hit)
  }, [claims, ref])
  return overlapped
}

export interface CallTileProps {
  call: VoiceCall
  video: VideoController
  actions: CallControlsProps['actions']
}

function Card({ call, video, actions }: CallTileProps): React.JSX.Element {
  const ref = useRef<HTMLDivElement>(null)
  const people = call.participants
  const tiles = useCallTiles(video, people)
  const speaker = useActiveSpeaker(people)
  const picture = tilePicture(tiles, speaker)
  const time = useCallDuration(call.connectedAt)
  const windowLabel = useMemo(getWindowLabel, [])
  const spots = (): Record<Corner, Vec> => {
    const el = ref.current
    const size = { width: el?.offsetWidth || CALL_TILE_WIDTH, height: el?.offsetHeight || 0 }
    return cornerSpots(
      { width: window.innerWidth, height: window.innerHeight },
      size,
      CALL_TILE_INSETS,
      visibleRects(CALL_TILE_AVOID, el),
      measureCallTileArea(),
    )
  }
  const { corner, held, moving, relayout, handlers } = useCornerDrag(ref, {
    windowLabel,
    spots,
    // A button's click is the button's; anywhere else opens the call view.
    onTap: (target) => {
      const hit = target instanceof Element ? target.closest('button') : null
      if (!hit || !ref.current?.contains(hit)) useCallScreen.getState().ask()
    },
  })
  useFadeIn(ref)
  useRelayoutOnDrawers(relayout)
  const relayoutRef = useRef(relayout)
  relayoutRef.current = relayout
  useEffect(() => {
    const again = (): void => relayoutRef.current()
    window.addEventListener('resize', again)
    const id = setInterval(again, CALL_TILE_RELAYOUT_MS)
    return () => {
      window.removeEventListener('resize', again)
      clearInterval(id)
    }
  }, [])
  // Over a live Browser page the tile's box is claimed (the page gives way);
  // while it moves, the whole window is (useCornerDrag's drag claim).
  useBrowserOverlay({ active: true, ref, kind: 'popover', watch: `${corner}:${moving}` })
  const overlapped = useOverlapped(ref)

  const label = `Video call in ${call.roomName}. Click the video to open the call.`
  return (
    <div
      ref={ref}
      role="group"
      aria-label={label}
      data-testid="call-tile"
      data-corner={corner}
      data-held={held ? 'true' : 'false'}
      data-picture={picture?.key ?? 'none'}
      data-overlapped={overlapped ? 'true' : 'false'}
      data-reduced-motion={reduceMotion() ? 'true' : 'false'}
      className="fixed flex flex-col items-center gap-1.5 select-none"
      style={{
        left: 0,
        top: 0,
        width: CALL_TILE_WIDTH,
        zIndex: CALL_TILE_Z,
        // The gap between the picture and the strip lets clicks through.
        pointerEvents: 'none',
        touchAction: 'none',
        willChange: 'transform',
        visibility: overlapped && !held ? 'hidden' : undefined,
        WebkitAppRegion: 'no-drag',
      } as React.CSSProperties}
      {...handlers}
    >
      <Surface
        role2="solid"
        elevation={held ? 5 : 3}
        data-testid="call-tile-picture"
        className="pointer-events-auto w-full cursor-pointer overflow-hidden"
        role="button"
        tabIndex={0}
        aria-label={`Open the call in ${call.roomName}`}
        onKeyDown={(e) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault()
            useCallScreen.getState().ask()
          }
        }}
      >
        {picture ? <VideoTile tile={{ ...picture, large: false }} ctl={video} /> : null}
        <div className="flex items-center px-2 text-[11px] text-[var(--color-text-muted)]" style={{ height: DURATION_BAND }}>
          {time ? (
            <span data-testid="call-tile-duration" className="font-mono tabular-nums text-[var(--color-text-secondary)]">
              {time}
            </span>
          ) : null}
        </div>
      </Surface>
      <Surface role2="elevated" elevation={held ? 5 : 3} data-testid="call-tile-strip" className="pointer-events-auto p-1.5">
        <CallControls variant="tile" call={call} actions={actions} video={video} />
        {/* The share list (where the OS has no picker of its own) and why a
            camera or share was refused, with the way to the privacy settings. */}
        <ScreenPicker ctl={video} />
        <VideoNoticeLine ctl={video} noticeOnly />
      </Surface>
    </div>
  )
}

/** The floating tile, portalled over the window. */
export default function CallTile(props: CallTileProps): React.JSX.Element | null {
  if (typeof document === 'undefined') return null
  return createPortal(<Card {...props} />, document.body)
}
