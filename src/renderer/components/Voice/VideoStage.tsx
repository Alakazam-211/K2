// Video in voice rooms: the call's video, in a panel at the window's bottom right
// that stays while the headset's list is closed. Screens draw large; with no
// screen shared, the last person who spoke draws large (3 or more tiles). The
// speaker's tile has the accent outline. Your own camera is the mirrored self-view.
// In a call, its header also has Expand (the call over K2, CallExpanded) and
// Pop out (the call in its own window); those two draw a share in the middle
// with the people beside it (CallStage), and the same grid without one.

import { useEffect, useMemo, useRef, useState } from 'react'
import { useStore } from 'zustand'
import { useBrowserOverlay } from '@/lib/browser-overlay'
import { videoTiles, withSpeakerEmphasis, type CallPerson } from '@/lib/voice/video-labels'
import type { VideoController } from '@/stores/video-core'
import VideoTile from './VideoTile'
import { ScreenPicker, VideoCallButtons, VideoNoticeLine } from './VideoControls'
import { ExpandButton, PopOutButton, useExpandChordLabel } from './CallViewButtons'

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

/** The grid of tiles. */
export function StageGrid({ tiles, ctl, className = '' }: { tiles: ReturnType<typeof videoTiles>; ctl: VideoController; className?: string }): React.JSX.Element {
  const columns = gridColumns(tiles.reduce((n, t) => n + (t.large ? 4 : 1), 0))
  return (
    <div
      data-testid="video-grid"
      data-columns={columns}
      className={`grid min-h-0 gap-1 overflow-y-auto p-1 ${className}`}
      style={{ gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))` }}
    >
      {tiles.map((t) => (
        <VideoTile key={t.key} tile={t} ctl={ctl} />
      ))}
    </div>
  )
}

export interface VideoStageProps {
  ctl: VideoController
  people: readonly CallPerson[]
  title: string
  /** Buttons before the camera and screen ones (the call view's Mute). */
  footerStart?: React.ReactNode
  /** Extra buttons in the footer after them (the call view's Leave). */
  footer?: React.ReactNode
  /** The demo closes itself. */
  onClose?: () => void
  /** The call's view hides back to the floating tile (CallLayer). Without
   *  this or `onClose`, it hides to a pill. */
  onHide?: () => void
  /** A call's view (not the demo): Expand and Pop out in the header. */
  viewButtons?: boolean
}

export default function VideoStage({ ctl, people, title, footerStart, footer, onClose, onHide, viewButtons = false }: VideoStageProps): React.JSX.Element | null {
  const availability = useStore(ctl.store, (s) => s.availability)
  const tiles = useStageTiles(ctl, people)
  const chord = useExpandChordLabel()
  const [hidden, setHidden] = useState(false)
  const [wide, setWide] = useState(false)
  const rootRef = useRef<HTMLDivElement>(null)
  const shown = availability === 'ready' && tiles.length > 0
  // The panel sits over the room, so it gives way over a live Browser page.
  useBrowserOverlay({ active: shown, ref: rootRef, kind: 'popover', watch: `${hidden}:${wide}:${tiles.length}` })
  if (!shown) return null

  if (hidden) {
    return (
      <div ref={rootRef} className="fixed bottom-3 right-3 z-40" style={noDrag}>
        <button type="button" data-testid="video-stage-show" className={`${SMALL_BTN} bg-[var(--color-bg)] shadow-lg`} onClick={() => setHidden(false)}>
          Show video ({tiles.length})
        </button>
      </div>
    )
  }

  return (
    <div
      ref={rootRef}
      role="region"
      aria-label={`Video in ${title}`}
      data-testid="video-stage"
      className={`fixed bottom-3 right-3 z-40 flex max-h-[calc(var(--k2-vh)*0.8)] flex-col border border-[var(--color-border)] bg-[var(--color-bg)] shadow-lg ${
        wide ? 'w-[min(720px,calc(var(--k2-vw)-24px))]' : 'w-[min(380px,calc(var(--k2-vw)-24px))]'
      }`}
      style={noDrag}
    >
      <header className="flex items-center gap-2 border-b border-[var(--color-border)] px-2 py-1 text-[12px] text-[var(--color-text-primary)]">
        <span className="min-w-0 flex-1 truncate">{title}</span>
        {viewButtons ? <PopOutButton testId="video-stage-popout" look="panel" /> : null}
        {viewButtons ? <ExpandButton testId="video-stage-expand" look="panel" chord={chord} /> : null}
        <button type="button" data-testid="video-stage-size" className={SMALL_BTN} onClick={() => setWide(!wide)}>
          {wide ? 'Smaller' : 'Bigger'}
        </button>
        <button type="button" data-testid="video-stage-hide" className={SMALL_BTN} onClick={() => (onClose ? onClose() : onHide ? onHide() : setHidden(true))}>
          {onClose ? 'Close' : 'Hide'}
        </button>
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
