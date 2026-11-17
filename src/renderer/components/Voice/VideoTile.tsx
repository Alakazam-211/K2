// Video in voice rooms: one tile. A canvas the tile's sink draws on, the
// person's name and mic at its bottom left, and in place of the picture: their
// avatar while the camera's off, or a line saying why it's paused. A tile may
// carry one more label at its bottom right (the floating tile's call time).
// Labels sit on the picture on the look's chip (`--color-media-label-*`),
// readable on any video in every Style. A
// `zoomable` tile (a share in the middle, the call view's grid) lets this
// viewer zoom in and move around its picture (TileZoom).
//
// The tile follows the active look: the look's raised tile surface and edge
// (`--tile-raised`, `--tile-edge`), rounded by the tile radius in Glass and
// cut by the surface chamfer in Terminal (`data-k2-chamfer`). Behind a
// picture that doesn't fill it, and in place of one, is that surface, never
// a black box.

import { useEffect, useRef, type CSSProperties, type ReactNode } from 'react'
import PresenceAvatar from '@/components/Presence/PresenceAvatar'
import { MicOffIcon } from '@/components/icons/K2Icons'
import { pausedText, type VideoTile as Tile } from '@/lib/voice/video-labels'
import { sinkZoom, viewTransform, zoomResetKey } from '@/lib/voice/share-zoom'
import type { VideoController } from '@/stores/video-core'
import { useTileZoom, ZoomControls } from './TileZoom'

function placeholderLine(t: Tile): string | null {
  switch (t.state) {
    case 'live':
      return null
    case 'off':
      return t.source === 'screen' ? 'Not sharing' : 'Camera off'
    case 'asking':
      return 'Starting your camera…'
    case 'unsupported':
      return "This computer can't show this video"
    case 'paused':
      return pausedText(t.paused ?? 'background')
  }
}

/** A label drawn on the picture: the name's chip, the time's. The look's
 *  chip: a pill in Glass (`--radius-pill`); in Terminal a cut chip where the
 *  element carries `data-k2-chamfer="chip"` (CHIP_CUT). Its colours read at
 *  4.5:1 or better over any picture, white ones included. */
export const MEDIA_LABEL =
  'flex min-w-0 items-center gap-1 rounded-[var(--radius-pill)] bg-[var(--color-media-label-bg)] px-2 py-0.5 text-[11px] leading-[14px] text-[var(--color-media-label-text)]'

/** A media label's Terminal cut, with no edge drawn on it (the chip has no
 *  border; the cut's edge would otherwise take the text colour). */
export const CHIP_CUT = { 'data-k2-chamfer': 'chip', style: { '--k2-ch-w': '0px' } as CSSProperties } as const

export interface VideoTileProps {
  tile: Tile
  ctl: VideoController
  /** At the bottom right, on the picture (the call's time). */
  corner?: ReactNode
  /** Fill the box it's given (a share in the middle) instead of keeping 16:9 itself;
   *  the picture is letterboxed inside. */
  fill?: boolean
  /** This viewer may zoom in on the picture while it's live (TileZoom). */
  zoomable?: boolean
  /** A small tile (the share layout's column): a smaller avatar. */
  compact?: boolean
}

export default function VideoTile({ tile, ctl, corner, fill = false, zoomable = false, compact = false }: VideoTileProps): React.JSX.Element {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const frameRef = useRef<HTMLDivElement>(null)
  const held = tile.state !== 'live'
  // Off and unsupported tracks never need a sink; paused ones keep theirs, held.
  const wantsSink = tile.state === 'live' || tile.state === 'paused'
  const sinkTile = tile.large ? 'large' : 'grid'

  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas || !wantsSink) return
    return ctl.attach(tile.key, canvas, { tile: sinkTile, held })
    // `held` follows below without re-attaching.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ctl, tile.key, wantsSink, sinkTile])

  useEffect(() => {
    ctl.setHeld(tile.key, held)
  }, [ctl, tile.key, held])

  const zoomOn = zoomable && tile.state === 'live'
  const zoom = useTileZoom({ enabled: zoomOn, resetKey: zoomResetKey(tile.key, tile.state), frameRef, canvasRef })
  // The sink asks for a sharper picture while zoomed in; fit again after.
  const wantSharp = sinkZoom(zoom.view)
  useEffect(() => {
    if (wantSharp === 1) return
    ctl.setZoom(tile.key, wantSharp)
    return () => ctl.setZoom(tile.key, 1)
  }, [ctl, tile.key, wantSharp])

  const line = placeholderLine(tile)
  const cameraOff = tile.state === 'off' && tile.source === 'camera'
  const avatarSize = compact ? 40 : tile.large || fill ? 72 : 56
  // The name's chip, with the mic when it's off: on the picture's bottom
  // left, or under the avatar while the camera's off.
  const label = (at: 'bottom-left' | 'center'): React.JSX.Element => (
    <div
      data-testid="video-tile-label"
      data-at={at}
      {...CHIP_CUT}
      className={
        at === 'center'
          ? `${MEDIA_LABEL} max-w-full`
          : `absolute bottom-1.5 left-1.5 ${MEDIA_LABEL} ${corner ? 'max-w-[60%]' : 'max-w-[calc(100%-12px)]'}`
      }
    >
      {tile.audioMuted ? (
        <span data-testid="video-tile-muted" title="Microphone off" className="flex shrink-0">
          <MicOffIcon size={12} />
        </span>
      ) : null}
      <span data-testid="video-tile-name" className="truncate">
        {tile.name}
      </span>
    </div>
  )
  const transform = viewTransform(zoom.view)
  return (
    <div
      ref={frameRef}
      {...(zoomOn
        ? {
            ...zoom.frameProps,
            tabIndex: 0,
            role: 'group',
            'aria-label': `${tile.name}: zoom with + and −, move with the arrow keys while zoomed, Esc to fit`,
          }
        : {})}
      data-testid="video-tile"
      data-track={tile.key}
      data-state={tile.state}
      data-speaking={tile.speaking ? 'true' : 'false'}
      data-large={tile.large ? 'true' : 'false'}
      data-self={tile.self ? 'true' : 'false'}
      data-zoomable={zoomOn ? 'true' : undefined}
      data-zoomed={zoom.zoomed ? 'true' : undefined}
      data-k2-chamfer="surface"
      className={`relative min-h-0 min-w-0 rounded-[var(--radius-box)] border ${tile.speaking ? 'border-[var(--color-accent)]' : 'border-[var(--tile-edge)]'} bg-[var(--tile-raised)] [box-shadow:var(--tile-sheen)] ${zoomOn ? 'group/zoom' : ''} ${
        zoom.zoomed ? 'cursor-grab active:cursor-grabbing' : ''
      } ${tile.large && !fill ? 'col-span-2 row-span-2' : ''} ${fill ? 'h-full w-full' : ''}`}
      style={fill ? undefined : { aspectRatio: '16 / 9' }}
    >
      {/* The picture and what stands in for it, clipped to the tile's inner
          corner (its radius less the 1px edge). The tile itself doesn't
          clip, so its ring and Terminal's cut edge stay whole. */}
      <div data-testid="video-tile-media" className="absolute inset-0 overflow-hidden rounded-[calc(var(--radius-box)-1px)]">
        {/* The zoom's layer: always there, so the canvas never moves to a new
            parent (its sink watches the parent's box, which a transform never
            changes). */}
        <div
          data-testid="video-zoom-layer"
          className="absolute inset-0"
          style={transform ? { transform, transformOrigin: '0 0', willChange: 'transform' } : undefined}
        >
          <canvas
            // A canvas per track: a sink that's closing (its short grace period)
            // must never draw over the next track's picture on a reused canvas.
            key={tile.key}
            ref={canvasRef}
            data-testid="video-canvas"
            className="absolute inset-0 h-full w-full object-contain"
            style={{ transform: tile.mirrored ? 'scaleX(-1)' : undefined, visibility: line === null ? 'visible' : 'hidden' }}
          />
        </div>
        {cameraOff ? (
          // The camera's off: the person, as everywhere else in K2, on the
          // tile's surface, with their name's chip (and its mic) under them.
          <div data-testid="video-tile-off" className="absolute inset-0 flex flex-col items-center justify-center gap-1.5 bg-[var(--tile-raised)] p-2">
            <span data-testid="video-tile-avatar" data-size={avatarSize} className="flex shrink-0">
              <PresenceAvatar name={tile.self ? 'You' : tile.name} role="member" size={avatarSize} />
            </span>
            {label('center')}
            <span data-testid="video-tile-line" className="sr-only">
              {line}
            </span>
          </div>
        ) : line !== null ? (
          <div data-testid="video-tile-placeholder" className="absolute inset-0 flex flex-col items-center justify-center gap-1 bg-[var(--tile-raised)]">
            <span data-testid="video-tile-line" className="px-2 text-center text-[11px] text-[var(--color-text-muted)]">
              {line}
            </span>
          </div>
        ) : null}
      </div>
      {cameraOff ? null : label('bottom-left')}
      {corner ? (
        <div data-testid="video-tile-corner" data-at="bottom-right" {...CHIP_CUT} className={`absolute bottom-1.5 right-1.5 ${MEDIA_LABEL}`}>
          {corner}
        </div>
      ) : null}
      {zoomOn ? <ZoomControls zoom={zoom} name={tile.name} chip={MEDIA_LABEL} /> : null}
    </div>
  )
}
