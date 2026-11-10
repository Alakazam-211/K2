// The call's video in a big view (the expanded view and the call's own
// window): with a screen share, the share in the middle and the people in a
// column of smaller tiles on the left (lib/voice/call-share.ts has the rule,
// and which share is in the middle with several); with none, the grid as the
// call view draws it.

import { useMemo } from 'react'
import { shareLayout, type ShareLayout } from '@/lib/voice/call-share'
import type { CallPerson } from '@/lib/voice/video-labels'
import { pickShare, useCallShare } from '@/stores/call-share'
import type { VideoController } from '@/stores/video-core'
import { StageGrid, useStageTiles } from './VideoStage'
import VideoTile from './VideoTile'

export interface CallStageProps {
  ctl: VideoController
  people: readonly CallPerson[]
  /** The call, for the kept pick (pop out and pop in keep it). */
  callId: string | null
  className?: string
}

/** The share layout's column width: a fifth of the view, between 132px and
 *  220px. */
const COLUMN = 'w-[clamp(132px,20%,220px)]'

function ShareView({ layout, ctl, callId, className }: { layout: ShareLayout; ctl: VideoController; callId: string | null; className: string }): React.JSX.Element {
  const { main, others, people } = layout
  return (
    <div data-testid="call-share-layout" data-main={main.key} className={`flex min-h-0 gap-2 p-2 ${className}`}>
      <div data-testid="call-share-column" className={`flex ${COLUMN} shrink-0 flex-col gap-1.5 overflow-y-auto`}>
        {others.length > 0 ? (
          <div role="group" aria-label="Other screen shares" className="flex flex-col gap-1.5">
            {others.map((t) => (
              <button
                key={t.key}
                type="button"
                data-testid="call-share-pick"
                data-track={t.key}
                aria-label={`Show ${t.name} in the middle`}
                title={`Show ${t.name} in the middle`}
                className="block w-full shrink-0 text-left"
                onClick={() => pickShare(callId, t.key)}
              >
                <VideoTile tile={t} ctl={ctl} />
              </button>
            ))}
            <div aria-hidden className="h-px shrink-0 bg-[var(--color-border)]" />
          </div>
        ) : null}
        <div role="list" aria-label="People in the call" data-testid="call-share-people" className="flex flex-col gap-1.5">
          {people.map((t) => (
            <div key={t.key} role="listitem" className="shrink-0">
              <VideoTile tile={t} ctl={ctl} />
            </div>
          ))}
        </div>
      </div>
      <div data-testid="call-share-main" className="relative min-h-0 min-w-0 flex-1">
        <VideoTile tile={main} ctl={ctl} fill />
      </div>
    </div>
  )
}

export default function CallStage({ ctl, people, callId, className = '' }: CallStageProps): React.JSX.Element {
  const tiles = useStageTiles(ctl, people)
  const picked = useCallShare((s) => s.picked)
  const layout = useMemo(() => shareLayout(tiles, people, picked), [tiles, people, picked])
  if (!layout) return <StageGrid tiles={tiles} ctl={ctl} className={`content-center ${className}`} />
  return <ShareView layout={layout} ctl={ctl} callId={callId} className={className} />
}
