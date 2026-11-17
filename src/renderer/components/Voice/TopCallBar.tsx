// The call in the top bar: the slim in-call bar beside the headset (name,
// time, controls), in the top bar's flow, never a box over the content. It
// is the whole call while the call is audio only, in its conversation too,
// and the call wherever its floating tile or its stage isn't showing in this
// window (Settings open, full screen, away from the stage's conversation,
// another window the call's home). It gives way while the tile or the stage
// is up. Its actions say where the call goes from here:
// - in the call's home window: Expand (the stage in its place; it opens a
//   DM call's conversation first), or Back to call once the stage is the
//   person's choice and only its conversation is away; and Pop out;
// - in a window that isn't the call's home: Show call here (Back to chat in
//   the window the call's own window came from).
// A DM this window can't place has no Expand or Back to call: never a dead
// button.

import { callHasConversation, useCallDocked, useCallPlace } from '@/stores/call-place'
import { useVoice } from '@/stores/voice'
import CallBar from './CallBar'
import { useCallShowsHere, useCallStageShown, useCallTileShown } from './CallLayer'
import { ExpandBarButton, PopOutBarButton } from './CallViewButtons'
import { openCallConversation, useCallConversationAddress, useExpandCall } from './callConversation'
import CallHereButton, { useCallHereAction } from './CallHereButton'
import type { VoiceCall } from '@/stores/voice-core'

export default function TopCallBar(): React.JSX.Element | null {
  const call = useVoice((s) => s.call)
  const native = useVoice((s) => s.engineKind) === 'native'
  const stageShown = useCallStageShown()
  const tileShown = useCallTileShown()
  if (!call || !native || stageShown || tileShown) return null
  return <TopCallBarFor call={call} />
}

function TopCallBarFor({ call }: { call: VoiceCall }): React.JSX.Element {
  const away = useCallHereAction(call)
  const showsHere = useCallShowsHere()
  const choice = useCallPlace((s) => s.choice)
  const docked = useCallDocked(call)
  const address = useCallConversationAddress(call)
  const expand = useExpandCall(call)
  let action: React.ReactNode = null
  if (away) {
    // Back to chat brings the stage back, so it opens the call's
    // conversation too, where it has one.
    const then = away === 'back-to-chat' && address ? () => openCallConversation(address, call.roomName) : undefined
    action = <CallHereButton kind={away} then={then} />
  } else if (showsHere) {
    // The stage is already the choice, its conversation away: Back to call.
    const back = choice === 'stage' && callHasConversation(call) && !docked
    action = (
      <>
        {expand && (choice !== 'stage' || back) ? <ExpandBarButton back={back} onPress={expand} /> : null}
        <PopOutBarButton />
      </>
    )
  }
  return <CallBar call={call} action={action} />
}
