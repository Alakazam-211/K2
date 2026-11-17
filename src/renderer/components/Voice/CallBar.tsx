// The slim in-call bar in the top bar: the call's name, its time and its
// controls (mute, deafen, camera, share, leave), and the actions that say
// where the call goes from here (Expand or Back to call, Pop out, Show call
// here, Back to chat). It is the whole call while the call is audio only, and
// the call wherever its tile or stage isn't showing. In the top bar's flow,
// never a box over the content.

import { callTitle } from '@/lib/home-chat/call-names'
import { useSettingsStore } from '@/stores/settings'
import { video } from '@/stores/video'
import { voice } from '@/stores/voice'
import type { VoiceCall } from '@/stores/voice-core'
import CallControls from './CallControls'
import { useCallNoiseRemoval } from './CallLayer'
import { useCallTime } from './callHooks'
import { useCallPopout } from '@/stores/call-popout'
import { useCallHomeIsCallWindow } from '@/stores/call-home'
import { useCallBarFit } from './call-bar-fit'

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const noDrag = { WebkitAppRegion: 'no-drag' } as any

export interface CallBarProps {
  call: VoiceCall
  /** Where the call goes from here (buttons), or nothing. */
  action?: React.ReactNode
}

export default function CallBar({ call, action }: CallBarProps): React.JSX.Element {
  const time = useCallTime(call)
  const noiseRemoval = useCallNoiseRemoval()
  const videoPreview = useSettingsStore((s) => s.videoRoomsPreview) === true
  const title = callTitle(call)
  // Popped out: the bar says where the call went.
  const poppedOut = useCallPopout((s) => s.open)
  const inItsWindow = useCallHomeIsCallWindow() || poppedOut
  const controls = <CallControls variant="topbar" call={call} actions={voice} video={videoPreview ? video : null} noiseRemoval={noiseRemoval} />
  // It folds before the top bar overflows (call-bar-fit).
  const fit = useCallBarFit(true)
  return (
    <div ref={fit} role="region" aria-label={`In call: ${title}`} data-testid="call-bar" data-look="top" className="ml-0.5 flex min-w-0 items-center gap-2 text-[12px]" style={noDrag}>
      <span data-testid="call-bar-title" data-call-bar-part="name" className="min-w-0 max-w-[160px] truncate text-[var(--color-text-primary)]">
        {title}
      </span>
      <span data-testid="call-bar-time" data-call-bar-part="time" className="text-[11px] tabular-nums text-[var(--color-text-secondary)]">
        {time ?? 'Connecting…'}
      </span>
      {inItsWindow ? (
        <span data-testid="call-bar-where" data-call-bar-part="name" className="whitespace-nowrap text-[11px] text-[var(--color-text-secondary)]">
          In its own window
        </span>
      ) : null}
      {controls}
      {action}
    </div>
  )
}
