// The call's stage: its video tiles, with the call's controls under them.
// It sits in the page where it's placed (never floating over content, never
// pinned to a corner): its header holds the place's actions (Pop out, Full
// screen, Compact). Screens draw large; with no screen shared, the last
// person who spoke draws large (3 or more tiles). The speaker's tile has the
// accent ring. Your own camera is the mirrored self-view. Full screen and
// the call's own window draw a share in the middle with the people beside it
// (CallStage), and the same grid without one.

import { useEffect, useMemo, useState } from 'react'
import { useStore } from 'zustand'
import { videoTiles, withSpeakerEmphasis, type CallPerson } from '@/lib/voice/video-labels'
import type { VideoController } from '@/stores/video-core'
import VideoTile from './VideoTile'
import { ScreenPicker, VideoCallButtons, VideoNoticeLine } from './VideoControls'

const SMALL_BTN =
  'border border-[var(--color-border)] px-2 py-0.5 text-[11px] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)] disabled:opacity-50'

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const noDrag = { WebkitAppRegion: 'no-drag' } as any

/** Columns for n tiles: 1, 2, 2, 2, 3… up to 4. */
export function gridColumns(n: number): number {
  if (n <= 1) return 1
  if (n <= 4) return 2
  if (n <= 9) return 3
  return 4
}

/** The last person who spoke, kept until someone else does. */
export function useLastSpeaker(people: readonly CallPerson[]): string | null {
  const [last, setLast] = useState<string | null>(null)
  const now = people.find((p) => p.speaking && !p.isSelf && p.memberId)?.memberId ?? null
  useEffect(() => {
    if (now !== null && now !== last) setLast(now)
  }, [now, last])
  return last
}

/** The call's tiles as the stage draws them: the last speaker large (3 or
 *  more tiles). */
export function useStageTiles(ctl: VideoController, people: readonly CallPerson[]): ReturnType<typeof videoTiles> {
  const tracks = useStore(ctl.store, (s) => s.tracks)
  const local = useStore(ctl.store, (s) => s.local)
  const lastSpeaker = useLastSpeaker(people)
  return useMemo(() => withSpeakerEmphasis(videoTiles(tracks, local, people), lastSpeaker), [tracks, local, people, lastSpeaker])
}

/** Rows for n tiles (a large one counts 4) over `columns`. */
export function gridRows(n: number, columns: number): number {
  return Math.max(1, Math.ceil(n / Math.max(1, columns)))
}

/** The gap between tiles (Tailwind's gap-2). */
const TILE_GAP = 'calc(var(--spacing) * 2)'

/** A column's width: the widest 16:9 tile for which `columns` across and
 *  `rows` down both fit the tiles' box. The box is a size container, so
 *  `cqw` and `cqh` are its content box: the tiles shrink with the window and
 *  never run under the controls below it. */
export function stageTileWidth(columns: number, rows: number): string {
  const across = `calc((100cqw - ${columns - 1} * ${TILE_GAP}) / ${columns})`
  const down = `calc((100cqh - ${rows - 1} * ${TILE_GAP}) / ${rows} * 16 / 9)`
  return `min(${across}, ${down})`
}

/** The grid of tiles, in a box of its own that takes the room its place
 *  leaves (the controls sit after it, never over it), the tiles centred in
 *  it. Each live one zooms (this viewer's only: TileZoom). */
export function StageGrid({ tiles, ctl, className = '' }: { tiles: ReturnType<typeof videoTiles>; ctl: VideoController; className?: string }): React.JSX.Element {
  const units = tiles.reduce((n, t) => n + (t.large ? 4 : 1), 0)
  const columns = gridColumns(units)
  const rows = gridRows(units, columns)
  return (
    <div data-testid="video-grid-box" className={`@container-[size] min-h-0 min-w-0 flex-1 p-2 ${className}`}>
      <div
        data-testid="video-grid"
        data-columns={columns}
        data-rows={rows}
        className="grid h-full w-full place-content-center gap-2"
        style={{ gridTemplateColumns: `repeat(${columns}, ${stageTileWidth(columns, rows)})` }}
      >
        {tiles.map((t) => (
          <VideoTile key={t.key} tile={t} ctl={ctl} zoomable />
        ))}
      </div>
    </div>
  )
}

export interface VideoStageProps {
  ctl: VideoController
  people: readonly CallPerson[]
  title: string
  /** Buttons before the camera and screen ones (the call's Mute). */
  footerStart?: React.ReactNode
  /** Extra buttons in the footer after them (the call's Leave call). */
  footer?: React.ReactNode
  /** The demo closes itself. */
  onClose?: () => void
  /** The header's actions, from the place the stage sits in (a call's:
   *  Pop out, Full screen, Compact). */
  actions?: React.ReactNode
  /** Extra classes from the place (its size there). */
  className?: string
  /** A call's controls (on the call's bar), in place of the footer's own
   *  buttons (the demo's). */
  controls?: React.ReactNode
  /** A line under the header from the place (why Pop out failed). */
  notice?: React.ReactNode
}

export default function VideoStage({ ctl, people, title, footerStart, footer, onClose, actions, className = '', controls, notice }: VideoStageProps): React.JSX.Element | null {
  const availability = useStore(ctl.store, (s) => s.availability)
  const tiles = useStageTiles(ctl, people)
  if (availability !== 'ready' || tiles.length === 0) return null

  return (
    <div
      role="region"
      aria-label={`Video in ${title}`}
      data-testid="video-stage"
      className={`flex min-h-0 flex-col border border-[var(--color-border)] bg-[var(--color-bg)] ${className}`}
      style={noDrag}
    >
      <header className="flex items-center gap-2 border-b border-[var(--color-border)] px-2 py-1 text-[12px] text-[var(--color-text-primary)]">
        <span className="min-w-0 flex-1 truncate">{title}</span>
        {actions}
        {onClose ? (
          <button type="button" data-testid="video-stage-close" className={SMALL_BTN} onClick={onClose}>
            Close
          </button>
        ) : null}
      </header>
      {notice}
      <StageGrid tiles={tiles} ctl={ctl} />
      {controls ? (
        <footer data-testid="video-stage-controls" className="flex flex-shrink-0 flex-col items-center gap-1 px-2 pb-2 pt-1">
          {controls}
          <ScreenPicker ctl={ctl} />
          <VideoNoticeLine ctl={ctl} noticeOnly />
        </footer>
      ) : (
        <footer className="border-t border-[var(--color-border)] px-2 py-1">
          <div className="flex items-center gap-1.5">
            {footerStart}
            <VideoCallButtons ctl={ctl} />
            {footer}
          </div>
          <ScreenPicker ctl={ctl} />
          <VideoNoticeLine ctl={ctl} />
        </footer>
      )}
    </div>
  )
}
