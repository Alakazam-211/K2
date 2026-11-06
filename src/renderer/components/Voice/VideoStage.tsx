// Video in voice rooms: the call's video, in a panel at the window's bottom right
// that stays while the headset's list is closed. Screens draw large; with no
// screen shared, the last person who spoke draws large (3 or more tiles). The
// speaker's tile has the accent outline. Your own camera is the mirrored self-view.

import { useEffect, useMemo, useRef, useState } from 'react'
import { useStore } from 'zustand'
import { useBrowserOverlay } from '@/lib/browser-overlay'
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
function useLastSpeaker(people: readonly CallPerson[]): string | null {
  const [last, setLast] = useState<string | null>(null)
  const now = people.find((p) => p.speaking && !p.isSelf && p.memberId)?.memberId ?? null
  useEffect(() => {
    if (now !== null && now !== last) setLast(now)
  }, [now, last])
  return last
}

export interface VideoStageProps {
  ctl: VideoController
  people: readonly CallPerson[]
  title: string
  /** Extra buttons in the footer (Mute, Leave). */
  footer?: React.ReactNode
  /** The demo closes itself; the real one hides to a pill. */
  onClose?: () => void
}

export default function VideoStage({ ctl, people, title, footer, onClose }: VideoStageProps): React.JSX.Element | null {
  const tracks = useStore(ctl.store, (s) => s.tracks)
  const local = useStore(ctl.store, (s) => s.local)
  const availability = useStore(ctl.store, (s) => s.availability)
  const lastSpeaker = useLastSpeaker(people)
  const tiles = useMemo(() => withSpeakerEmphasis(videoTiles(tracks, local, people), lastSpeaker), [tracks, local, people, lastSpeaker])
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

  const columns = gridColumns(tiles.reduce((n, t) => n + (t.large ? 4 : 1), 0))
  return (
    <div
      ref={rootRef}
      role="region"
      aria-label={`Video in ${title}`}
      data-testid="video-stage"
      className={`fixed bottom-3 right-3 z-40 flex max-h-[80vh] flex-col border border-[var(--color-border)] bg-[var(--color-bg)] shadow-lg ${
        wide ? 'w-[min(720px,calc(100vw-24px))]' : 'w-[min(380px,calc(100vw-24px))]'
      }`}
      style={noDrag}
    >
      <header className="flex items-center gap-2 border-b border-[var(--color-border)] px-2 py-1 text-[12px] text-[var(--color-text-primary)]">
        <span className="min-w-0 flex-1 truncate">{title}</span>
        <button type="button" data-testid="video-stage-size" className={SMALL_BTN} onClick={() => setWide(!wide)}>
          {wide ? 'Smaller' : 'Bigger'}
        </button>
        <button type="button" data-testid="video-stage-hide" className={SMALL_BTN} onClick={() => (onClose ? onClose() : setHidden(true))}>
          {onClose ? 'Close' : 'Hide'}
        </button>
      </header>
      <div
        data-testid="video-grid"
        data-columns={columns}
        className="grid min-h-0 gap-1 overflow-y-auto p-1"
        style={{ gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))` }}
      >
        {tiles.map((t) => (
          <VideoTile key={t.key} tile={t} ctl={ctl} />
        ))}
      </div>
      <footer className="border-t border-[var(--color-border)] px-2 py-1">
        <div className="flex items-center gap-1">
          <VideoCallButtons ctl={ctl} />
          {footer}
        </div>
        <ScreenPicker ctl={ctl} />
        <VideoNoticeLine ctl={ctl} />
      </footer>
    </div>
  )
}
