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

/** The grid of tiles. Each live one zooms (this viewer's only: TileZoom). */
export function StageGrid({ tiles, ctl, className = '' }: { tiles: ReturnType<typeof videoTiles>; ctl: VideoController; className?: string }): React.JSX.Element {
  const columns = gridColumns(tiles.reduce((n, t) => n + (t.large ? 4 : 1), 0))
  return (
    <div
      data-testid="video-grid"
      data-columns={columns}
      className={`grid min-h-0 gap-2 overflow-y-auto p-2 ${className}`}
      style={{ gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))` }}
    >
      {tiles.map((t) => (
        <VideoTile key={t.key} tile={t} ctl={ctl} zoomable />
      ))}
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
}

export default function VideoStage({ ctl, people, title, footerStart, footer, onClose, actions, className = '' }: VideoStageProps): React.JSX.Element | null {
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
      <StageGrid tiles={tiles} ctl={ctl} />
      <footer className="border-t border-[var(--color-border)] px-2 py-1">
        <div className="flex items-center gap-1.5">
          {footerStart}
          <VideoCallButtons ctl={ctl} />
          {footer}
        </div>
        <ScreenPicker ctl={ctl} />
        <VideoNoticeLine ctl={ctl} />
      </footer>
    </div>
  )
}
