// The slim in-call bar: the call's name, its time and its controls (mute,
// deafen, camera, share, leave), and one action that says where the call's
// video goes (Expand, Back to call, Show call here, Back to chat). It is the
// stage folded (Compact) at the top of the call's conversation, and the call
// everywhere else in the app (the top bar). Never a box over the content.

import { callTitle } from '@/lib/home-chat/call-names'
import { callStartedAt } from '@/lib/voice/call-view'
import { useSettingsStore } from '@/stores/settings'
import { video } from '@/stores/video'
import { voice } from '@/stores/voice'
import type { VoiceCall } from '@/stores/voice-core'
import CallControls, { CallControlBar } from './CallControls'
import { useCallNoiseRemoval } from './CallLayer'
import { useCallDuration } from './callHooks'
import { useCallPopout } from '@/stores/call-popout'
import { useCallHomeIsCallWindow } from '@/stores/call-home'

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const noDrag = { WebkitAppRegion: 'no-drag' } as any

/** The call's time as every window counts it (from the earliest second any
 *  window saw it connect). Null while it connects. */
export function useCallTime(call: VoiceCall): string | null {
  const startedAt = call.callId ? callStartedAt(call.callId, call.connectedAt) : call.connectedAt
  return useCallDuration(startedAt)
}

export interface CallBarProps {
  call: VoiceCall
  /** Where the call's video goes from here (a button), or nothing. */
  action?: React.ReactNode
  /** `top`: in the top bar, no frame of its own; `dock`: at the top of the
   *  call's conversation, ruled off from the messages below it. */
  look: 'top' | 'dock'
}

export default function CallBar({ call, action, look }: CallBarProps): React.JSX.Element {
  const time = useCallTime(call)
  const noiseRemoval = useCallNoiseRemoval()
  const videoPreview = useSettingsStore((s) => s.videoRoomsPreview) === true
  const title = callTitle(call)
  // Popped out: the bar says where the call went.
  const poppedOut = useCallPopout((s) => s.open)
  const inItsWindow = useCallHomeIsCallWindow() || poppedOut
  const controls = <CallControls variant="topbar" call={call} actions={voice} video={videoPreview ? video : null} noiseRemoval={noiseRemoval} />
  const frame = look === 'dock' ? 'border-b border-[var(--color-border)] px-4 py-1.5' : 'ml-0.5'
  return (
    <div role="region" aria-label={`In call: ${title}`} data-testid="call-bar" data-look={look} className={`flex min-w-0 items-center gap-2 text-[12px] ${frame}`} style={noDrag}>
      <span data-testid="call-bar-title" className={`min-w-0 truncate text-[var(--color-text-primary)] ${look === 'top' ? 'max-w-[160px]' : ''}`}>
        {title}
      </span>
      <span data-testid="call-bar-time" className="font-mono text-[11px] tabular-nums text-[var(--color-text-secondary)]">
        {time ?? 'Connecting…'}
      </span>
      {inItsWindow ? (
        <span data-testid="call-bar-where" className="whitespace-nowrap text-[11px] text-[var(--color-text-secondary)]">
          In its own window
        </span>
      ) : null}
      {look === 'dock' ? <span className="flex-1" /> : null}
      {look === 'dock' ? (
        // On the call's bar, as the stage's controls are.
        <CallControlBar className="py-1">{controls}</CallControlBar>
      ) : (
        controls
      )}
      {action}
    </div>
  )
}
