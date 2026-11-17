// Where the call's stage sits in the page:
// - ConversationCallPlace: at the top of the call's conversation (a DM's
//   view), the messages below it. The stage (tiles and the call's controls)
//   while video is on and the stage is open; otherwise the slim bar (name,
//   time, controls), with Expand while video is on.
// - RoomCallBand: a voice room has no view of its own, so its stage sits at
//   the top of the page's content area, the page under it. Folded, the top
//   bar's slim bar has the call.
// Both are in the page's flow: never floating over content, never pinned to
// a corner. The stage is drawn only where the call's video is shown in this
// window (CallLayer's `useCallStageShown`), so each track has one sink.

import { useVoice } from '@/stores/voice'
import { callHasConversation, callPlaceKey, useCallDock, useCallPlace } from '@/stores/call-place'
import type { VoiceCall } from '@/stores/voice-core'
import CallBar from './CallBar'
import { CallStageView, useCallStageShown, useCallVideoHere } from './CallLayer'
import { CompactButton } from './CallViewButtons'
import CallHereButton, { useCallHereAction } from './CallHereButton'

/** The stage at the top of a conversation: under half the window, so the
 *  messages stay in view. Its height is set, so its tiles fit the room left
 *  above its controls (StageGrid) instead of running under them. */
const STAGE_IN_PAGE = 'flex-shrink-0 h-[calc(var(--k2-vh)*0.45)]'

function PlaceStageOrBar({ call }: { call: VoiceCall }): React.JSX.Element {
  const stageShown = useCallStageShown()
  const videoHere = useCallVideoHere()
  const compact = useCallPlace((s) => s.compact)
  // Not the call's home: Show call here (or Back to chat) moves it here.
  const away = useCallHereAction(call)
  if (stageShown) return <CallStageView call={call} className={`${STAGE_IN_PAGE} border-x-0 border-t-0`} />
  const action = away ? <CallHereButton kind={away} /> : videoHere && compact ? <CompactButton testId="call-bar-expand" compact /> : null
  return <CallBar call={call} look="dock" action={action} />
}

/** The call's place at the top of a conversation (`hostKey`, `roomId`: the
 *  conversation's call room). `fallback`: what the conversation shows of its
 *  own call where this app has no engine for it (the web client's seats). */
export function ConversationCallPlace({ hostKey, roomId, fallback = null }: { hostKey: string; roomId: string; fallback?: React.ReactNode }): React.JSX.Element | null {
  const call = useVoice((s) => s.call)
  const native = useVoice((s) => s.engineKind) === 'native'
  const mine = call !== null && call.hostKey === hostKey && call.roomId === roomId
  useCallDock(mine && native ? callPlaceKey({ hostKey, roomId }) : null)
  if (!mine || !call) return null
  if (!native) return <>{fallback}</>
  return <PlaceStageOrBar call={call} />
}

/** A voice room's stage, at the top of the page's content area. */
export function RoomCallBand(): React.JSX.Element | null {
  const call = useVoice((s) => s.call)
  const stageShown = useCallStageShown()
  if (!call || callHasConversation(call) || !stageShown) return null
  return <CallStageView call={call} className={`${STAGE_IN_PAGE} border-x-0 border-t-0`} />
}
