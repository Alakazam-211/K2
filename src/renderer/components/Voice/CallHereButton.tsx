// In a window that isn't the call's home (one call, one home window), the
// slim bar's action moves the call's stage here: Show call here, or Back to
// chat in the window the call's own window came from. The old home folds to
// the slim bar; the call's own window closes. Only while the call has video:
// without it there is no stage to move.

import { Button } from '@/components/ui/Button'
import { CALL_WINDOW_LABEL, readPoppedOutFrom } from '@/lib/voice/call-view'
import { getWindowLabel } from '@/lib/window-chrome'
import { showCallHere, useCallHome, useIsCallHome } from '@/stores/call-home'
import { expandCallStage } from '@/stores/call-place'
import { useSettingsStore } from '@/stores/settings'
import { video } from '@/stores/video'
import type { VoiceCall } from '@/stores/voice-core'
import { useCallVideoLive } from './callHooks'

/** Whether this window offers to take the stage, and in which words. */
export function useCallHereAction(call: VoiceCall): 'here' | 'back-to-chat' | null {
  const tracked = useCallHome((s) => s.tracked)
  const owner = useCallHome((s) => s.owner)
  const home = useIsCallHome()
  const preview = useSettingsStore((s) => s.videoRoomsPreview) === true
  const live = useCallVideoLive(video, call.participants)
  if (!tracked || home || !preview || !live) return null
  return owner === CALL_WINDOW_LABEL && readPoppedOutFrom() === getWindowLabel() ? 'back-to-chat' : 'here'
}

export default function CallHereButton({ kind, then }: { kind: 'here' | 'back-to-chat'; then?: () => void }): React.JSX.Element {
  const press = (): void => {
    showCallHere()
    expandCallStage()
    then?.()
  }
  if (kind === 'back-to-chat') {
    return (
      <Button variant="quiet" data-testid="call-bar-back-to-chat" title="Back to chat: the call's video comes back here, and its own window closes" onClick={press}>
        Back to chat
      </Button>
    )
  }
  return (
    <Button variant="quiet" data-testid="call-bar-here" title="Show call here: the call's video moves to this window" onClick={press}>
      Show call here
    </Button>
  )
}
