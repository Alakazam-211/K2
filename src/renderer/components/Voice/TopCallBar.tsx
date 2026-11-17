// The call elsewhere in the app: the slim in-call bar beside the top bar's
// headset (name, time, controls), in the top bar's flow, never a box over
// the content. It gives way where the call is already on screen in this
// window: its stage is up, or its conversation is open with the call at its
// top. Its one action says where the video is:
// - a DM call away from its DM: Back to call (opens the DM, stage open);
// - a voice room folded (Compact) with video on: Expand.
// A DM this window can't place has no Back to call: never a dead button.

import { Button } from '@/components/ui/Button'
import { callHasConversation, useCallPlace } from '@/stores/call-place'
import { useVoice } from '@/stores/voice'
import CallBar from './CallBar'
import { useCallInItsConversation, useCallStageShown, useCallVideoHere } from './CallLayer'
import { CompactButton } from './CallViewButtons'
import { openCallConversation, useCallConversationAddress } from './callConversation'

export default function TopCallBar(): React.JSX.Element | null {
  const call = useVoice((s) => s.call)
  const native = useVoice((s) => s.engineKind) === 'native'
  const stageShown = useCallStageShown()
  const inConversation = useCallInItsConversation()
  const videoHere = useCallVideoHere()
  const compact = useCallPlace((s) => s.compact)
  const address = useCallConversationAddress(call)
  if (!call || !native || stageShown || inConversation) return null
  let action: React.ReactNode = null
  if (callHasConversation(call)) {
    if (address) {
      action = (
        <Button variant="quiet" data-testid="call-bar-back" title="Back to call: open the conversation the call is in" onClick={() => openCallConversation(address, call.roomName)}>
          Back to call
        </Button>
      )
    }
  } else if (videoHere && compact) {
    action = <CompactButton testId="call-bar-expand" compact />
  }
  return <CallBar call={call} look="top" action={action} />
}
