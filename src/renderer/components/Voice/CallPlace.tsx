// Where the call's stage sits in the page, once the person chose Expand:
// - ConversationCallPlace: at the top of the call's conversation (a DM's
//   view), the messages below it.
// - RoomCallBand: a voice room has no view of its own, so its stage sits at
//   the top of the page's content area, the page under it.
// Both are in the page's flow: never floating over content. Otherwise the
// conversation gives the call no room at all: an audio-only call is the top
// bar's slim bar, and a call with video the floating tile (CallTile). The
// stage is drawn only where CallLayer's `useCallStageShown` says, so each
// track has one sink.

import { useVoice } from '@/stores/voice'
import { callHasConversation, callPlaceKey, useCallDock } from '@/stores/call-place'
import { CallStageView, useCallStageShown } from './CallLayer'

/** The stage at the top of a conversation: under half the window, so the
 *  messages stay in view. Its height is set, so its tiles fit the room left
 *  above its controls (StageGrid) instead of running under them. */
const STAGE_IN_PAGE = 'flex-shrink-0 h-[calc(var(--k2-vh)*0.45)]'

/** The call's place at the top of a conversation (`hostKey`, `roomId`: the
 *  conversation's call room). `fallback`: what the conversation shows of its
 *  own call where this app has no engine for it (the web client's seats). */
export function ConversationCallPlace({ hostKey, roomId, fallback = null }: { hostKey: string; roomId: string; fallback?: React.ReactNode }): React.JSX.Element | null {
  const call = useVoice((s) => s.call)
  const native = useVoice((s) => s.engineKind) === 'native'
  const mine = call !== null && call.hostKey === hostKey && call.roomId === roomId
  useCallDock(mine && native ? callPlaceKey({ hostKey, roomId }) : null)
  const stageShown = useCallStageShown()
  if (!mine || !call) return null
  if (!native) return <>{fallback}</>
  if (!stageShown) return null
  return <CallStageView call={call} className={`${STAGE_IN_PAGE} border-x-0 border-t-0`} />
}

/** A voice room's stage, at the top of the page's content area. */
export function RoomCallBand(): React.JSX.Element | null {
  const call = useVoice((s) => s.call)
  const stageShown = useCallStageShown()
  if (!call || callHasConversation(call) || !stageShown) return null
  return <CallStageView call={call} className={`${STAGE_IN_PAGE} border-x-0 border-t-0`} />
}
