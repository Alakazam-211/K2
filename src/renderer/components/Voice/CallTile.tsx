// The floating call tile: while the call carries video (anyone's camera or a
// screen share) and the person hasn't chosen Expand or Pop out, a small video
// tile floats in a corner of the call's home window, on every page, with the
// call's controls on their bar just under it. The two drag as one, snap to
// the nearest of the four corners of the free content area (under the tab
// strip, beside open drawers) on the release velocity, and the corner is
// remembered for this client. Without video there's no tile: the call is the
// top bar's slim bar.
//
// Ported from K2 Companion's `callTile.tsx` (the card, the strip, the time)
// with its motion (`useCornerDrag`, `mini-snap.ts`). What differs: the
// picture is K2's own video tile (a canvas a sink draws on, not a native
// layer), it follows the active speaker with the same 1.5 s hold, and the
// look is the active Style's: the tile surface (rounded in Glass, cut in
// Terminal) with its name chips and the camera-off avatar, the call's bar
// under it (Frost in Glass, a raised cut bar in Terminal) with the call's
// larger controls.
//
// A press that never drags is a click: a button's is the button's; anywhere
// else on the picture expands the call (its stage, in its conversation or at
// the top of the page). A double-click, or the chord, opens it full screen.
// Expand and Pop out sit on the picture's top edge. The name sits at the
// picture's bottom left and the call's time at its bottom right, on the
// picture itself. While someone else shares a screen the tile isn't showing
// (it follows the speaker), a chip on the picture says whose, and a click on
// it opens the call full screen, where the share is in the middle.

import { useEffect, useRef, useState, useSyncExternalStore } from 'react'
import { createPortal } from 'react-dom'
import { Surface } from '@/components/ui/Surface'
import { measureElement, overlayClaims, rectsOverlap, subscribeOverlayClaims, useBrowserOverlay } from '@/lib/browser-overlay'
import { CALL_TILE_Z } from '@/lib/overlay-layers'
import { composerBands, cornerSpots, freeArea, type Bounds, type Corner, type Rect, type Vec } from '@/lib/voice/mini-snap'
import { usePanelsStore } from '@/stores/panels'
import { DRAWER_ANIM_MS } from '@/components/Layout/drawer-motion'
import { holdLeft, nextSpeaker, NO_SPEAKER, speakingIds, tilePicture, type SpeakerHold } from '@/lib/voice/call-tile-picture'
import { isShare } from '@/lib/voice/call-share'
import type { CallPerson } from '@/lib/voice/video-labels'
import { DOUBLE_CLICK_MS } from '@/lib/voice/call-view'
import { expandCall } from '@/stores/call-expand'
import { popOutCall, useCallPopout } from '@/stores/call-popout'
import { CALL_TILE_GAP, clearCallTileRest, setCallTileRest } from '@/stores/call-tile-place'
import type { VideoController } from '@/stores/video-core'
import type { VoiceCall } from '@/stores/voice-core'
import CallControls, { CallControlBar, type CallControlsProps } from './CallControls'
import { PictureButton } from './CallViewButtons'
import { ScreenPicker, VideoNoticeLine } from './VideoControls'
import PopOutError from './PopOutError'
import { useCallTiles, useCallTime } from './callHooks'
import { useExpandCall } from './callConversation'
import { reduceMotion, useCornerDrag, useFadeIn } from './useCornerDrag'
import VideoTile, { MEDIA_LABEL } from './VideoTile'
import { ExpandIcon, PopOutIcon, ScreenShareIcon } from '@/components/icons/K2Icons'
import { TOPBAR_HEIGHT } from '../../../shared/constants'
import { clientRectToCss, cssViewport } from '@/lib/app-zoom'
import { callTitle } from '@/lib/home-chat/call-names'

/** The picture's width (px); it is 16:9. The call's bar under it may be
 *  wider (its controls at the call bar's size), and the tile is as wide as
 *  the wider of the two. */
export const CALL_TILE_WIDTH = 320
/** Between the picture and the call's bar (px). */
export const CALL_TILE_STRIP_GAP = 6
/** Clear of the window's edges, the top bar and what the tile avoids (px). */
export const CALL_TILE_INSETS = { top: TOPBAR_HEIGHT + 12, bottom: 12, side: 12, gap: CALL_TILE_GAP }
/** The message boxes: the terminal's compose bar, the Thread's slot, and
 *  the Thread look's box (Home chat, a project's chat, a ticket's agent).
 *  While one is on screen, the tile's bottom corners sit above it across the
 *  whole content area (composerBands), so it never covers Send or the
 *  newest message. */
export const CALL_TILE_COMPOSERS = '[data-compose-bar], [data-zen-compose], [data-testid="agent-session-thread-compose-slot"]'
/** Anything else the tile never covers, where it is. */
export const CALL_TILE_AVOID = '[data-call-tile-avoid]'
/** Composers come and go with tabs: the corner's spot is checked this often. */
export const CALL_TILE_RELAYOUT_MS = 1_000

/** The active speaker, held 1.5 s after they stop before someone else who is
 *  talking takes over (Sew's active-speaker hold). */
export function useActiveSpeaker(people: readonly CallPerson[]): string | null {
  const key = speakingIds(people).join('\n')
  // Whoever is talking already has it from the first draw (a tile drawn
  // again never flashes someone else first).
  const [hold, setHold] = useState<SpeakerHold>(() => nextSpeaker(NO_SPEAKER, speakingIds(people), Date.now()))
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

/** All of the tile's geometry is CSS px: it's placed with a translate, which
 *  the app zoom scales. Rects and the window are converted (lib/app-zoom). */
function rectOf(el: Element): Rect {
  const r = clientRectToCss(el.getBoundingClientRect())
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
  return freeArea(cssViewport(), { content, drawers, tabs: visibleRects(CALL_TILE_TABS, null) })
}

/** Each corner's spot now for a tile `el` (its top left, CSS px). */
export function measureCallTileSpots(el: HTMLElement | null): Record<Corner, Vec> {
  const size = { width: el?.offsetWidth || CALL_TILE_WIDTH, height: el?.offsetHeight || 0 }
  const area = measureCallTileArea()
  const avoid = [...composerBands(visibleRects(CALL_TILE_COMPOSERS, el), area), ...visibleRects(CALL_TILE_AVOID, el)]
  return cornerSpots(cssViewport(), size, CALL_TILE_INSETS, avoid, area)
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
  /** The wave button (CallControls), with the native engine. */
  noiseRemoval?: boolean | null
}

function Card({ call, video, actions, noiseRemoval }: CallTileProps): React.JSX.Element {
  const ref = useRef<HTMLDivElement>(null)
  const people = call.participants
  const tiles = useCallTiles(video, people)
  const speaker = useActiveSpeaker(people)
  const picture = tilePicture(tiles, speaker)
  // Someone else's share the picture isn't showing.
  const hiddenShare = picture?.source === 'screen' ? null : (tiles.find((t) => isShare(t) && !t.self) ?? null)
  const time = useCallTime(call)
  const title = callTitle(call)
  // Expand: the stage in the call's place (a DM this window can't place has
  // none, never a dead button).
  const expand = useExpandCall(call)
  const expandRef = useRef(expand)
  expandRef.current = expand
  const canPopOut = useCallPopout((s) => s.available)
  // A single click expands it, once it's clear it wasn't the first half of a
  // double-click (full screen).
  const tapTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  useEffect(
    () => () => {
      if (tapTimer.current) clearTimeout(tapTimer.current)
    },
    [],
  )
  // Where it rests, for a message list under it to make room (never its
  // newest message under the tile).
  useEffect(() => () => clearCallTileRest(), [])
  const { corner, held, moving, relayout, handlers } = useCornerDrag(ref, {
    spots: () => measureCallTileSpots(ref.current),
    onPlace: (at, c) => {
      const el = ref.current
      setCallTileRest({ left: at.x, top: at.y, width: el?.offsetWidth || CALL_TILE_WIDTH, height: el?.offsetHeight || 0, bottom: c.startsWith('bottom') })
    },
    // A button's click is the button's; anywhere else on the picture expands it.
    onTap: (target) => {
      const el = target instanceof Element ? target : null
      const hit = el?.closest('button')
      if (hit && ref.current?.contains(hit)) return
      if (!el?.closest('[data-call-tile-picture]')) return
      if (tapTimer.current) {
        // The second click of a double-click: `onDoubleClick` has it.
        clearTimeout(tapTimer.current)
        tapTimer.current = null
        return
      }
      tapTimer.current = setTimeout(() => {
        tapTimer.current = null
        expandRef.current?.()
      }, DOUBLE_CLICK_MS)
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

  const hint = expand ? 'Click the video to expand the call; double-click for full screen.' : 'Double-click the video for full screen.'
  return (
    <div
      ref={ref}
      role="group"
      aria-label={`${title}. ${hint}`}
      data-testid="call-tile"
      data-corner={corner}
      data-held={held ? 'true' : 'false'}
      data-picture={picture?.key ?? 'none'}
      data-overlapped={overlapped ? 'true' : 'false'}
      data-reduced-motion={reduceMotion() ? 'true' : 'false'}
      className="fixed flex flex-col items-center select-none"
      style={{
        left: 0,
        top: 0,
        minWidth: CALL_TILE_WIDTH,
        gap: CALL_TILE_STRIP_GAP,
        zIndex: CALL_TILE_Z,
        // The gap between the picture and the bar lets clicks through.
        pointerEvents: 'none',
        touchAction: 'none',
        willChange: 'transform',
        visibility: overlapped && !held ? 'hidden' : undefined,
        WebkitAppRegion: 'no-drag',
      } as React.CSSProperties}
      {...handlers}
    >
      {/* The float shadow: a box-shadow round the tile's radius in Glass, a
          drop shadow that follows the cut in Terminal. */}
      <div
        data-k2-chamfer-float=""
        data-testid="call-tile-picture"
        data-call-tile-picture=""
        className={`group pointer-events-auto relative cursor-pointer rounded-[var(--radius-box)] ${held ? '[box-shadow:var(--shadow-5)]' : '[box-shadow:var(--shadow-3)]'}`}
        style={{ width: CALL_TILE_WIDTH }}
        role="button"
        tabIndex={0}
        aria-label={expand ? `Expand ${title}` : `${title}, full screen`}
        onKeyDown={(e) => {
          if (e.target !== e.currentTarget || (e.key !== 'Enter' && e.key !== ' ')) return
          e.preventDefault()
          if (expand) expand()
          else expandCall('call-tile-picture')
        }}
        onDoubleClick={(e) => {
          const hit = e.target instanceof Element ? e.target.closest('button') : null
          if (hit) return
          if (tapTimer.current) {
            clearTimeout(tapTimer.current)
            tapTimer.current = null
          }
          expandCall('call-tile-picture')
        }}
      >
        {picture ? (
          <VideoTile
            tile={{ ...picture, large: false }}
            ctl={video}
            corner={
              time ? (
                <span data-testid="call-tile-duration" className="tabular-nums">
                  {time}
                </span>
              ) : null
            }
          />
        ) : null}
        <div
          data-testid="call-tile-view-buttons"
          className="absolute right-1.5 top-1.5 flex gap-1 opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100"
        >
          {expand ? (
            <PictureButton label="Expand: the call's stage in its place" testId="call-tile-expand" onPress={expand}>
              <ExpandIcon size={13} />
            </PictureButton>
          ) : null}
          {canPopOut ? (
            <PictureButton label="Pop out into its own window" testId="call-tile-popout" onPress={() => void popOutCall()}>
              <PopOutIcon size={13} />
            </PictureButton>
          ) : null}
        </div>
        {hiddenShare ? (
          <button
            type="button"
            data-testid="call-tile-share-cue"
            data-track={hiddenShare.key}
            aria-label={`Full screen, to see ${hiddenShare.name}`}
            title={`Full screen, to see ${hiddenShare.name}`}
            data-k2-chamfer="chip"
            className={`k2-focus absolute left-1.5 top-1.5 ${MEDIA_LABEL} max-w-[60%] opacity-90 hover:opacity-100 focus-visible:opacity-100`}
            style={{ WebkitAppRegion: 'no-drag', '--k2-ch-w': '0px' } as React.CSSProperties}
            onClick={() => expandCall('call-tile-share-cue')}
          >
            <ScreenShareIcon size={12} />
            <span className="truncate">{hiddenShare.name}</span>
          </button>
        ) : null}
      </div>
      <div data-k2-chamfer-float="" data-testid="call-tile-strip" className="pointer-events-auto flex flex-col items-center gap-1">
        <CallControlBar>
          <CallControls variant="overlay" call={call} actions={actions} video={video} noiseRemoval={noiseRemoval} />
        </CallControlBar>
        {/* The share list (where the OS has no picker of its own), why a
            camera or share was refused, and why Pop out failed, on a
            surface of their own (none while there's nothing to say). */}
        <Surface role2="elevated" chamfer data-testid="call-tile-notices" className="max-w-[360px] p-1.5 empty:hidden">
          <ScreenPicker ctl={video} />
          <VideoNoticeLine ctl={video} noticeOnly />
          <PopOutError />
        </Surface>
      </div>
    </div>
  )
}

/** The floating tile, portalled over the window. */
export default function CallTile(props: CallTileProps): React.JSX.Element | null {
  if (typeof document === 'undefined') return null
  return createPortal(<Card {...props} />, document.body)
}
