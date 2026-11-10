// Video in voice rooms: one tile. A canvas the tile's sink draws on, the
// person's name and mic at its bottom left, and in place of the picture: their
// avatar while the camera's off, or a line saying why it's paused. A tile may
// carry one more label at its bottom right (the floating tile's and full
// screen's call time). Labels sit on the picture on a chip of their own
// (`--color-media-label-*`), readable on any video in every Style.

import { useEffect, useRef, type ReactNode } from 'react'
import PresenceAvatar from '@/components/Presence/PresenceAvatar'
import { MicOffIcon } from '@/components/icons/K2Icons'
import { pausedText, type VideoTile as Tile } from '@/lib/voice/video-labels'
import type { VideoController } from '@/stores/video-core'

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

/** A label drawn on the picture: the name's chip, the time's. */
export const MEDIA_LABEL =
  'flex max-w-full items-center gap-1 bg-[var(--color-media-label-bg)] px-1.5 py-0.5 text-[11px] leading-[14px] text-[var(--color-media-label-text)]'

export interface VideoTileProps {
  tile: Tile
  ctl: VideoController
  /** At the bottom right, on the picture (the call's time). */
  corner?: ReactNode
  /** Fill the box it's given (full screen) instead of keeping 16:9 itself;
   *  the picture is letterboxed inside. */
  fill?: boolean
}

export default function VideoTile({ tile, ctl, corner, fill = false }: VideoTileProps): React.JSX.Element {
  const canvasRef = useRef<HTMLCanvasElement>(null)
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

  const line = placeholderLine(tile)
  return (
    <div
      data-testid="video-tile"
      data-track={tile.key}
      data-state={tile.state}
      data-speaking={tile.speaking ? 'true' : 'false'}
      data-large={tile.large ? 'true' : 'false'}
      data-self={tile.self ? 'true' : 'false'}
      className={`relative min-h-0 min-w-0 overflow-hidden border bg-black ${
        tile.speaking ? 'border-[var(--color-accent)] outline outline-2 outline-[var(--color-accent)]' : 'border-[var(--color-border)]'
      } ${tile.large && !fill ? 'col-span-2 row-span-2' : ''} ${fill ? 'h-full w-full' : ''}`}
      style={fill ? undefined : { aspectRatio: '16 / 9' }}
    >
      <canvas
        ref={canvasRef}
        data-testid="video-canvas"
        className="absolute inset-0 h-full w-full object-contain"
        style={{ transform: tile.mirrored ? 'scaleX(-1)' : undefined, visibility: line === null ? 'visible' : 'hidden' }}
      />
      {line !== null ? (
        <div className="absolute inset-0 flex flex-col items-center justify-center gap-1 bg-[var(--color-bg-elevated)]">
          {tile.state === 'off' && tile.source === 'camera' ? <PresenceAvatar name={tile.self ? 'You' : tile.name} role="member" size={36} /> : null}
          <span data-testid="video-tile-line" className="px-2 text-center text-[11px] text-[var(--color-text-muted)]">
            {line}
          </span>
        </div>
      ) : null}
      <div data-testid="video-tile-label" data-at="bottom-left" className={`absolute bottom-0 left-0 ${MEDIA_LABEL} ${corner ? 'max-w-[60%]' : ''}`}>
        {tile.audioMuted ? (
          <span data-testid="video-tile-muted" title="Microphone off" className="flex">
            <MicOffIcon size={12} />
          </span>
        ) : null}
        <span data-testid="video-tile-name" className="truncate">
          {tile.name}
        </span>
      </div>
      {corner ? (
        <div data-testid="video-tile-corner" data-at="bottom-right" className={`absolute bottom-0 right-0 ${MEDIA_LABEL}`}>
          {corner}
        </div>
      ) : null}
    </div>
  )
}
